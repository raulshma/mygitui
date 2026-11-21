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
pub const PR_LIST_JSON_FIELDS: &str = "number,title,headRefName,baseRefName,state,isDraft,url,createdAt,updatedAt,mergedAt,closedAt,author,body,additions,deletions,changedFiles,reviewDecision,mergeable,labels,headRefOid";

/// How many PRs `pr_list` requests (`gh pr list --limit`).
pub const PR_LIST_LIMIT: usize = 50;

/// `--json` field list of `issue_list` (all optional fields parse
/// tolerantly, so older gh versions still work).
pub const ISSUE_LIST_JSON_FIELDS: &str = "number,title,state,author,updatedAt,labels";

/// How many issues `issue_list` requests (`gh issue list --limit`).
pub const ISSUE_LIST_LIMIT: usize = 50;

/// `--json` field list of `issue_view` (`gh issue view`).
pub const ISSUE_VIEW_JSON_FIELDS: &str =
    "number,title,state,author,body,comments,labels,url,updatedAt";

/// Candidate workdir paths of the pull-request template, tried in order
/// (GitHub's `.github` discovery minus the `docs/` fallback mygitui does
/// not read): the `PULL_REQUEST_TEMPLATE/` directory form wins over the
/// flat file, and the flat file's case variants come last.
pub const PR_TEMPLATE_PATHS: [&str; 3] = [
    ".github/PULL_REQUEST_TEMPLATE/pull_request_template.md",
    ".github/PULL_REQUEST_TEMPLATE.md",
    ".github/pull_request_template.md",
];

/// Budget for `gh api notifications` reads.
pub const NOTIFICATIONS_TIMEOUT: Duration = Duration::from_secs(60);

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
    /// Head commit sha (`headRefOid`; "" when gh omits it) — the snapshot
    /// uses it to detect PR-head vs local-HEAD divergence.
    pub head_ref_oid: String,
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

/// One issue of `issue_list`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgeIssue {
    pub number: u64,
    pub title: String,
    /// `open` | `closed` (verbatim gh value — issues have no `merged`).
    pub state: String,
    /// Author login (empty when gh omits it).
    pub author: String,
    /// ISO-8601 last-update timestamp (optional in tolerant parsing).
    pub updated_at: Option<String>,
    /// Label names, in gh order.
    pub labels: Vec<String>,
}

/// One comment inside [`ForgeIssueDetail`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgeIssueComment {
    /// Comment author login (empty when gh omits it).
    pub author: String,
    /// Markdown body (may be empty when gh omits it).
    pub body: String,
    /// ISO-8601 creation timestamp (optional in tolerant parsing).
    pub created_at: Option<String>,
}

/// `issue_view` payload: one issue plus its comments.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgeIssueDetail {
    pub number: u64,
    pub title: String,
    /// `open` | `closed` (verbatim gh value).
    pub state: String,
    pub author: String,
    /// Markdown body (empty when gh omits it).
    pub body: String,
    pub labels: Vec<String>,
    /// GitHub issue page URL (empty when gh omits it).
    pub url: String,
    pub updated_at: Option<String>,
    pub comments: Vec<ForgeIssueComment>,
}

/// `issue_create` success payload.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct IssueCreated {
    pub url: String,
    pub number: u64,
}

/// One GitHub notification thread (`gh api notifications`; GitHub's default
/// listing is the unread set).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgeNotification {
    /// REST thread id (the mark-read target).
    pub id: u64,
    /// `pull` | `issue` | `commit` | `other` — derived from `subject_url`
    /// (string-literal union in the generated TS type; the FE narrows it
    /// via `ForgeNotificationKind`).
    #[ts(type = "\"pull\" | \"issue\" | \"commit\" | \"other\"")]
    pub kind: String,
    pub subject_title: String,
    /// REST URL of the subject (the PR/issue/commit page the thread is about).
    pub subject_url: String,
    /// Repository name (REST `repository.name`).
    pub repository: String,
    /// ISO-8601 last-update timestamp (optional in tolerant parsing).
    pub updated_at: Option<String>,
    /// Why the user was notified (`review_requested`, `mention`, …).
    pub reason: String,
}

/// Structured `pr_create` failure (the command's rejection value; FE name
/// `ForgeFailure`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgeError {
    /// `NoGh` | `NotAuthed` | `AlreadyExists` | `NotRepo` | `RateLimited` |
    /// `Offline` | `Other` (kept as a string-literal union in the generated
    /// TS type — the FE narrows it via `ForgeFailureKind`; values are the
    /// `KIND_*` consts).
    #[ts(
        type = "\"NoGh\" | \"NotAuthed\" | \"AlreadyExists\" | \"NotRepo\" | \"RateLimited\" | \"Offline\" | \"Other\""
    )]
    pub kind: String,
    pub message: String,
    /// For `AlreadyExists`: the existing PR's URL when parseable.
    pub url: Option<String>,
}

/// `ForgeError.kind` values (contracts.md; E1 adds the degraded-chip kinds —
/// `NotRepo` for non-GitHub/missing remotes, `RateLimited` for 403-quota
/// responses, `Offline` for network-down failures).
pub const KIND_NO_GH: &str = "NoGh";
pub const KIND_NOT_AUTHED: &str = "NotAuthed";
pub const KIND_ALREADY_EXISTS: &str = "AlreadyExists";
pub const KIND_NOT_REPO: &str = "NotRepo";
pub const KIND_RATE_LIMITED: &str = "RateLimited";
pub const KIND_OFFLINE: &str = "Offline";
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
/// fewer/more than two path segments yield `None`. (Production resolution
/// goes through [`parse_github_remote_with_hosts`] with the authed-host
/// cache; this github.com-only form stays as the documented helper.)
#[allow(dead_code)]
pub fn parse_github_remote(url: &str) -> Option<(String, String)> {
    parse_github_remote_with_hosts(url, &[])
}

/// Host token of a URL authority (lowercased, `www.` stripped, port and
/// userinfo dropped) — the comparison form for the allowed-host list.
fn normalize_host_token(raw: &str) -> String {
    let host = raw.trim();
    // Drop userinfo (`git@`) and port (`:22`).
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_owned()
}

/// [`parse_github_remote`] against an allowed-host list: `github.com` always
/// matches, plus every host `gh auth status` reported (E1 — GitHub Enterprise
/// remotes resolve on the same machine gh is authed for). Hosts compare
/// case-insensitively with `www.` stripped. Unknown hosts (a coworker's ssh
/// dev-machine alias) yield `None` — the FE renders the muted "off" chip.
pub fn parse_github_remote_with_hosts(
    url: &str,
    allowed_hosts: &[String],
) -> Option<(String, String)> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let (host, path) = if let Some((_scheme, rest)) = url.split_once("://") {
        // Any scheme (https/http/ssh/git/git+ssh); the host gates.
        let (authority, path) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx + 1..]),
            None => (rest, ""),
        };
        (normalize_host_token(authority), path)
    } else {
        let colon = url.find(':')?;
        // scp-like: `git@github.com:owner/repo.git` (no scheme).
        let authority = &url[..colon];
        (normalize_host_token(authority), &url[colon + 1..])
    };

    let host_allowed = host == "github.com"
        || allowed_hosts
            .iter()
            .any(|h| normalize_host_token(h) == host);
    if !host_allowed {
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

/// Hosts `gh auth status` reported, parsed from its output ("✓ Logged in to
/// <host> account …"; older gh printed "as <user>" after the host). Pure,
/// fixture-tested.
pub fn gh_auth_hosts(output: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for line in output.lines() {
        let Some(idx) = line.to_ascii_lowercase().find("logged in to ") else {
            continue;
        };
        let rest = &line[idx + "logged in to ".len()..];
        let token = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|c: char| ".,:;()".contains(c));
        if token.is_empty() {
            continue;
        }
        let host = normalize_host_token(token);
        if host.is_empty() || host == "account" || host == "as" {
            continue;
        }
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
}

/// The default host: `github.com` is always resolvable (plan §9.1).
pub const DEFAULT_GITHUB_HOST: &str = "github.com";

/// Process-global cache of hosts reported by `gh auth status` (`github.com`
/// always included). Refreshed by [`status_sync`] on auth-state change and by
/// [`hosts_sync`] (the manual-refresh command).
static AUTHED_HOSTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The currently cached host list (`github.com` first; never empty).
pub fn cached_hosts() -> Vec<String> {
    match AUTHED_HOSTS.lock() {
        Ok(guard) => {
            let mut hosts = vec![DEFAULT_GITHUB_HOST.to_string()];
            for host in guard.iter() {
                if host != DEFAULT_GITHUB_HOST && !hosts.contains(host) {
                    hosts.push(host.clone());
                }
            }
            hosts
        }
        Err(_) => vec![DEFAULT_GITHUB_HOST.to_string()],
    }
}

/// Replaces the cache with `hosts` (deduped, `github.com` ensured). Only
/// non-empty input replaces — `gh auth status` on an unauthed machine prints
/// no accounts and must not wipe previously learned Enterprise hosts.
pub fn cache_authed_hosts(hosts: Vec<String>) {
    let learned: Vec<String> = hosts
        .into_iter()
        .map(|h| normalize_host_token(&h))
        .filter(|h| !h.is_empty())
        .collect();
    if learned.is_empty() {
        return;
    }
    if let Ok(mut guard) = AUTHED_HOSTS.lock() {
        *guard = learned;
    }
}

/// Refresh the host cache from `gh auth status` output (no-op on failure).
fn refresh_hosts_from_auth() {
    let outcome = run_program(
        GH,
        None,
        &["auth".to_string(), "status".to_string()],
        AUTH_TIMEOUT,
    );
    let hosts = gh_auth_hosts(&format!("{}\n{}", outcome.stdout, outcome.stderr));
    cache_authed_hosts(hosts);
}

/// Body of the manual host-refresh command: re-runs `gh auth status`, caches
/// what it reports and returns the live list.
pub fn hosts_sync() -> Vec<String> {
    refresh_hosts_from_auth();
    cached_hosts()
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
        "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => {
            "fail"
        }
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
                head_ref_oid: string_field("headRefOid"),
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
                author: json_author_login(&item),
                body: string_field("body"),
                additions: number_field("additions"),
                deletions: number_field("deletions"),
                changed_files: number_field("changedFiles"),
                review_decision: string_field("reviewDecision"),
                mergeable: string_field("mergeable"),
                labels: json_label_names(&item),
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

/// Author login of a gh JSON object (`author.login`; `""` when missing).
fn json_author_login(item: &serde_json::Value) -> String {
    item.get("author")
        .and_then(|author| author.get("login"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Author login of a REST JSON object (`user.login`; `""` when missing).
fn json_rest_user_login(item: &serde_json::Value) -> String {
    item.get("user")
        .and_then(|user| user.get("login"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Label names of a gh JSON object (`labels[].name`, in gh order).
fn json_label_names(item: &serde_json::Value) -> Vec<String> {
    item.get("labels")
        .and_then(serde_json::Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.get("name").and_then(serde_json::Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
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

/// Parse `gh issue list --json …` output tolerantly: entries without a
/// numeric `number` are skipped, missing strings default to `""`/`None`.
pub fn parse_issue_list_json(payload: &str) -> Vec<ForgeIssue> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(payload) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| {
            let number = item.get("number")?.as_u64()?;
            Some(ForgeIssue {
                number,
                title: json_string(&item, "title"),
                state: json_string(&item, "state"),
                author: json_author_login(&item),
                updated_at: json_option_string(&item, "updatedAt"),
                labels: json_label_names(&item),
            })
        })
        .collect()
}

/// Parse `gh issue view <n> --json …` output; `None` when the payload is
/// not an object. Comment entries that are not objects are skipped; the
/// rest parse tolerantly (missing fields default to `""`/`None`).
pub fn parse_issue_view_json(payload: &str) -> Option<ForgeIssueDetail> {
    let item: serde_json::Value = serde_json::from_str(payload).ok()?;
    if !item.is_object() {
        return None;
    }
    let comments = item
        .get("comments")
        .and_then(serde_json::Value::as_array)
        .map(|comments| {
            comments
                .iter()
                .filter(|comment| comment.is_object())
                .map(|comment| ForgeIssueComment {
                    author: json_author_login(comment),
                    body: json_string(comment, "body"),
                    created_at: json_option_string(comment, "createdAt"),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ForgeIssueDetail {
        number: json_u64(&item, "number"),
        title: json_string(&item, "title"),
        state: json_string(&item, "state"),
        author: json_author_login(&item),
        body: json_string(&item, "body"),
        labels: json_label_names(&item),
        url: json_string(&item, "url"),
        updated_at: json_option_string(&item, "updatedAt"),
        comments,
    })
}

/// Notification kind for a subject URL (`/pull/`/`/pulls/` — html vs REST —
/// → pull, `/issues/` → issue, `/commit`/`/commits` → commit, anything else
/// → other). Pure.
pub fn notification_kind(subject_url: &str) -> &'static str {
    if subject_url.contains("/pull/") || subject_url.contains("/pulls/") {
        "pull"
    } else if subject_url.contains("/issues/") {
        "issue"
    } else if subject_url.contains("/commit") {
        "commit"
    } else {
        "other"
    }
}

/// Parse `gh api notifications` output (array of REST thread objects —
/// snake_case keys, `subject`/`repository` nested) tolerantly: entries
/// without a numeric `id` are skipped, missing strings default to `""`/
/// `None`, and `kind` derives from the subject URL.
pub fn parse_notifications_json(payload: &str) -> Vec<ForgeNotification> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(payload) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_u64()?;
            let subject_title = item
                .get("subject")
                .and_then(|subject| subject.get("title"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let subject_url = item
                .get("subject")
                .and_then(|subject| subject.get("url"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let repository = item
                .get("repository")
                .and_then(|repo| repo.get("name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Some(ForgeNotification {
                id,
                kind: notification_kind(&subject_url).to_owned(),
                subject_title,
                subject_url,
                repository,
                updated_at: json_option_string(&item, "updated_at"),
                reason: json_string(&item, "reason"),
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

/// Parse the number out of a `<segment><digits>` URL (`/pull/12` → 12).
fn number_in_url(url: &str, segment: &str) -> Option<u64> {
    let idx = url.find(segment)? + segment.len();
    let digits: String = url[idx..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Parse the PR number out of a `/pull/<digits>` URL.
pub fn pr_number_of(url: &str) -> Option<u64> {
    number_in_url(url, "/pull/")
}

/// Parse the issue number out of an `/issues/<digits>` URL.
pub fn issue_number_of(url: &str) -> Option<u64> {
    number_in_url(url, "/issues/")
}

/// Extract the first `github.com/<owner>/<repo><segment><digits>` URL from
/// gh output (segment e.g. `/pull/` or `/issues/`; the create-success lines
/// and the already-exists errors carry one). Occurrences are tried left to
/// right; a match only counts when digits follow.
fn extract_github_url(text: &str, segment: &str) -> Option<String> {
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
        if let Some(url) = segment_url_of_token(token, segment) {
            return Some(url);
        }
        from = start + NEEDLE.len();
    }
    None
}

/// Extract the first PR URL (`…/pull/<n>`) from gh output.
pub fn extract_pull_url(text: &str) -> Option<String> {
    extract_github_url(text, "/pull/")
}

/// Extract the first issue URL (`…/issues/<n>`) from gh output.
pub fn extract_issue_url(text: &str) -> Option<String> {
    extract_github_url(text, "/issues/")
}

/// `github.com/o/r/issues/12` (with or without scheme prefix in the token)
/// → canonical `https://github.com/o/r/issues/12`.
fn segment_url_of_token(token: &str, segment: &str) -> Option<String> {
    const PREFIX: &str = "github.com/";
    let path = token.strip_prefix(PREFIX)?;
    let mut search = 0;
    while let Some(rel) = path[search..].find(segment) {
        let repo = &path[..search + rel];
        let after = &path[search + rel + segment.len()..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            return Some(format!("https://github.com/{repo}{segment}{digits}"));
        }
        search += rel + segment.len();
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

/// stderr markers (lowercased haystack) that mean GitHub answered 403 with a
/// quota message (the rate-limited degraded state; FE shows "retry in Xm").
const RATE_LIMIT_MARKERS: [&str; 3] = [
    "rate limit exceeded",
    "secondary rate limit",
    "abuse detection",
];

/// stderr markers (lowercased haystack) that mean the machine has no usable
/// network path to GitHub (the offline degraded state).
const OFFLINE_MARKERS: [&str; 6] = [
    "could not resolve host",
    "no such host",
    "connection refused",
    "connection reset by peer",
    "network unreachable",
    "dial tcp",
];

/// Classify one failed gh run into a [`ForgeError`] (contracts.md kinds).
/// Order matters: quota 403s mention auth ("here's the good news: auth") and
/// must classify as `RateLimited` before the auth markers run; offline
/// markers win over the catch-all.
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
    if RATE_LIMIT_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return ForgeError {
            kind: KIND_RATE_LIMITED.to_string(),
            message: "GitHub API rate limit reached — retrying with backoff; `gh auth status` shows the account's quota".to_string(),
            url: None,
        };
    }
    if AUTH_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return ForgeError {
            kind: KIND_NOT_AUTHED.to_string(),
            message: "gh is not authenticated — run `gh auth login`".to_string(),
            url: None,
        };
    }
    if OFFLINE_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return ForgeError {
            kind: KIND_OFFLINE.to_string(),
            message: "GitHub is unreachable — check the network connection".to_string(),
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
/// A successful auth probe also refreshes the authed-host cache (plan §9.1:
/// hosts refresh on auth-state change).
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
    let authed = !auth.timed_out && auth.code == Some(0);
    // Host cache refresh rides the auth probe (both an authed run — which
    // lists accounts — and an unauthed one still print host context).
    let hosts = gh_auth_hosts(&format!("{}\n{}", auth.stdout, auth.stderr));
    cache_authed_hosts(hosts);
    ForgeStatus {
        available: true,
        version,
        authed,
    }
}

/// A GitHub remote resolved from one open repository: the owner/repo pair
/// plus the remote's name and URL (E1 — the snapshot and the remote-override
/// UI both report what was resolved).
pub struct ResolvedRemote {
    pub remote_name: String,
    pub remote_url: String,
    pub owner: String,
    pub repo: String,
}

/// Shared tail of context/snapshot resolution: open the repo (short-lived
/// `git2::Repository`, a read that touches no shared engine state), find
/// `remote` (default `origin`; the per-repo override wins) and parse it
/// against the cached authed-host list. Failures are structured
/// [`ForgeError`]s with kind `NotRepo` (missing remote / not GitHub) — the
/// FE's degraded "off" chip shows the reason verbatim.
pub fn resolve_remote(
    root: &Path,
    remote_override: Option<&str>,
) -> Result<ResolvedRemote, ForgeError> {
    let remote_name = remote_override
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("origin")
        .to_owned();
    let repo = git2::Repository::discover(root)
        .map_err(|e| ForgeError::other(format!("not a repository: {e}")))?;
    let remote = repo.find_remote(&remote_name).map_err(|_| ForgeError {
        kind: KIND_NOT_REPO.to_string(),
        message: format!("no `{remote_name}` remote configured — the GitHub flow needs one"),
        url: None,
    })?;
    let remote_url = remote.url().map(str::to_owned).map_err(|_| ForgeError {
        kind: KIND_NOT_REPO.to_string(),
        message: format!("`{remote_name}` has no URL"),
        url: None,
    })?;
    let (owner, name) =
        parse_github_remote_with_hosts(&remote_url, &cached_hosts()).ok_or_else(|| ForgeError {
            kind: KIND_NOT_REPO.to_string(),
            message: format!("`{remote_name}` is not a GitHub remote: {remote_url}"),
            url: None,
        })?;
    Ok(ResolvedRemote {
        remote_name,
        remote_url,
        owner,
        repo: name,
    })
}

/// Body of `forge_context`: owner/repo from the remote (git2) plus the
/// current branch. `remote_override` selects a non-default remote (per-repo
/// override, E1). Rejection is the structured [`ForgeError`] (`NotRepo` for
/// remote problems) so the FE can render named degraded states.
pub fn context_sync(
    root: &Path,
    remote_override: Option<&str>,
) -> Result<ForgeContext, ForgeError> {
    let resolved = resolve_remote(root, remote_override)?;
    let repo = git2::Repository::discover(root)
        .map_err(|e| ForgeError::other(format!("not a repository: {e}")))?;
    if repo.head_detached().unwrap_or(false) {
        return Err(ForgeError::other(
            "detached HEAD — check out a branch to work with pull requests",
        ));
    }
    let head = repo
        .head()
        .map_err(|e| ForgeError::other(format!("repository has no HEAD: {e}")))?;
    let branch = head
        .shorthand()
        .map_err(|e| ForgeError::other(format!("current branch could not be determined: {e}")))?;
    if branch.is_empty() || branch == "HEAD" {
        return Err(ForgeError::other("current branch could not be determined"));
    }
    Ok(ForgeContext {
        owner: resolved.owner,
        repo: resolved.repo,
        branch: branch.to_owned(),
        remote_url: resolved.remote_url,
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
fn run_gh_mutating(root: &Path, args: Vec<String>, label: &str) -> Result<String, ForgeError> {
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
    run_gh_mutating(root, args, "pr merge")
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
    run_gh_mutating(root, args, "pr close")
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
    run_gh_mutating(root, args, "pr reopen")
}

/// Body of `pr_ready`: mark ready for review, or convert back to draft
/// (`gh pr ready --undo`).
pub fn pr_ready_sync(root: &Path, number: u64, ready: bool) -> Result<String, ForgeError> {
    let mut args = vec!["pr".to_string(), "ready".to_string(), number.to_string()];
    if !ready {
        args.push("--undo".to_string());
    }
    run_gh_mutating(root, args, "pr ready")
}

/// Body of `pr_checkout`: check out the PR's head branch locally.
pub fn pr_checkout_sync(root: &Path, number: u64) -> Result<String, ForgeError> {
    let args = vec!["pr".to_string(), "checkout".to_string(), number.to_string()];
    run_gh_mutating(root, args, "pr checkout")
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
    let result = run_gh_mutating(root, args, "pr comment");
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
    let result = run_gh_mutating(root, args, "pr review");
    if let Some(path) = body_file {
        let _ = std::fs::remove_file(path);
    }
    result
}

// -- PR edit + template ------------------------------------------------------

/// Arguments of `pr_edit` (`gh pr edit <n> [--title <t>] [--body-file <f>]`).
/// `body_file` is the temp file [`write_body_file`] produced for the new
/// body; at least one of title/body must be present (gh rejects an empty
/// edit).
pub fn pr_edit_args(
    number: u64,
    title: Option<&str>,
    body_file: Option<&Path>,
) -> Result<Vec<String>, ForgeError> {
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    if title.is_none() && body_file.is_none() {
        return Err(ForgeError::other(
            "nothing to edit — provide a title or a body",
        ));
    }
    let mut args = vec!["pr".to_string(), "edit".to_string(), number.to_string()];
    if let Some(title) = title {
        args.push("--title".to_string());
        args.push(title.to_string());
    }
    if let Some(path) = body_file {
        args.push("--body-file".to_string());
        args.push(path.to_string_lossy().into_owned());
    }
    Ok(args)
}

/// Body of `pr_edit`: update a PR's title and/or body (`gh pr edit`, the
/// shared 120 s mutating budget; the body travels through the same temp
/// file channel as `pr_create`, so quoting never matters). The temp file
/// is removed on both success and failure paths.
pub fn pr_edit_sync(
    root: &Path,
    number: u64,
    title: Option<&str>,
    body: Option<&str>,
) -> Result<String, ForgeError> {
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    let body_file = match body.map(str::trim).filter(|b| !b.is_empty()) {
        Some(body) => Some(write_body_file(body)?),
        None => None,
    };
    let result = pr_edit_args(number, title, body_file.as_deref())
        .and_then(|args| run_gh_mutating(root, args, "pr edit"));
    if let Some(path) = body_file {
        let _ = std::fs::remove_file(path);
    }
    result
}

/// First present template among [`PR_TEMPLATE_PATHS`], resolved through
/// `read` (`None` = no template). A missing file or non-UTF-8 content reads
/// as absent; the first readable candidate wins verbatim.
pub fn pick_pr_template(read: impl Fn(&str) -> Option<String>) -> Option<String> {
    PR_TEMPLATE_PATHS.iter().find_map(|rel| read(rel))
}

/// Body of `pr_template` (local repo): first template file under `root`.
/// The template is an optional convenience, so read errors (permissions)
/// count as "absent" instead of failing the command.
pub fn pr_template_sync(root: &Path) -> Option<String> {
    pick_pr_template(|rel| std::fs::read_to_string(root.join(rel)).ok())
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
// Issues (browse/close/reopen/create via the same gh patterns as PRs)
// ---------------------------------------------------------------------------

/// Normalize an issue-list state filter (`open`/`closed`/`all`); blank means
/// `open` (issues have no `merged` state).
pub fn normalize_issue_state(state: &str) -> Result<&'static str, String> {
    match state.trim().to_ascii_lowercase().as_str() {
        "" | "open" => Ok("open"),
        "closed" => Ok("closed"),
        "all" => Ok("all"),
        other => Err(format!(
            "invalid issue state filter {other:?} — use open, closed, or all"
        )),
    }
}

/// Arguments of `issue_list` (state filter normalized by the caller).
pub fn issue_list_args(state: &str) -> Result<Vec<String>, String> {
    let normalized = normalize_issue_state(state)?;
    Ok(vec![
        "issue".to_string(),
        "list".to_string(),
        "--state".to_string(),
        normalized.to_string(),
        "--json".to_string(),
        ISSUE_LIST_JSON_FIELDS.to_string(),
        "--limit".to_string(),
        ISSUE_LIST_LIMIT.to_string(),
    ])
}

/// Body of `issue_list`: `gh issue list --state <state> --json … --limit 50`.
pub fn issue_list_sync(root: &Path, state: &str) -> Result<Vec<ForgeIssue>, String> {
    let args = issue_list_args(state)?;
    let output = read_gh(root, &args, LIST_TIMEOUT)?;
    Ok(parse_issue_list_json(&output))
}

/// Body of `issue_view`: one issue with its body and comments
/// (`gh issue view <n> --json …`).
pub fn issue_view_sync(root: &Path, number: u64) -> Result<ForgeIssueDetail, String> {
    let args = vec![
        "issue".to_string(),
        "view".to_string(),
        number.to_string(),
        "--json".to_string(),
        ISSUE_VIEW_JSON_FIELDS.to_string(),
    ];
    let output = read_gh(root, &args, LIST_TIMEOUT)?;
    parse_issue_view_json(&output)
        .ok_or_else(|| "could not parse `gh issue view --json` output".to_string())
}

/// Shared tail of issue close/reopen: `gh issue <verb> <n> [--comment c]`.
fn issue_state_change(
    root: &Path,
    verb: &str,
    number: u64,
    comment: Option<&str>,
) -> Result<String, ForgeError> {
    let mut args = vec!["issue".to_string(), verb.to_string(), number.to_string()];
    if let Some(comment) = comment.map(str::trim).filter(|c| !c.is_empty()) {
        args.push("--comment".to_string());
        args.push(comment.to_string());
    }
    run_gh_mutating(root, args, &format!("issue {verb}"))
}

/// Body of `issue_close`: close an issue, optionally leaving a comment.
pub fn issue_close_sync(
    root: &Path,
    number: u64,
    comment: Option<&str>,
) -> Result<String, ForgeError> {
    issue_state_change(root, "close", number, comment)
}

/// Body of `issue_reopen`: reopen a closed issue, optionally leaving a
/// comment.
pub fn issue_reopen_sync(
    root: &Path,
    number: u64,
    comment: Option<&str>,
) -> Result<String, ForgeError> {
    issue_state_change(root, "reopen", number, comment)
}

/// Body of `issue_create`: `gh issue create --title <t> [--body-file <tmp>]`
/// (the body is optional). Success is the issue URL line in the output;
/// failures classify via [`forge_failure`].
pub fn issue_create_sync(root: &Path, title: &str, body: &str) -> Result<IssueCreated, ForgeError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(ForgeError::other("title must not be empty"));
    }
    let body_file = if body.trim().is_empty() {
        None
    } else {
        Some(write_body_file(body)?)
    };
    let mut args = vec![
        "issue".to_string(),
        "create".to_string(),
        "--title".to_string(),
        title.to_string(),
    ];
    if let Some(path) = &body_file {
        args.push("--body-file".to_string());
        args.push(path.to_string_lossy().into_owned());
    }
    let outcome = run_program(GH, Some(root), &args, CREATE_TIMEOUT);
    if let Some(path) = body_file {
        let _ = std::fs::remove_file(path);
    }

    if let Some(err) = &outcome.spawn_error {
        return Err(forge_failure(Some(err), ""));
    }
    let combined = format!("{}\n{}", outcome.stdout, outcome.stderr);
    if outcome.code == Some(0) && !outcome.timed_out {
        if let Some(url) = extract_issue_url(&combined) {
            let number = issue_number_of(&url).unwrap_or(0);
            return Ok(IssueCreated { url, number });
        }
        return Err(ForgeError::other(format!(
            "gh issue create succeeded but no issue URL was found in its output: {}",
            trim_message(&combined)
        )));
    }
    if outcome.timed_out {
        return Err(ForgeError::other(
            "gh issue create timed out after 120 s and was killed — check `gh issue list` before retrying",
        ));
    }
    Err(forge_failure(None, &combined))
}

// ---------------------------------------------------------------------------
// Notifications (gh api notifications — list, mark one / all read)
// ---------------------------------------------------------------------------

/// Body of `notifications_list`: the user's unread notification threads
/// (`gh api notifications`).
pub fn notifications_list_sync(root: &Path) -> Result<Vec<ForgeNotification>, String> {
    let args = vec!["api".to_string(), "notifications".to_string()];
    let output = read_gh(root, &args, NOTIFICATIONS_TIMEOUT)?;
    Ok(parse_notifications_json(&output))
}

/// Body of `notification_mark_read`: mark one thread read
/// (`gh api threads/<id> -X PATCH`).
pub fn notification_mark_read_sync(root: &Path, id: u64) -> Result<(), ForgeError> {
    let args = vec![
        "api".to_string(),
        format!("threads/{id}"),
        "-X".to_string(),
        "PATCH".to_string(),
    ];
    run_gh_mutating(root, args, "api threads").map(|_| ())
}

/// Body of `notifications_mark_all_read`: mark every thread read
/// (`gh api notifications -X PUT`).
pub fn notifications_mark_all_read_sync(root: &Path) -> Result<(), ForgeError> {
    let args = vec![
        "api".to_string(),
        "notifications".to_string(),
        "-X".to_string(),
        "PUT".to_string(),
    ];
    run_gh_mutating(root, args, "api notifications").map(|_| ())
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

/// `gh api …/jobs/<id>/logs` starts with a UTF-8 BOM; drop every BOM so the
/// parser sees the first timestamp.
pub(crate) fn strip_log_bom(text: &str) -> String {
    text.replace('\u{feff}', "")
}

/// The REST job-log payload is plain text. Prefix each line with the job name
/// in `gh run view --log`'s tab-separated shape so step correlation works.
pub(crate) fn prefix_job_log(job_name: &str, text: &str) -> String {
    let mut combined = String::new();
    for line in text.lines() {
        combined.push_str(job_name);
        combined.push('\t');
        combined.push('\t');
        combined.push_str(line);
        combined.push('\n');
    }
    combined
}

/// Fetch one completed job's log archive. `None` while GitHub has not
/// published it yet (in-progress jobs return `404 BlobNotFound`).
fn fetch_job_log(root: &Path, job_id: u64) -> Option<String> {
    let api_args = vec![
        "api".to_string(),
        format!("repos/:owner/:repo/actions/jobs/{job_id}/logs"),
    ];
    let outcome = run_program(GH, Some(root), &api_args, Duration::from_secs(30));
    if outcome.code == Some(0) && !outcome.stdout.trim().is_empty() {
        Some(strip_log_bom(&outcome.stdout))
    } else {
        None
    }
}

/// Conclusion values that count as a failed job for the failed-only filter.
fn workflow_job_failed(job: &WorkflowJob) -> bool {
    matches!(
        job.conclusion.as_str(),
        "failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure"
    )
}

/// Body of `workflow_run_logs`: full run log, one job's log, and/or only
/// failed steps (`gh run view <id> [--job <id>] --log[-failed]`).
///
/// `gh run view --log` only serves the completed archive. While a run is in
/// progress (or the archive is still being built), fall back to the per-job
/// REST logs GitHub publishes as each job finishes. In-progress jobs have no
/// log text through the API; the caller polls again and the UI links to the
/// live console on GitHub. Empty output while the run is active is a normal
/// state, not an error.
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
    let output = read_gh(root, &args, LOGS_TIMEOUT);
    if let Ok(out) = &output {
        if !out.trim().is_empty() {
            return Ok(cap_log(strip_log_bom(out)));
        }
    }

    // Live logs fallback for in-progress runs / pending archives. The detail
    // lookup also tells us whether an empty result is expected.
    let detail = workflow_run_detail_sync(root, run_id).ok();
    let run_active = detail
        .as_ref()
        .map(|detail| detail.run.status != "completed")
        .unwrap_or(false);

    if let Some(job_id) = job_id {
        let job = detail
            .as_ref()
            .and_then(|detail| detail.jobs.iter().find(|job| job.database_id == job_id));
        if let Some(log) = fetch_job_log(root, job_id) {
            let name = job.map(|job| job.name.as_str()).unwrap_or("Logs");
            return Ok(cap_log(prefix_job_log(name, &log)));
        }
        if run_active || job.map(|job| job.status != "completed").unwrap_or(false) {
            return Ok(String::new());
        }
    } else if let Some(detail) = detail.as_ref() {
        let mut combined = String::new();
        for job in &detail.jobs {
            // Only completed jobs have a log archive; the next poll picks up
            // the jobs that finish in the meantime.
            if job.status != "completed" {
                continue;
            }
            if failed_only && !workflow_job_failed(job) {
                continue;
            }
            if let Some(log) = fetch_job_log(root, job.database_id) {
                combined.push_str(&prefix_job_log(&job.name, &log));
            }
        }
        if !combined.is_empty() {
            return Ok(cap_log(combined));
        }
        if run_active {
            return Ok(String::new());
        }
    }

    output.map(cap_log)
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

// ---------------------------------------------------------------------------
// Snapshot (E1, plan §9.3): one batched read of the branch's live GitHub
// state — branch PR, HEAD check-runs, PR-head check-runs, latest branch run
// — fanned out over at most 4 CONCURRENT gh calls, with content hashes so
// unchanged snapshots skip FE re-renders.
// ---------------------------------------------------------------------------

/// Content hashes of a [`ForgeSnapshot`]'s four parts (FNV-1a over the
/// serialized part). Equal hashes ⇒ the FE keeps its previous render.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgeSnapshotHashes {
    pub pr: u64,
    pub head_checks: u64,
    pub pr_head_checks: u64,
    pub run: u64,
}

/// One repo's batched GitHub snapshot (E1 footer chips + popovers).
#[derive(Debug, Clone, Serialize, PartialEq, TS)]
#[ts(export)]
pub struct ForgeSnapshot {
    /// Current branch shorthand (`None` on detached HEAD — no branch PR/run
    /// then, but HEAD checks still resolve).
    pub branch: Option<String>,
    /// HEAD commit sha (`None` on an unborn repo).
    pub head_sha: Option<String>,
    /// HEAD is reachable from the remote-tracking branch (`Some(false)` =
    /// unpushed; `None` = unknown — no tracking ref).
    pub pushed: Option<bool>,
    /// The branch's PR, open one preferred (merged/closed kept for the chip's
    /// merged/closed states).
    pub pr: Option<PrInfo>,
    /// Commit-level check-runs of the local HEAD sha.
    pub head_checks: Vec<CheckInfo>,
    /// The branch PR's head sha (`headRefOid`), when gh reported one.
    pub pr_head_sha: Option<String>,
    /// PR-head check-runs — fetched only when the PR head differs from the
    /// local HEAD (`None` = same sha, the head checks cover it).
    pub pr_head_checks: Option<Vec<CheckInfo>>,
    /// Latest Actions run for the branch, newest first's first entry.
    pub run: Option<WorkflowRun>,
    pub hashes: ForgeSnapshotHashes,
    /// Highest-priority failure among the snapshot's parts (NoGh beats
    /// NotAuthed beats RateLimited beats Offline beats Other); `None` when
    /// every part resolved. Missing individual parts with no error just mean
    /// "no such object" (no PR / no runs).
    pub error: Option<ForgeError>,
}

/// One file of a PR's diff (`forge_pr_files`; REST `pulls/<n>/files`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgePrFile {
    pub path: String,
    /// GitHub's file status (`added` | `removed` | `modified` | `renamed` |
    /// `changed`; verbatim REST value).
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    /// Unified patch (REST omits it above its size cap / for binaries).
    pub patch: Option<String>,
}

/// One review comment of a PR (`forge_pr_review_threads`; REST
/// `pulls/<n>/comments`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgeReviewComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: Option<String>,
}

/// One review thread (a top-level review comment plus its replies, read-only
/// for E2's PR-detail view).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct ForgeReviewThread {
    pub id: u64,
    pub path: String,
    /// The commented line (`None` for file-level comments).
    pub line: Option<u64>,
    /// GitHub's "outdated" flag (the commented lines have changed).
    pub outdated: bool,
    pub author: String,
    pub body: String,
    pub created_at: Option<String>,
    pub replies: Vec<ForgeReviewComment>,
}

/// FNV-1a 64-bit (pure, fixture-tested) — the snapshot content hash.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Content hash of any serializable snapshot part (0 on serialization
/// failure — a hash collision with real content is harmless: the FE would
/// just skip a re-render it did not need anyway).
pub fn hash_serialized<T: serde::Serialize>(value: &T) -> u64 {
    match serde_json::to_vec(value) {
        Ok(bytes) => fnv1a64(&bytes),
        Err(_) => 0,
    }
}

/// The branch's PR: the first entry whose head branch matches, preferring
/// `OPEN` over MERGED/CLOSED (mirrors the FE's `prForBranch`). Pure.
pub fn branch_pr_of(prs: &[PrInfo], branch: &str) -> Option<PrInfo> {
    if branch.is_empty() {
        return None;
    }
    let mut open: Option<&PrInfo> = None;
    let mut other: Option<&PrInfo> = None;
    for pr in prs {
        if pr.head_ref_name != branch {
            continue;
        }
        if pr.state == "OPEN" {
            open = Some(pr);
        }
        other = other.or(Some(pr));
    }
    open.or(other).cloned()
}

/// Canonical [`CheckInfo`] state of one REST check-run
/// (`repos/…/commits/<sha>/check-runs`): a not-completed run is pending; a
/// completed conclusion maps through the shared token table.
pub fn check_run_state(status: &str, conclusion: &str) -> &'static str {
    if status != "completed" || conclusion.is_empty() {
        return "pending";
    }
    check_state_from_state(conclusion)
}

/// Parse `gh api repos/:owner/:repo/commits/<sha>/check-runs` output
/// tolerantly (`{check_runs: [...]}`; a bare array also parses). Nameless
/// entries are skipped.
pub fn parse_commit_check_runs_json(payload: &str) -> Vec<CheckInfo> {
    let value: serde_json::Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let runs = match &value {
        serde_json::Value::Array(items) => items.as_slice(),
        _ => value
            .get("check_runs")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
    };
    runs.iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.trim().to_owned();
            if name.is_empty() {
                return None;
            }
            let status = item
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let conclusion = item
                .get("conclusion")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            Some(CheckInfo {
                name,
                state: check_run_state(status, conclusion).to_owned(),
            })
        })
        .collect()
}

/// Parse the REST `pulls/<n>/files` payload tolerantly.
pub fn parse_pr_files_json(payload: &str) -> Vec<ForgePrFile> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(payload) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| {
            let path = item.get("filename")?.as_str()?.trim().to_owned();
            if path.is_empty() {
                return None;
            }
            Some(ForgePrFile {
                path,
                status: item
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("modified")
                    .to_owned(),
                additions: item
                    .get("additions")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                deletions: item
                    .get("deletions")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                patch: item
                    .get("patch")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

/// One REST review comment (`pulls/<n>/comments`), ungrouped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawReviewComment {
    pub id: u64,
    pub in_reply_to_id: Option<u64>,
    pub path: String,
    pub line: Option<u64>,
    pub outdated: bool,
    pub author: String,
    pub body: String,
    pub created_at: Option<String>,
}

/// Parse the REST `pulls/<n>/comments` payload tolerantly.
pub fn parse_review_comments_json(payload: &str) -> Vec<RawReviewComment> {
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(payload) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_u64()?;
            Some(RawReviewComment {
                id,
                in_reply_to_id: item
                    .get("in_reply_to_id")
                    .and_then(serde_json::Value::as_u64),
                path: item
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                line: item.get("line").and_then(serde_json::Value::as_u64),
                outdated: item
                    .get("outdated")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                author: json_rest_user_login(&item),
                body: item
                    .get("body")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                created_at: json_option_string(&item, "created_at"),
            })
        })
        .collect()
}

/// Groups flat review comments into threads: top-level comments (no
/// `in_reply_to_id`) open threads in list order; replies attach to their
/// parent. A reply whose parent is missing (GitHub pagination) becomes its
/// own thread rather than being dropped. Pure, fixture-tested.
pub fn group_review_threads(comments: Vec<RawReviewComment>) -> Vec<ForgeReviewThread> {
    let mut threads: Vec<ForgeReviewThread> = Vec::new();
    let mut positions: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for comment in comments {
        let to_thread = |comment: &RawReviewComment| ForgeReviewThread {
            id: comment.id,
            path: comment.path.clone(),
            line: comment.line,
            outdated: comment.outdated,
            author: comment.author.clone(),
            body: comment.body.clone(),
            created_at: comment.created_at.clone(),
            replies: Vec::new(),
        };
        match comment.in_reply_to_id {
            None => {
                positions.insert(comment.id, threads.len());
                threads.push(to_thread(&comment));
            }
            Some(parent) => match positions.get(&parent).copied() {
                Some(index) => threads[index].replies.push(ForgeReviewComment {
                    id: comment.id,
                    author: comment.author,
                    body: comment.body,
                    created_at: comment.created_at,
                }),
                None => {
                    positions.insert(comment.id, threads.len());
                    threads.push(to_thread(&comment));
                }
            },
        }
    }
    threads
}

/// Degraded-state priority of an error kind (lower wins — the FE shows the
/// most fundamental problem first: no gh means nothing else can work).
fn error_priority(kind: &str) -> u8 {
    match kind {
        KIND_NO_GH => 0,
        KIND_NOT_AUTHED => 1,
        KIND_RATE_LIMITED => 2,
        KIND_OFFLINE => 3,
        _ => 4,
    }
}

/// Highest-priority error of a snapshot's failed parts (`None` when all
/// resolved). Pure, tested.
pub fn worst_part_error(errors: Vec<ForgeError>) -> Option<ForgeError> {
    errors.into_iter().min_by(|a, b| {
        error_priority(&a.kind)
            .cmp(&error_priority(&b.kind))
            .then_with(|| a.message.cmp(&b.message))
    })
}

/// Snapshot gh arguments: the branch's PR (explicit `--repo` slug so a
/// non-default remote resolves the right repository regardless of gh's own
/// remote detection).
pub fn snapshot_pr_args(slug: &str, branch: &str) -> Vec<String> {
    vec![
        "pr".to_string(),
        "list".to_string(),
        "--repo".to_string(),
        slug.to_string(),
        "--state".to_string(),
        "all".to_string(),
        "--head".to_string(),
        branch.to_string(),
        "--json".to_string(),
        PR_LIST_JSON_FIELDS.to_string(),
        "--limit".to_string(),
        "10".to_string(),
    ]
}

/// Fallback arguments of the branch-PR lookup for gh versions that reject
/// `--head`/`--repo` combinations: the unfiltered list, branch-filtered in
/// Rust by [`branch_pr_of`].
pub fn snapshot_pr_list_args(slug: &str) -> Vec<String> {
    vec![
        "pr".to_string(),
        "list".to_string(),
        "--repo".to_string(),
        slug.to_string(),
        "--state".to_string(),
        "all".to_string(),
        "--json".to_string(),
        PR_LIST_JSON_FIELDS.to_string(),
        "--limit".to_string(),
        "50".to_string(),
    ]
}

/// Snapshot gh arguments: the latest run of one branch.
pub fn snapshot_run_args(slug: &str, branch: &str) -> Vec<String> {
    vec![
        "run".to_string(),
        "list".to_string(),
        "--repo".to_string(),
        slug.to_string(),
        "--branch".to_string(),
        branch.to_string(),
        "--limit".to_string(),
        "1".to_string(),
        "--json".to_string(),
        RUN_LIST_JSON_FIELDS.to_string(),
    ]
}

/// Arguments of the commit-level check-runs read (REST via `gh api`).
pub fn commit_check_runs_args(slug: &str, sha: &str) -> Vec<String> {
    vec![
        "api".to_string(),
        format!("repos/{slug}/commits/{sha}/check-runs"),
    ]
}

/// Arguments of the PR-files read (REST via `gh api`).
pub fn pr_files_args(slug: &str, number: u64) -> Vec<String> {
    vec![
        "api".to_string(),
        format!("repos/{slug}/pulls/{number}/files"),
    ]
}

/// Arguments of the PR review-threads read (REST via `gh api`).
pub fn review_threads_args(slug: &str, number: u64) -> Vec<String> {
    vec![
        "api".to_string(),
        format!("repos/{slug}/pulls/{number}/comments"),
    ]
}

/// Is HEAD reachable from the remote-tracking branch? `merge_base(head,
/// remote) == head` answers it without a full walk. `None` = no tracking
/// ref (never pushed).
fn head_pushed(
    repo: &git2::Repository,
    remote_name: &str,
    branch: &str,
    head_oid: &git2::Oid,
) -> Option<bool> {
    let reference = repo
        .find_reference(&format!("refs/remotes/{remote_name}/{branch}"))
        .ok()?;
    let remote_oid = reference.target()?;
    if remote_oid == *head_oid {
        return Some(true);
    }
    match repo.merge_base(*head_oid, remote_oid) {
        Ok(base) => Some(base == *head_oid),
        // Unrelated histories: HEAD is certainly not on the remote.
        Err(_) => Some(false),
    }
}

/// One snapshot gh read classified into [`ForgeError`] (snapshot parts carry
/// structured kinds — the chips' degraded states).
fn run_gh_part(cwd: &Path, args: Vec<String>, timeout: Duration) -> Result<String, ForgeError> {
    let outcome = run_program(GH, Some(cwd), &args, timeout);
    if let Some(err) = &outcome.spawn_error {
        return Err(forge_failure(Some(err), ""));
    }
    if outcome.code == Some(0) && !outcome.timed_out {
        return Ok(outcome.stdout);
    }
    if outcome.timed_out {
        return Err(ForgeError::other(format!(
            "gh timed out after {} s and was killed",
            timeout.as_secs()
        )));
    }
    let combined = format!("{}\n{}", outcome.stderr, outcome.stdout);
    Err(forge_failure(None, &combined))
}

/// Snapshot part: the branch's PR. `Ok(None)` = no PR for the branch (a
/// normal state, not an error). Older gh versions that reject the
/// `--head`/`--repo` combination fall back to the unfiltered list.
pub fn snapshot_branch_pr_sync(
    cwd: &Path,
    slug: &str,
    branch: &str,
) -> Result<Option<PrInfo>, ForgeError> {
    let outcome = run_gh_part(cwd, snapshot_pr_args(slug, branch), LIST_TIMEOUT);
    match outcome {
        Ok(stdout) => Ok(branch_pr_of(&parse_pr_list_json(&stdout), branch)),
        Err(err) if err.kind == KIND_OTHER => {
            // Retry once with the unfiltered list before surfacing the error.
            let fallback = run_gh_part(cwd, snapshot_pr_list_args(slug), LIST_TIMEOUT);
            match fallback {
                Ok(stdout) => Ok(branch_pr_of(&parse_pr_list_json(&stdout), branch)),
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

/// Snapshot part: the latest Actions run of the branch (`None` = none).
pub fn snapshot_branch_run_sync(
    cwd: &Path,
    slug: &str,
    branch: &str,
) -> Result<Option<WorkflowRun>, ForgeError> {
    let stdout = run_gh_part(cwd, snapshot_run_args(slug, branch), RUNS_TIMEOUT)?;
    Ok(parse_workflow_runs_json(&stdout).into_iter().next())
}

/// Commit-level check-runs of one sha (the snapshot's HEAD/PR-head parts and
/// the standalone `forge_commit_checks` command).
pub fn commit_check_runs_sync(
    cwd: &Path,
    slug: &str,
    sha: &str,
) -> Result<Vec<CheckInfo>, ForgeError> {
    let stdout = run_gh_part(cwd, commit_check_runs_args(slug, sha), CHECKS_TIMEOUT)?;
    Ok(parse_commit_check_runs_json(&stdout))
}

/// Body of `forge_snapshot`: resolve the remote, read branch/HEAD locally,
/// then fan the GitHub reads out over at most 4 CONCURRENT gh calls (plan
/// §9.3). Per-repo `remote_override` selects a non-default remote.
pub fn forge_snapshot_sync(
    root: &Path,
    remote_override: Option<&str>,
) -> Result<ForgeSnapshot, ForgeError> {
    let resolved = resolve_remote(root, remote_override)?;
    let repo = git2::Repository::discover(root)
        .map_err(|e| ForgeError::other(format!("not a repository: {e}")))?;
    let head = repo.head().ok();
    let head_commit = head.as_ref().and_then(|h| h.peel_to_commit().ok());
    let head_sha = head_commit.as_ref().map(|c| c.id().to_string());
    let detached = repo.head_detached().unwrap_or(false);
    let branch: Option<String> = if detached {
        None
    } else {
        head.as_ref()
            .and_then(|h| h.shorthand().ok())
            .filter(|b| !b.is_empty() && *b != "HEAD")
            .map(str::to_owned)
    };
    let pushed = match (&head_commit, &branch) {
        (Some(commit), Some(branch_name)) => {
            head_pushed(&repo, &resolved.remote_name, branch_name, &commit.id())
        }
        _ => None,
    };
    let slug = format!("{}/{}", resolved.owner, resolved.repo);

    // Phase 1 — three independent reads in parallel (branch PR, HEAD checks,
    // latest run). Phase 2 — PR-head checks, which depend on phase 1's PR
    // head sha; concurrent calls never exceed 4 (§9.3).
    let (pr_part, head_checks_part, run_part) = std::thread::scope(|scope| {
        let pr = branch.clone().map(|branch_name| {
            let cwd = root.to_path_buf();
            let slug = slug.clone();
            scope.spawn(move || snapshot_branch_pr_sync(&cwd, &slug, &branch_name))
        });
        let head_checks = head_sha.clone().map(|sha| {
            let cwd = root.to_path_buf();
            let slug = slug.clone();
            scope.spawn(move || commit_check_runs_sync(&cwd, &slug, &sha))
        });
        let run = branch.clone().map(|branch_name| {
            let cwd = root.to_path_buf();
            let slug = slug.clone();
            scope.spawn(move || snapshot_branch_run_sync(&cwd, &slug, &branch_name))
        });
        let pr_part = pr.map(|handle| {
            handle
                .join()
                .unwrap_or_else(|_| Err(ForgeError::other("snapshot pr worker failed")))
        });
        let head_checks_part = head_checks.map(|handle| {
            handle
                .join()
                .unwrap_or_else(|_| Err(ForgeError::other("snapshot checks worker failed")))
        });
        let run_part = run.map(|handle| {
            handle
                .join()
                .unwrap_or_else(|_| Err(ForgeError::other("snapshot run worker failed")))
        });
        (pr_part, head_checks_part, run_part)
    });

    let mut errors: Vec<ForgeError> = Vec::new();
    let pr = match pr_part {
        Some(Ok(pr)) => pr,
        Some(Err(err)) => {
            errors.push(err);
            None
        }
        None => None,
    };
    let head_checks = match head_checks_part {
        Some(Ok(checks)) => checks,
        Some(Err(err)) => {
            errors.push(err);
            Vec::new()
        }
        None => Vec::new(),
    };
    let run = match run_part {
        Some(Ok(run)) => run,
        Some(Err(err)) => {
            errors.push(err);
            None
        }
        None => None,
    };

    // Phase 2 — PR-head checks, only when the PR head diverges from the
    // local HEAD (same sha ⇒ the head checks already cover the PR).
    let pr_head_sha = pr
        .as_ref()
        .map(|pr| pr.head_ref_oid.clone())
        .filter(|sha| !sha.is_empty());
    let pr_head_checks: Option<Vec<CheckInfo>> = match (&pr_head_sha, &head_sha) {
        (Some(pr_sha), Some(local_sha)) if pr_sha != local_sha => {
            match commit_check_runs_sync(root, &slug, pr_sha) {
                Ok(checks) => Some(checks),
                Err(err) => {
                    errors.push(err);
                    None
                }
            }
        }
        _ => None,
    };

    // Hashes before the struct literal moves the parts into it.
    let hashes = ForgeSnapshotHashes {
        pr: pr.as_ref().map_or(0, hash_serialized),
        head_checks: hash_serialized(&head_checks),
        pr_head_checks: pr_head_checks.as_ref().map_or(0, hash_serialized),
        run: run.as_ref().map_or(0, hash_serialized),
    };
    Ok(ForgeSnapshot {
        branch,
        head_sha,
        pushed,
        pr,
        head_checks,
        pr_head_sha,
        pr_head_checks,
        run,
        hashes,
        error: worst_part_error(errors),
    })
}

/// Body of `forge_commit_checks`: commit-level check-runs for an arbitrary
/// sha (E2's commit surfaces; also the snapshot's part reader).
pub fn forge_commit_checks_sync(root: &Path, sha: &str) -> Result<Vec<CheckInfo>, String> {
    let resolved = resolve_remote(root, None).map_err(|e| e.to_string())?;
    let slug = format!("{}/{}", resolved.owner, resolved.repo);
    commit_check_runs_sync(root, &slug, sha.trim()).map_err(|e| e.to_string())
}

/// Body of `forge_pr_files`: the PR's changed files with their patches
/// (E2's PR diff view).
pub fn forge_pr_files_sync(root: &Path, number: u64) -> Result<Vec<ForgePrFile>, String> {
    let resolved = resolve_remote(root, None).map_err(|e| e.to_string())?;
    let slug = format!("{}/{}", resolved.owner, resolved.repo);
    let args = pr_files_args(&slug, number);
    let outcome = run_program(GH, Some(root), &args, LIST_TIMEOUT);
    if let Some(err) = &outcome.spawn_error {
        return Err(format!("gh CLI not found on PATH ({err})"));
    }
    if outcome.code == Some(0) && !outcome.timed_out {
        return Ok(parse_pr_files_json(&outcome.stdout));
    }
    let combined = format!("{}\n{}", outcome.stderr, outcome.stdout);
    Err(trim_message(&combined))
}

/// Body of `forge_pr_review_threads`: the PR's review threads, read-only
/// (E2's review-thread panel).
pub fn forge_pr_review_threads_sync(
    root: &Path,
    number: u64,
) -> Result<Vec<ForgeReviewThread>, String> {
    let resolved = resolve_remote(root, None).map_err(|e| e.to_string())?;
    let slug = format!("{}/{}", resolved.owner, resolved.repo);
    let args = review_threads_args(&slug, number);
    let outcome = run_program(GH, Some(root), &args, LIST_TIMEOUT);
    if let Some(err) = &outcome.spawn_error {
        return Err(format!("gh CLI not found on PATH ({err})"));
    }
    if outcome.code == Some(0) && !outcome.timed_out {
        return Ok(group_review_threads(parse_review_comments_json(
            &outcome.stdout,
        )));
    }
    let combined = format!("{}\n{}", outcome.stderr, outcome.stdout);
    Err(trim_message(&combined))
}
