//! M6 forge (lane H2): gh-assisted GitHub flow (contracts.md "Commands (M6
//! forge)").
//!
//! Every GitHub interaction shells out to the **user's** `gh` CLI in JSON
//! mode with the repository workdir as cwd — mygitui stores no GitHub tokens
//! and never talks to the API itself; `gh` brings its own auth. Process
//! rules (mirrors `actions.rs`/`cli.rs` discipline): argv arrays only (no
//! shell), piped streams drained concurrently (a chatty child must never
//! block on a full pipe), and a hard timeout — the waiter polls `try_wait`
//! and kills at the deadline (version probe 3 s, `auth status` 10 s,
//! list/checks 30–60 s, `pr create` 120 s).
//!
//! The process runner ([`run_program`]) is generic over the program name so
//! `forge_tests` can exercise the timeout kill and spawn-failure paths with
//! `ping`/`sleep` instead of `gh`; everything else here is either a pure
//! function (remote-URL parsing, gh output parsing — fixture-tested) or a
//! thin `_sync` orchestrator the IPC commands wrap in `spawn_blocking`
//! (no op-queue membership: these are spec'd long ops with their own
//! timeouts, no event stream needed).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The `gh` binary probed on PATH (production runner target).
const GH: &str = "gh";

/// How often the waiter polls `try_wait` (mirrors `actions.rs`).
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Grace for the pipe readers to hit EOF after exit/kill before the outcome
/// proceeds with whatever was collected (a killed child's pipes close with
/// it; the grace only guards pathological survivors).
const READER_GRACE: Duration = Duration::from_secs(2);

/// `gh --version` probe budget.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(3);
/// `gh auth status` budget (may validate the token against the API).
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// `gh pr create` budget (a real create includes the push-side upload).
pub const CREATE_TIMEOUT: Duration = Duration::from_secs(120);
/// `gh pr list` budget.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(60);
/// `gh pr checks` budget (JSON attempt and table fallback each).
pub const CHECKS_TIMEOUT: Duration = Duration::from_secs(30);
/// Budget for mutating PR/action commands (merge/close/reopen/checkout/
/// review/comment, run cancel/rerun, workflow dispatch).
pub const ACTION_TIMEOUT: Duration = Duration::from_secs(120);

/// `--json` field list shared by `pr_list` (all optional fields parse
/// tolerantly, so older gh versions still work).
pub const PR_LIST_JSON_FIELDS: &str = "number,title,headRefName,baseRefName,state,isDraft,url,createdAt,updatedAt,mergedAt,closedAt,author,body,additions,deletions,changedFiles,reviewDecision,mergeable,labels";

/// How many PRs `pr_list` requests (`gh pr list --limit`).
pub const PR_LIST_LIMIT: usize = 50;

/// `--json` field list of `workflow_runs` (`gh run list`).
pub const RUN_LIST_JSON_FIELDS: &str = "attempt,conclusion,createdAt,databaseId,displayTitle,event,headBranch,headSha,name,number,startedAt,status,updatedAt,url,workflowDatabaseId,workflowName";

/// `--json` field list of `workflow_run_detail` (`gh run view`).
pub const RUN_VIEW_JSON_FIELDS: &str = "attempt,conclusion,createdAt,databaseId,displayTitle,event,headBranch,headSha,jobs,name,number,startedAt,status,updatedAt,url,workflowDatabaseId,workflowName";

/// How many runs `workflow_runs` requests by default (`gh run list --limit`).
pub const RUN_LIST_LIMIT: u32 = 30;

/// Upper bound for a caller-supplied run-list limit.
pub const RUN_LIST_LIMIT_MAX: u32 = 100;

/// Budget for run list/detail reads.
pub const RUNS_TIMEOUT: Duration = Duration::from_secs(60);

/// Budget for fetching run logs (gh may download a log archive first).
pub const LOGS_TIMEOUT: Duration = Duration::from_secs(120);

/// Cap on returned log text; a chatty failing run can be tens of MB.
pub const MAX_LOG_CHARS: usize = 1_500_000;

/// Run statuses accepted by `gh run list --status` (the union of lifecycle
/// states and conclusions gh documents).
pub const RUN_STATUSES: [&str; 15] = [
    "queued",
    "completed",
    "in_progress",
    "requested",
    "waiting",
    "pending",
    "action_required",
    "cancelled",
    "failure",
    "neutral",
    "skipped",
    "stale",
    "startup_failure",
    "success",
    "timed_out",
];

/// Cap on a classified error `message` (gh stderr can be a wall of text).
const MAX_ERROR_MESSAGE: usize = 400;

// ---------------------------------------------------------------------------
// Wire types (contracts.md "Commands (M6 forge)"; generated to
// `src/lib/ipc/gen/` via ts-rs, re-exported by `src/lib/ipc/types.ts`)
// ---------------------------------------------------------------------------

/// Result of `forge_status`: is `gh` usable at all?
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default, TS)]
#[ts(export)]
pub struct ForgeStatus {
    /// `gh` was found on PATH and ran.
    pub available: bool,
    /// Parsed `gh --version` number ("2.63.3"); empty when unavailable.
    pub version: String,
    /// `gh auth status` exited 0 (only meaningful when `available`).
    pub authed: bool,
}

/// Owner/repo/branch context of one open repository (forge_context).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgeContext {
    pub owner: String,
    pub repo: String,
    /// Current branch shorthand (commands error on detached HEAD instead).
    pub branch: String,
    /// The `origin` URL the pair was derived from.
    pub remote_url: String,
}

/// `pr_create` success payload.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct PrCreated {
    pub url: String,
    pub number: u64,
}

/// One PR of `pr_list`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct PrInfo {
    pub number: u64,
    pub title: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    /// `OPEN` | `MERGED` | `CLOSED` (verbatim gh value).
    pub state: String,
    pub is_draft: bool,
    pub url: String,
    /// ISO-8601 creation timestamp (optional in tolerant parsing).
    pub created_at: Option<String>,
    /// ISO-8601 last-update timestamp (optional in tolerant parsing).
    pub updated_at: Option<String>,
    /// ISO-8601 merge timestamp (`None` while open/closed-unmerged).
    pub merged_at: Option<String>,
    /// ISO-8601 close timestamp (`None` while open/merged).
    pub closed_at: Option<String>,
    /// Author login (empty when gh omits it).
    pub author: String,
    /// PR description (empty when gh omits it).
    pub body: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    /// `APPROVED` | `CHANGES_REQUESTED` | `REVIEW_REQUIRED` (`""` when no
    /// review is required/known).
    pub review_decision: String,
    /// `MERGEABLE` | `CONFLICTING` | `UNKNOWN` (`""` when absent).
    pub mergeable: String,
    /// Label names, in gh order.
    pub labels: Vec<String>,
}

/// One CI check of `pr_checks`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct CheckInfo {
    pub name: String,
    /// Canonical: `"pass" | "fail" | "pending" | "skipping"` (kept as a
    /// string-literal union in the generated TS type — the FE narrows it
    /// via `CheckState`).
    #[ts(type = "\"pass\" | \"fail\" | \"pending\" | \"skipping\"")]
    pub state: String,
}

/// One GitHub Actions workflow run of `workflow_runs`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct WorkflowRun {
    /// GitHub's run id (`databaseId`; used by cancel/rerun/logs).
    pub database_id: u64,
    /// Per-workflow run number (`#27`).
    pub number: u64,
    /// Workflow name (`gh`'s `name`/`workflowName`).
    pub name: String,
    pub display_title: String,
    /// Trigger event (`push`, `pull_request`, `workflow_dispatch`, `schedule`, …).
    pub event: String,
    pub head_branch: String,
    pub head_sha: String,
    /// `queued` | `in_progress` | `completed` | `waiting` | `requested` | …
    pub status: String,
    /// `success` | `failure` | `cancelled` | … (empty while running).
    pub conclusion: String,
    pub url: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub started_at: Option<String>,
    pub attempt: u64,
    pub workflow_database_id: u64,
}

/// One step inside a workflow job.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct WorkflowStep {
    pub number: u64,
    pub name: String,
    pub status: String,
    /// Empty while the step has not completed.
    pub conclusion: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// One job of a workflow run.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct WorkflowJob {
    pub database_id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub url: String,
    pub steps: Vec<WorkflowStep>,
}

/// `workflow_run_detail`: the run plus its jobs/steps.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct WorkflowRunDetail {
    pub run: WorkflowRun,
    pub jobs: Vec<WorkflowJob>,
}

/// One workflow file of `workflow_list`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct WorkflowInfo {
    pub id: u64,
    pub name: String,
    /// Path in the repository (`.github/workflows/ci.yml`).
    pub path: String,
    /// `active` | `disabled_manually` | `disabled_inactivity` | …
    pub state: String,
}

/// One `key=value` input of `workflow_dispatch`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct WorkflowInput {
    pub key: String,
    pub value: String,
}

/// Structured `pr_create` failure (the command's rejection value; FE name
/// `ForgeFailure`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgeError {
    /// `NoGh` | `NotAuthed` | `AlreadyExists` | `Other` (kept as a
    /// string-literal union in the generated TS type — the FE narrows it
    /// via `ForgeFailureKind`; values are the `KIND_*` consts).
    #[ts(type = "\"NoGh\" | \"NotAuthed\" | \"AlreadyExists\" | \"Other\"")]
    pub kind: String,
    pub message: String,
    /// For `AlreadyExists`: the existing PR's URL when parseable.
    pub url: Option<String>,
}

/// `ForgeError.kind` values (contracts.md).
pub const KIND_NO_GH: &str = "NoGh";
pub const KIND_NOT_AUTHED: &str = "NotAuthed";
pub const KIND_ALREADY_EXISTS: &str = "AlreadyExists";
pub const KIND_OTHER: &str = "Other";

impl ForgeError {
    pub fn other(message: impl Into<String>) -> Self {
        Self {
            kind: KIND_OTHER.to_string(),
            message: message.into(),
            url: None,
        }
    }
}

impl std::fmt::Display for ForgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

// ---------------------------------------------------------------------------
// Process runner (generic so tests can target `ping`/`sleep`)
// ---------------------------------------------------------------------------

/// Outcome of one [`run_program`] launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcOutcome {
    pub stdout: String,
    pub stderr: String,
    /// Exit code; `None` when killed at the deadline or the code was a
    /// signal.
    pub code: Option<i32>,
    /// True when the deadline was hit and the child was killed.
    pub timed_out: bool,
    /// Spawn failure — usually "program not on PATH".
    pub spawn_error: Option<String>,
}

/// Run `program args…` (argv array, **no shell**) optionally in `cwd`, with
/// piped streams drained by reader threads and a hard `timeout` (kill on
/// exceed). The environment is inherited verbatim — `gh` needs the user's
/// PATH, config, token helpers and proxies exactly as in their shell.
pub fn run_program(
    program: &str,
    cwd: Option<&Path>,
    args: &[String],
    timeout: Duration,
) -> ProcOutcome {
    let mut cmd = crate::process::command(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            return ProcOutcome {
                spawn_error: Some(format!("{program}: {err}")),
                ..ProcOutcome::default()
            };
        }
    };
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    // One reader per stream: read to EOF (draining so the child never
    // blocks on a full pipe) and hand the bytes over.
    fn spawn_drain(pipe: Option<impl Read + Send + 'static>) -> std::sync::mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(pipe) = pipe {
            let _ = std::thread::Builder::new()
                .name("forge-drain".into())
                .spawn(move || {
                    let mut buf = Vec::new();
                    let mut pipe = pipe;
                    let _ = pipe.read_to_end(&mut buf);
                    let _ = tx.send(buf);
                });
        }
        rx
    }
    let stdout_rx = spawn_drain(stdout_pipe);
    let stderr_rx = spawn_drain(stderr_pipe);

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(WAIT_POLL_INTERVAL),
            Err(_) => break None,
        }
    };

    ProcOutcome {
        stdout: recv_lossy(&stdout_rx),
        stderr: recv_lossy(&stderr_rx),
        code: status.and_then(|s| s.code()),
        timed_out,
        spawn_error: None,
    }
}

/// Wait (bounded) for one reader channel and decode lossy UTF-8.
fn recv_lossy(rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> String {
    match rx.recv_timeout(READER_GRACE) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Pure parsing (fixture-tested)
// ---------------------------------------------------------------------------

/// Parse a GitHub remote URL into `(owner, repo)`. Accepted forms (host
/// compared case-insensitively, `www.` stripped, `.git` suffix optional):
/// `https://github.com/o/r`, `http|ssh|git://[git@]github.com/o/r(.git)`,
/// scp-like `git@github.com:o/r(.git)`. Non-GitHub hosts and anything with
/// fewer/more than two path segments yield `None`.
pub fn parse_github_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        let _ = scheme; // any scheme (https/http/ssh/git/git+ssh); the host gates
        let (authority, path) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx + 1..]),
            None => (rest, ""),
        };
        // Drop userinfo (`git@`) and port (`:22`).
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host = host.split(':').next().unwrap_or(host);
        (host, path)
    } else {
        let colon = url.find(':')?;
        // scp-like: `git@github.com:owner/repo.git` (no scheme).
        let authority = &url[..colon];
        let host = authority.rsplit('@').next().unwrap_or(authority);
        (host, &url[colon + 1..])
    };

    let host = host.trim().to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if host != "github.com" {
        return None;
    }
    let path = path.trim().trim_end_matches('/');
    // Case-insensitive `.git` suffix strip (byte-safe: `get` only accepts
    // the slice when it lands on a char boundary).
    let path = match path.get(path.len().saturating_sub(4)..) {
        Some(suffix) if suffix.eq_ignore_ascii_case(".git") => &path[..path.len() - 4],
        _ => path,
    };
    let mut segments = path.split('/');
    let owner = segments.next()?.trim();
    let repo = segments.next()?.trim();
    if owner.is_empty() || repo.is_empty() || segments.next().is_some() {
        return None;
    }
    Some((owner.to_owned(), repo.to_owned()))
}

/// Parse the version token out of `gh --version` output
/// ("gh version 2.63.3 (2024-12-11)\n…" → `"2.63.3"`; unparsable → `""`).
pub fn version_from_output(output: &str) -> String {
    let first = output.lines().next().unwrap_or("");
    let Some(idx) = first.find("version ") else {
        return String::new();
    };
    let rest = &first[idx + "version ".len()..];
    rest.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c| c == '(' || c == ')')
        .to_owned()
}

/// Canonical check `bucket` straight from `gh pr checks --json`.
fn canonical_bucket(bucket: &str) -> Option<&'static str> {
    match bucket {
        "pass" => Some("pass"),
        "fail" => Some("fail"),
        "pending" => Some("pending"),
        "skipping" => Some("skipping"),
        _ => None,
    }
}

/// Map a `state` value (`gh pr checks --json` / table words) onto the
/// canonical four-state set. Unknown states degrade to `pending`.
fn check_state_from_state(state: &str) -> &'static str {
    match state.to_ascii_uppercase().as_str() {
        "SUCCESS" => "pass",
        "FAILURE" | "ERROR" | "CANCELLED" => "fail",
        "SKIPPING" | "SKIPPED" | "NEUTRAL" | "BLANK" => "skipping",
        _ => "pending",
    }
}

/// Map a lowercase table word onto the canonical set (`None` when the token
/// is not a state word).
fn word_state(token: &str) -> Option<&'static str> {
    match token {
        "pass" | "passed" | "success" => Some("pass"),
        "fail" | "failed" | "failure" | "error" | "errors" => Some("fail"),
        "pending" | "queued" | "in_progress" | "waiting" | "requested" | "running" => {
            Some("pending")
        }
        "skipping" | "skipped" | "neutral" | "blank" => Some("skipping"),
        _ => None,
    }
}

/// Parse `gh pr list --json …` output (array of objects) tolerantly:
/// entries without a numeric `number` are skipped, missing strings default
/// to `""`/`false`/`None`.
pub fn parse_pr_list_json(payload: &str) -> Vec<PrInfo> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(payload) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| {
            let number = item.get("number")?.as_u64()?;
            let string_field = |key: &str| {
                item.get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let number_field = |key: &str| {
                item.get(key)
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            };
            let option_string = |key: &str| {
                item.get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            };
            Some(PrInfo {
                number,
                title: string_field("title"),
                head_ref_name: string_field("headRefName"),
                base_ref_name: string_field("baseRefName"),
                state: string_field("state"),
                is_draft: item
                    .get("isDraft")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                url: string_field("url"),
                created_at: option_string("createdAt"),
                updated_at: option_string("updatedAt"),
                merged_at: option_string("mergedAt"),
                closed_at: option_string("closedAt"),
                author: item
                    .get("author")
                    .and_then(|author| author.get("login"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                body: string_field("body"),
                additions: number_field("additions"),
                deletions: number_field("deletions"),
                changed_files: number_field("changedFiles"),
                review_decision: string_field("reviewDecision"),
                mergeable: string_field("mergeable"),
                labels: item
                    .get("labels")
                    .and_then(serde_json::Value::as_array)
                    .map(|labels| {
                        labels
                            .iter()
                            .filter_map(|label| {
                                label.get("name").and_then(serde_json::Value::as_str)
                            })
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Parse `gh pr checks <n> --json name,state,bucket` output tolerantly:
/// `bucket` wins when canonical, `state` maps otherwise, nameless entries
/// are skipped.
pub fn parse_pr_checks_json(payload: &str) -> Vec<CheckInfo> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(payload) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.trim().to_owned();
            if name.is_empty() {
                return None;
            }
            let state = item
                .get("bucket")
                .and_then(serde_json::Value::as_str)
                .and_then(canonical_bucket)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    item.get("state")
                        .and_then(serde_json::Value::as_str)
                        .map(check_state_from_state)
                        .unwrap_or("pending")
                        .to_owned()
                });
            Some(CheckInfo { name, state })
        })
        .collect()
}

/// String field of a gh JSON object (`""` when missing/null/non-string).
fn json_string(item: &serde_json::Value, key: &str) -> String {
    item.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Optional string field of a gh JSON object.
fn json_option_string(item: &serde_json::Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Unsigned field of a gh JSON object (0 when missing/null/non-number).
fn json_u64(item: &serde_json::Value, key: &str) -> u64 {
    item.get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

/// One gh run object → [`WorkflowRun`]; `None` when the run id is missing.
fn parse_workflow_run(item: &serde_json::Value) -> Option<WorkflowRun> {
    let database_id = item.get("databaseId")?.as_u64()?;
    let mut name = json_string(item, "name");
    if name.is_empty() {
        name = json_string(item, "workflowName");
    }
    if name.is_empty() {
        name = json_string(item, "displayTitle");
    }
    Some(WorkflowRun {
        database_id,
        number: json_u64(item, "number"),
        name,
        display_title: json_string(item, "displayTitle"),
        event: json_string(item, "event"),
        head_branch: json_string(item, "headBranch"),
        head_sha: json_string(item, "headSha"),
        status: json_string(item, "status"),
        conclusion: json_string(item, "conclusion"),
        url: json_string(item, "url"),
        created_at: json_option_string(item, "createdAt"),
        updated_at: json_option_string(item, "updatedAt"),
        started_at: json_option_string(item, "startedAt"),
        attempt: json_u64(item, "attempt").max(1),
        workflow_database_id: json_u64(item, "workflowDatabaseId"),
    })
}

/// Parse `gh run list --json …` output tolerantly: entries without a
/// numeric `databaseId` are skipped, missing/null strings default empty.
pub fn parse_workflow_runs_json(payload: &str) -> Vec<WorkflowRun> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(payload) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| parse_workflow_run(&item))
        .collect()
}

/// One gh job object → [`WorkflowJob`] (`None` without a `databaseId`).
fn parse_workflow_job(item: &serde_json::Value) -> Option<WorkflowJob> {
    let database_id = item.get("databaseId")?.as_u64()?;
    let mut name = json_string(item, "name");
    if name.is_empty() {
        name = format!("job {database_id}");
    }
    let steps = item
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .map(|step| WorkflowStep {
                    number: json_u64(step, "number"),
                    name: json_string(step, "name"),
                    status: json_string(step, "status"),
                    conclusion: json_string(step, "conclusion"),
                    started_at: json_option_string(step, "startedAt"),
                    completed_at: json_option_string(step, "completedAt"),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(WorkflowJob {
        database_id,
        name,
        status: json_string(item, "status"),
        conclusion: json_string(item, "conclusion"),
        started_at: json_option_string(item, "startedAt"),
        completed_at: json_option_string(item, "completedAt"),
        url: json_string(item, "url"),
        steps,
    })
}

/// Parse `gh run view <id> --json …jobs…` output; `None` when the payload is
/// not an object with a numeric run id.
pub fn parse_workflow_run_detail_json(payload: &str) -> Option<WorkflowRunDetail> {
    let item: serde_json::Value = serde_json::from_str(payload).ok()?;
    if !item.is_object() {
        return None;
    }
    let run = parse_workflow_run(&item)?;
    let jobs = item
        .get("jobs")
        .and_then(serde_json::Value::as_array)
        .map(|jobs| jobs.iter().filter_map(parse_workflow_job).collect())
        .unwrap_or_default();
    Some(WorkflowRunDetail { run, jobs })
}

/// Parse `gh workflow list --all --json id,name,path,state` output.
pub fn parse_workflow_list_json(payload: &str) -> Vec<WorkflowInfo> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(payload) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_u64()?;
            Some(WorkflowInfo {
                id,
                name: json_string(&item, "name"),
                path: json_string(&item, "path"),
                state: json_string(&item, "state"),
            })
        })
        .collect()
}

/// Fallback check parser for `gh pr checks` table output (older gh without
/// `--json`, or non-tty rows). Each non-empty line carries a check name plus
/// a state word (`pass`/`fail`/`pending`/`skipping`, possibly as
/// `success`/`failure`/…) separated by tabs/spaces, or a leading icon
/// (`✓` pass, `✗`/`X` fail, `-` skipping, `*`/`•` pending). Unparsable
/// lines are dropped.
pub fn parse_pr_checks_table(text: &str) -> Vec<CheckInfo> {
    text.lines().filter_map(parse_check_line).collect()
}

/// One table row → `CheckInfo` (see [`parse_pr_checks_table`]).
fn parse_check_line(line: &str) -> Option<CheckInfo> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Word style (non-tty): tab-separated columns, one of which is a state
    // word; the name is the first column that is not one.
    let columns: Vec<&str> = trimmed.split('\t').map(str::trim).collect();
    if let Some(found) = word_scan(&columns) {
        return Some(found);
    }
    // Icon style: `<icon> name host status elapsed url` — the leading icon
    // carries the state (`✓` pass, `✗`/`X` fail, `-` skipping, `*`/`•`
    // pending).
    if let Some((state, rest)) = icon_state(trimmed) {
        let rest = rest.trim_start();
        let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if name_end > 0 {
            return Some(CheckInfo {
                name: rest[..name_end].to_owned(),
                state: state.to_owned(),
            });
        }
    }
    // Space-separated fallback: `<name> … <state word> …`.
    word_scan(&trimmed.split_whitespace().collect::<Vec<_>>())
}

/// `tokens` contains one state word → `(name = first non-state token,
/// state)`; `None` when no token is a state word.
fn word_scan(tokens: &[&str]) -> Option<CheckInfo> {
    for token in tokens {
        if let Some(state) = word_state(&token.to_ascii_lowercase()) {
            let name = tokens
                .iter()
                .copied()
                .find(|t| !t.is_empty() && word_state(&t.to_ascii_lowercase()).is_none())?;
            return Some(CheckInfo {
                name: name.to_owned(),
                state: state.to_owned(),
            });
        }
    }
    None
}

/// Leading status icon of a `gh pr checks` row → `(state, rest of line)`.
fn icon_state(line: &str) -> Option<(&'static str, &str)> {
    const ICONS: [(&str, &str); 7] = [
        ("✓", "pass"),
        ("✗", "fail"),
        ("X", "fail"),
        ("x", "fail"),
        ("-", "skipping"),
        ("*", "pending"),
        ("•", "pending"),
    ];
    for (icon, state) in ICONS {
        if let Some(rest) = line.strip_prefix(icon) {
            return Some((state, rest));
        }
    }
    None
}

/// Parse the PR number out of a `/pull/<digits>` URL.
pub fn pr_number_of(url: &str) -> Option<u64> {
    let idx = url.find("/pull/")? + "/pull/".len();
    let digits: String = url[idx..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Extract the first `github.com/<owner>/<repo>/pull/<n>` URL from gh
/// output (the create-success line and the already-exists error both carry
/// one). Occurrences are tried left to right; a `/pull/` match only counts
/// when digits follow.
pub fn extract_pull_url(text: &str) -> Option<String> {
    const NEEDLE: &str = "github.com/";
    let mut from = 0;
    while let Some(rel) = text[from..].find(NEEDLE) {
        let start = from + rel;
        let rest = &text[start..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | '(' | ')' | '<' | '>' | ',' | ';')
            })
            .unwrap_or(rest.len());
        let token = &rest[..end];
        if let Some(url) = pull_url_of_token(token) {
            return Some(url);
        }
        from = start + NEEDLE.len();
    }
    None
}

/// `github.com/o/r/pull/12` (with or without scheme prefix in the token) →
/// canonical `https://github.com/o/r/pull/12`.
fn pull_url_of_token(token: &str) -> Option<String> {
    const PREFIX: &str = "github.com/";
    let path = token.strip_prefix(PREFIX)?;
    let mut search = 0;
    while let Some(rel) = path[search..].find("/pull/") {
        let repo = &path[..search + rel];
        let after = &path[search + rel + "/pull/".len()..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            return Some(format!("https://github.com/{repo}/pull/{digits}"));
        }
        search += rel + "/pull/".len();
    }
    None
}

/// stderr markers (lowercased haystack) that mean "gh is not authenticated".
const AUTH_MARKERS: [&str; 5] = [
    "gh auth login",
    "not logged into",
    "unauthorized",
    "bad credentials",
    "gh: to get started",
];

/// Classify one failed gh run into a [`ForgeError`] (contracts.md kinds).
pub fn forge_failure(spawn_error: Option<&str>, combined: &str) -> ForgeError {
    if let Some(err) = spawn_error {
        return ForgeError {
            kind: KIND_NO_GH.to_string(),
            message: format!(
                "gh CLI not found on PATH ({err}) — install it from https://cli.github.com/ and run `gh auth login`"
            ),
            url: None,
        };
    }
    if combined.to_ascii_lowercase().contains("already exists") {
        return ForgeError {
            kind: KIND_ALREADY_EXISTS.to_string(),
            message: "a pull request for this branch already exists".to_string(),
            url: extract_pull_url(combined),
        };
    }
    let lower = combined.to_ascii_lowercase();
    if AUTH_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return ForgeError {
            kind: KIND_NOT_AUTHED.to_string(),
            message: "gh is not authenticated — run `gh auth login`".to_string(),
            url: None,
        };
    }
    ForgeError::other(trim_message(combined))
}

/// Trim gh output into a bounded error message.
fn trim_message(text: &str) -> String {
    let trimmed = text.trim();
    let mut out = if trimmed.is_empty() {
        "gh command failed".to_string()
    } else {
        trimmed.to_string()
    };
    if out.len() > MAX_ERROR_MESSAGE {
        let mut end = MAX_ERROR_MESSAGE;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push('…');
    }
    out
}

// ---------------------------------------------------------------------------
// Sync orchestrators (IPC commands wrap these in spawn_blocking)
// ---------------------------------------------------------------------------

/// Body of `forge_status` (no repository needed): PATH probe + auth check.
pub fn status_sync() -> ForgeStatus {
    let probe = run_program(GH, None, &["--version".to_string()], VERSION_TIMEOUT);
    if probe.spawn_error.is_some() {
        return ForgeStatus::default();
    }
    let version = version_from_output(&probe.stdout);
    let auth = run_program(
        GH,
        None,
        &["auth".to_string(), "status".to_string()],
        AUTH_TIMEOUT,
    );
    ForgeStatus {
        available: true,
        version,
        authed: !auth.timed_out && auth.code == Some(0),
    }
}

/// Body of `forge_context`: owner/repo from the `origin` remote (git2) plus
/// the current branch. Opens its own short-lived `git2::Repository` (a read
/// that touches no shared engine state).
pub fn context_sync(root: &Path) -> Result<ForgeContext, String> {
    let repo = git2::Repository::discover(root).map_err(|e| format!("not a repository: {e}"))?;
    let remote = repo
        .find_remote("origin")
        .map_err(|_| "no `origin` remote configured — the gh flow needs one".to_string())?;
    let remote_url = remote
        .url()
        .map(str::to_owned)
        .map_err(|_| "origin has no URL".to_string())?;
    let (owner, name) = parse_github_remote(&remote_url)
        .ok_or_else(|| format!("origin is not a GitHub remote: {remote_url}"))?;
    if repo.head_detached().unwrap_or(false) {
        return Err("detached HEAD — check out a branch to work with pull requests".to_string());
    }
    let head = repo
        .head()
        .map_err(|e| format!("repository has no HEAD: {e}"))?;
    let branch = head
        .shorthand()
        .map_err(|e| format!("current branch could not be determined: {e}"))?;
    if branch.is_empty() || branch == "HEAD" {
        return Err("current branch could not be determined".to_string());
    }
    Ok(ForgeContext {
        owner,
        repo: name,
        branch: branch.to_owned(),
        remote_url,
    })
}

/// Unique temp file for the PR body (`--body-file`; quoting-proof).
fn write_body_file(body: &str) -> Result<PathBuf, ForgeError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path =
        std::env::temp_dir().join(format!("mygitui-pr-body-{}-{nanos}.md", std::process::id()));
    std::fs::write(&path, body)
        .map_err(|e| ForgeError::other(format!("failed to write PR body temp file: {e}")))?;
    Ok(path)
}

/// Body of `pr_create`: `gh pr create --base <b> --title <t> --body-file
/// <tmp> [--draft]` in the repo cwd, 120 s timeout, temp body file removed
/// afterwards. Success is the PR URL line in the output; failures classify
/// via [`forge_failure`] (already-exists keeps the existing URL).
pub fn pr_create_sync(
    root: &Path,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<PrCreated, ForgeError> {
    let base = base.trim();
    let title = title.trim();
    if base.is_empty() {
        return Err(ForgeError::other("base branch must not be empty"));
    }
    if title.is_empty() {
        return Err(ForgeError::other("title must not be empty"));
    }
    let body_file = write_body_file(body)?;
    let mut args = vec![
        "pr".to_string(),
        "create".to_string(),
        "--base".to_string(),
        base.to_string(),
        "--title".to_string(),
        title.to_string(),
        "--body-file".to_string(),
        body_file.to_string_lossy().into_owned(),
    ];
    if draft {
        args.push("--draft".to_string());
    }
    let outcome = run_program(GH, Some(root), &args, CREATE_TIMEOUT);
    let _ = std::fs::remove_file(&body_file);

    if let Some(err) = &outcome.spawn_error {
        return Err(forge_failure(Some(err), ""));
    }
    let combined = format!("{}\n{}", outcome.stdout, outcome.stderr);
    if outcome.code == Some(0) && !outcome.timed_out {
        if let Some(url) = extract_pull_url(&combined) {
            let number = pr_number_of(&url).unwrap_or(0);
            return Ok(PrCreated { url, number });
        }
        return Err(ForgeError::other(format!(
            "gh pr create succeeded but no PR URL was found in its output: {}",
            trim_message(&combined)
        )));
    }
    if outcome.timed_out {
        return Err(ForgeError::other(
            "gh pr create timed out after 120 s and was killed — check `gh pr list` before retrying",
        ));
    }
    Err(forge_failure(None, &combined))
}

/// Normalize a PR-list state filter (`open`/`closed`/`merged`/`all`);
/// blank means `open`.
pub fn normalize_pr_state(state: &str) -> Result<&'static str, String> {
    match state.trim().to_ascii_lowercase().as_str() {
        "" | "open" => Ok("open"),
        "closed" => Ok("closed"),
        "merged" => Ok("merged"),
        "all" => Ok("all"),
        other => Err(format!(
            "invalid PR state filter {other:?} — use open, closed, merged, or all"
        )),
    }
}

/// Arguments of `pr_list` (state filter normalized by the caller).
pub fn pr_list_args(state: &str) -> Result<Vec<String>, String> {
    let normalized = normalize_pr_state(state)?;
    Ok(vec![
        "pr".to_string(),
        "list".to_string(),
        "--state".to_string(),
        normalized.to_string(),
        "--json".to_string(),
        PR_LIST_JSON_FIELDS.to_string(),
        "--limit".to_string(),
        PR_LIST_LIMIT.to_string(),
    ])
}

/// Body of `pr_list`: `gh pr list --state <state> --json … --limit 50`.
pub fn pr_list_sync(root: &Path, state: &str) -> Result<Vec<PrInfo>, String> {
    let args = pr_list_args(state)?;
    let outcome = run_program(GH, Some(root), &args, LIST_TIMEOUT);
    if let Some(err) = &outcome.spawn_error {
        return Err(format!("gh CLI not found on PATH ({err})"));
    }
    if outcome.code == Some(0) && !outcome.timed_out {
        return Ok(parse_pr_list_json(&outcome.stdout));
    }
    let combined = format!("{}\n{}", outcome.stderr, outcome.stdout);
    Err(trim_message(&combined))
}

/// Shared runner for mutating gh commands: classify spawn failures, timeouts
/// and non-zero exits; return trimmed stdout on success.
fn run_gh_action(
    root: &Path,
    args: Vec<String>,
    label: &str,
    timeout: Duration,
) -> Result<String, ForgeError> {
    let outcome = run_program(GH, Some(root), &args, timeout);
    if let Some(err) = &outcome.spawn_error {
        return Err(forge_failure(Some(err), ""));
    }
    let combined = format!("{}\n{}", outcome.stdout, outcome.stderr);
    if outcome.code == Some(0) && !outcome.timed_out {
        return Ok(outcome.stdout.trim().to_owned());
    }
    if outcome.timed_out {
        return Err(ForgeError::other(format!(
            "`gh {label}` timed out after {} s and was killed — check the result before retrying",
            timeout.as_secs()
        )));
    }
    Err(forge_failure(None, &combined))
}

/// [`run_gh_action`] with the shared mutating-command budget.
fn run_pr_action(root: &Path, args: Vec<String>, label: &str) -> Result<String, ForgeError> {
    run_gh_action(root, args, label, ACTION_TIMEOUT)
}

/// Arguments of `pr_merge`: method flag + optional `--delete-branch`/`--auto`.
/// `method` is `merge`, `squash`, or `rebase`.
pub fn pr_merge_args(
    number: u64,
    method: &str,
    delete_branch: bool,
    auto: bool,
) -> Result<Vec<String>, ForgeError> {
    let flag = match method.trim().to_ascii_lowercase().as_str() {
        "merge" => "--merge",
        "squash" => "--squash",
        "rebase" => "--rebase",
        other => {
            return Err(ForgeError::other(format!(
                "unknown merge method {other:?} — use merge, squash, or rebase"
            )))
        }
    };
    let mut args = vec![
        "pr".to_string(),
        "merge".to_string(),
        number.to_string(),
        flag.to_string(),
    ];
    if delete_branch {
        args.push("--delete-branch".to_string());
    }
    if auto {
        args.push("--auto".to_string());
    }
    Ok(args)
}

/// Body of `pr_merge`: merge/squash/rebase, optional branch deletion and
/// auto-merge (merges once required checks pass).
pub fn pr_merge_sync(
    root: &Path,
    number: u64,
    method: &str,
    delete_branch: bool,
    auto: bool,
) -> Result<String, ForgeError> {
    let args = pr_merge_args(number, method, delete_branch, auto)?;
    run_pr_action(root, args, "pr merge")
}

/// Body of `pr_close`: close a PR, optionally leaving a comment.
pub fn pr_close_sync(
    root: &Path,
    number: u64,
    comment: Option<&str>,
) -> Result<String, ForgeError> {
    let mut args = vec!["pr".to_string(), "close".to_string(), number.to_string()];
    if let Some(comment) = comment.map(str::trim).filter(|c| !c.is_empty()) {
        args.push("--comment".to_string());
        args.push(comment.to_string());
    }
    run_pr_action(root, args, "pr close")
}

/// Body of `pr_reopen`: reopen a closed PR, optionally leaving a comment.
pub fn pr_reopen_sync(
    root: &Path,
    number: u64,
    comment: Option<&str>,
) -> Result<String, ForgeError> {
    let mut args = vec!["pr".to_string(), "reopen".to_string(), number.to_string()];
    if let Some(comment) = comment.map(str::trim).filter(|c| !c.is_empty()) {
        args.push("--comment".to_string());
        args.push(comment.to_string());
    }
    run_pr_action(root, args, "pr reopen")
}

/// Body of `pr_ready`: mark ready for review, or convert back to draft
/// (`gh pr ready --undo`).
pub fn pr_ready_sync(root: &Path, number: u64, ready: bool) -> Result<String, ForgeError> {
    let mut args = vec!["pr".to_string(), "ready".to_string(), number.to_string()];
    if !ready {
        args.push("--undo".to_string());
    }
    run_pr_action(root, args, "pr ready")
}

/// Body of `pr_checkout`: check out the PR's head branch locally.
pub fn pr_checkout_sync(root: &Path, number: u64) -> Result<String, ForgeError> {
    let args = vec!["pr".to_string(), "checkout".to_string(), number.to_string()];
    run_pr_action(root, args, "pr checkout")
}

/// Body of `pr_comment`: post a comment (`gh pr comment --body-file`).
pub fn pr_comment_sync(root: &Path, number: u64, body: &str) -> Result<String, ForgeError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(ForgeError::other("comment body must not be empty"));
    }
    let body_file = write_body_file(body)?;
    let args = vec![
        "pr".to_string(),
        "comment".to_string(),
        number.to_string(),
        "--body-file".to_string(),
        body_file.to_string_lossy().into_owned(),
    ];
    let result = run_pr_action(root, args, "pr comment");
    let _ = std::fs::remove_file(&body_file);
    result
}

/// Body of `pr_review`: approve, request changes, or leave a review comment.
pub fn pr_review_sync(
    root: &Path,
    number: u64,
    action: &str,
    body: &str,
) -> Result<String, ForgeError> {
    let flag = match action.trim().to_ascii_lowercase().as_str() {
        "approve" => "--approve",
        "request-changes" => "--request-changes",
        "comment" => "--comment",
        other => {
            return Err(ForgeError::other(format!(
                "unknown review action {other:?} — use approve, request-changes, or comment"
            )))
        }
    };
    let body = body.trim();
    if flag == "--comment" && body.is_empty() {
        return Err(ForgeError::other("a review comment must not be empty"));
    }
    let mut args = vec![
        "pr".to_string(),
        "review".to_string(),
        number.to_string(),
        flag.to_string(),
    ];
    let body_file = if body.is_empty() {
        None
    } else {
        Some(write_body_file(body)?)
    };
    if let Some(path) = &body_file {
        args.push("--body-file".to_string());
        args.push(path.to_string_lossy().into_owned());
    }
    let result = run_pr_action(root, args, "pr review");
    if let Some(path) = body_file {
        let _ = std::fs::remove_file(path);
    }
    result
}

/// Body of `pr_checks`: JSON mode first (`gh pr checks <n> --json
/// name,state,bucket`); when that invocation fails (older gh, unknown flag),
/// fall back to table output and parse the ✓/✗/- columns.
pub fn pr_checks_sync(root: &Path, number: u64) -> Result<Vec<CheckInfo>, String> {
    let json_args = vec![
        "pr".to_string(),
        "checks".to_string(),
        number.to_string(),
        "--json".to_string(),
        "name,state,bucket".to_string(),
    ];
    let outcome = run_program(GH, Some(root), &json_args, CHECKS_TIMEOUT);
    if outcome.spawn_error.is_none() && outcome.code == Some(0) && !outcome.timed_out {
        let checks = parse_pr_checks_json(&outcome.stdout);
        if !checks.is_empty() {
            return Ok(checks);
        }
    }
    // Fallback: plain table output.
    let table_args = vec!["pr".to_string(), "checks".to_string(), number.to_string()];
    let outcome = run_program(GH, Some(root), &table_args, CHECKS_TIMEOUT);
    if let Some(err) = &outcome.spawn_error {
        return Err(format!("gh CLI not found on PATH ({err})"));
    }
    if outcome.code == Some(0) && !outcome.timed_out {
        return Ok(parse_pr_checks_table(&outcome.stdout));
    }
    let combined = format!("{}\n{}", outcome.stderr, outcome.stdout);
    Err(trim_message(&combined))
}

// ---------------------------------------------------------------------------
// GitHub Actions (browse runs/jobs/steps, logs, cancel/rerun, dispatch)
// ---------------------------------------------------------------------------

/// Normalize a `gh run list --status` filter; blank means no filter.
pub fn normalize_run_status(status: &str) -> Result<Option<&'static str>, String> {
    let status = status.trim().to_ascii_lowercase();
    if status.is_empty() {
        return Ok(None);
    }
    RUN_STATUSES
        .iter()
        .copied()
        .find(|known| *known == status)
        .map(Some)
        .ok_or_else(|| format!("invalid run status filter {status:?}"))
}

/// Arguments of `workflow_runs` (`gh run list --json … --limit N` with the
/// optional branch/status/workflow filters).
pub fn workflow_runs_args(
    limit: u32,
    branch: &str,
    status: &str,
    workflow: &str,
) -> Result<Vec<String>, String> {
    let limit = limit.clamp(1, RUN_LIST_LIMIT_MAX);
    let mut args = vec![
        "run".to_string(),
        "list".to_string(),
        "--json".to_string(),
        RUN_LIST_JSON_FIELDS.to_string(),
        "--limit".to_string(),
        limit.to_string(),
    ];
    let branch = branch.trim();
    if !branch.is_empty() {
        args.push("--branch".to_string());
        args.push(branch.to_string());
    }
    if let Some(status) = normalize_run_status(status)? {
        args.push("--status".to_string());
        args.push(status.to_string());
    }
    let workflow = workflow.trim();
    if !workflow.is_empty() {
        args.push("--workflow".to_string());
        args.push(workflow.to_string());
    }
    Ok(args)
}

/// Run one read-only gh command in the repo cwd; errors are trimmed and
/// spawn failures say the CLI is missing.
fn read_gh(root: &Path, args: &[String], timeout: Duration) -> Result<String, String> {
    let outcome = run_program(GH, Some(root), args, timeout);
    if let Some(err) = &outcome.spawn_error {
        return Err(format!("gh CLI not found on PATH ({err})"));
    }
    if outcome.code == Some(0) && !outcome.timed_out {
        return Ok(outcome.stdout);
    }
    if outcome.timed_out {
        return Err(format!(
            "gh timed out after {} s and was killed",
            timeout.as_secs()
        ));
    }
    let combined = format!("{}\n{}", outcome.stderr, outcome.stdout);
    Err(trim_message(&combined))
}

/// Body of `workflow_runs`: recent Actions runs, newest first.
pub fn workflow_runs_sync(
    root: &Path,
    limit: u32,
    branch: &str,
    status: &str,
    workflow: &str,
) -> Result<Vec<WorkflowRun>, String> {
    let args = workflow_runs_args(limit, branch, status, workflow)?;
    let output = read_gh(root, &args, RUNS_TIMEOUT)?;
    Ok(parse_workflow_runs_json(&output))
}

/// Body of `workflow_run_detail`: one run with its jobs and steps.
pub fn workflow_run_detail_sync(root: &Path, run_id: u64) -> Result<WorkflowRunDetail, String> {
    let args = vec![
        "run".to_string(),
        "view".to_string(),
        run_id.to_string(),
        "--json".to_string(),
        RUN_VIEW_JSON_FIELDS.to_string(),
    ];
    let output = read_gh(root, &args, RUNS_TIMEOUT)?;
    parse_workflow_run_detail_json(&output)
        .ok_or_else(|| "could not parse `gh run view --json` output".to_string())
}

/// Bound huge log payloads at a char boundary.
pub(crate) fn cap_log(mut text: String) -> String {
    if text.chars().count() <= MAX_LOG_CHARS {
        return text;
    }
    let end = text
        .char_indices()
        .nth(MAX_LOG_CHARS)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    text.truncate(end);
    text.push_str("\n… log truncated by mygitui (too large) …\n");
    text
}

/// Body of `workflow_run_logs`: full run log, one job's log, and/or only
/// failed steps (`gh run view <id> [--job <id>] --log[-failed]`).
pub fn workflow_run_logs_sync(
    root: &Path,
    run_id: u64,
    job_id: Option<u64>,
    failed_only: bool,
) -> Result<String, String> {
    let mut args = vec!["run".to_string(), "view".to_string(), run_id.to_string()];
    if let Some(job_id) = job_id {
        args.push("--job".to_string());
        args.push(job_id.to_string());
    }
    args.push(if failed_only { "--log-failed" } else { "--log" }.to_string());
    let output = read_gh(root, &args, LOGS_TIMEOUT)?;
    Ok(cap_log(output))
}

/// Body of `workflow_run_cancel`: cancel a queued or running run.
pub fn workflow_run_cancel_sync(root: &Path, run_id: u64) -> Result<String, ForgeError> {
    let args = vec!["run".to_string(), "cancel".to_string(), run_id.to_string()];
    run_gh_action(root, args, "run cancel", ACTION_TIMEOUT)
}

/// Body of `workflow_run_rerun`: rerun all jobs or only failed ones.
pub fn workflow_run_rerun_sync(
    root: &Path,
    run_id: u64,
    failed_only: bool,
) -> Result<String, ForgeError> {
    let mut args = vec!["run".to_string(), "rerun".to_string(), run_id.to_string()];
    if failed_only {
        args.push("--failed".to_string());
    }
    run_gh_action(root, args, "run rerun", ACTION_TIMEOUT)
}

/// Arguments of `workflow_dispatch`.
pub fn workflow_dispatch_args(
    workflow: &str,
    git_ref: &str,
    inputs: &[WorkflowInput],
) -> Result<Vec<String>, ForgeError> {
    let workflow = workflow.trim();
    if workflow.is_empty() {
        return Err(ForgeError::other("a workflow must be selected"));
    }
    let mut args = vec![
        "workflow".to_string(),
        "run".to_string(),
        workflow.to_string(),
    ];
    let git_ref = git_ref.trim();
    if !git_ref.is_empty() {
        args.push("--ref".to_string());
        args.push(git_ref.to_string());
    }
    for input in inputs {
        let key = input.key.trim();
        if key.is_empty() {
            return Err(ForgeError::other("workflow input names must not be empty"));
        }
        args.push("--raw-field".to_string());
        args.push(format!("{key}={}", input.value));
    }
    Ok(args)
}

/// Body of `workflow_dispatch`: trigger a `workflow_dispatch` run on a ref.
pub fn workflow_dispatch_sync(
    root: &Path,
    workflow: &str,
    git_ref: &str,
    inputs: &[WorkflowInput],
) -> Result<String, ForgeError> {
    let args = workflow_dispatch_args(workflow, git_ref, inputs)?;
    run_gh_action(root, args, "workflow run", ACTION_TIMEOUT)
}

/// Body of `workflow_list`: workflow files, including disabled ones.
pub fn workflow_list_sync(root: &Path) -> Result<Vec<WorkflowInfo>, String> {
    let args = vec![
        "workflow".to_string(),
        "list".to_string(),
        "--all".to_string(),
        "--json".to_string(),
        "id,name,path,state".to_string(),
        "--limit".to_string(),
        "100".to_string(),
    ];
    let output = read_gh(root, &args, RUNS_TIMEOUT)?;
    Ok(parse_workflow_list_json(&output))
}
