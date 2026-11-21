//! Forge commands: gh-assisted GitHub flow (contracts.md "Commands (M6
//! forge)"). All GitHub traffic shells out to the user's `gh` CLI (JSON mode)
//! with the repo workdir as cwd; mygitui stores no GitHub tokens. The logic
//! lives in `forge.rs` (process runner with hard timeouts + pure parsers,
//! fixture-tested in `forge_tests.rs`); these are thin wrappers. None of the
//! commands join the op queue — `pr_create` is a spec'd long op (120 s
//! timeout inside the runner, kill on exceed) and the rest are
//! `spawn_blocking` reads, following the actions.rs pattern minus the event
//! stream.

use tauri::State;

use std::sync::Arc;

use crate::engine::types::RepoId;
use crate::forge::{
    self, CheckInfo, ForgeContext, ForgeError, ForgeRepo, ForgeSnapshot, ForgeStatus, PrCreated,
    PrInfo, WorkflowInfo, WorkflowInput, WorkflowRun, WorkflowRunDetail,
};
use crate::repo::{RepoHandle, RepoManager};

use super::{enqueue_remote_read, finish_op, get_handle};

/// The forge execution context of one open repo: local repos run gh with
/// the workdir as cwd; device repos run gh and git ON THE DEVICE (the
/// device's own gh config and auth) through [`ForgeRepo`].
fn forge_repo_of(handle: &Arc<RepoHandle>) -> ForgeRepo {
    ForgeRepo {
        root: handle.root.clone(),
        remote: handle.remote().map(Arc::clone),
    }
}

/// Detect `gh` and whether the user is authenticated. Without `repo_id`
/// (and for local repos) the probe runs on this machine; a repo on a device
/// probes the DEVICE's gh (`gh --version` + `gh auth status` over SSH) —
/// remote GitHub features work with the device's own auth. A panicking
/// `spawn_blocking` task resolves as a real error (the FE's catch logs it
/// and falls back to "unavailable") instead of the old
/// `unwrap_or_default()`'s misleading `available: false`.
#[tauri::command(rename_all = "snake_case")]
pub async fn forge_status(
    repo_id: Option<RepoId>,
    state: State<'_, RepoManager>,
) -> Result<ForgeStatus, String> {
    let remote = repo_id
        .and_then(|id| get_handle(&state, &id).ok())
        .and_then(|handle| handle.remote().map(Arc::clone));
    tauri::async_runtime::spawn_blocking(move || match remote {
        Some(device) => Ok(forge::status_sync_remote(&device.session, &device.root)),
        None => Ok(forge::status_sync()),
    })
    .await
    .map_err(|e| format!("forge_status task failed: {e}"))?
}

/// Hosts `gh auth status` reports (manual refresh of the E1 host cache;
/// `forge_status` refreshes it on auth-state change). Always includes
/// `github.com`.
#[tauri::command(rename_all = "snake_case")]
pub async fn forge_hosts() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(forge::hosts_sync)
        .await
        .map_err(|e| format!("forge_hosts task failed: {e}"))
}

/// Owner/repo/branch context of one open repo (derived from the configured
/// remote — default `origin`, `remote` selects the per-repo override — plus
/// HEAD; errors with a structured `ForgeError` when the remote is missing or
/// not GitHub, or HEAD is detached).
#[tauri::command(rename_all = "snake_case")]
pub async fn forge_context(
    repo_id: RepoId,
    remote: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<ForgeContext, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || forge::context_sync(&repo, remote.as_deref()))
        .await
        .map_err(|e| ForgeError::other(format!("forge_context task failed: {e}")))?
}

/// One repo's batched GitHub snapshot (E1): branch PR, HEAD check-runs,
/// PR-head check-runs (when the PR head diverges), latest branch run —
/// resolved with at most 4 concurrent gh calls and content hashes so the FE
/// skips re-renders of unchanged parts. `remote` selects the per-repo
/// override remote (default `origin`). Resolution-level failures (non-GitHub
/// remote, gh missing) reject with a structured `ForgeError`; per-part
/// failures ride inside the snapshot (`error` + missing parts).
#[tauri::command(rename_all = "snake_case")]
pub async fn forge_snapshot(
    repo_id: RepoId,
    remote: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<ForgeSnapshot, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::forge_snapshot_sync(&repo, remote.as_deref())
    })
    .await
    .map_err(|e| ForgeError::other(format!("forge_snapshot task failed: {e}")))?
}

/// Commit-level check-runs for an arbitrary sha (E2 commit surfaces).
#[tauri::command(rename_all = "snake_case")]
pub async fn forge_commit_checks(
    repo_id: RepoId,
    sha: String,
    state: State<'_, RepoManager>,
) -> Result<Vec<CheckInfo>, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    tauri::async_runtime::spawn_blocking(move || forge::forge_commit_checks_sync(&repo, &sha))
        .await
        .map_err(|e| format!("forge_commit_checks task failed: {e}"))?
}

/// The PR's changed files with their patches (E2's PR diff view).
#[tauri::command(rename_all = "snake_case")]
pub async fn forge_pr_files(
    repo_id: RepoId,
    number: u64,
    state: State<'_, RepoManager>,
) -> Result<Vec<forge::ForgePrFile>, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    tauri::async_runtime::spawn_blocking(move || forge::forge_pr_files_sync(&repo, number))
        .await
        .map_err(|e| format!("forge_pr_files task failed: {e}"))?
}

/// The PR's review threads, read-only (E2's review-thread panel).
#[tauri::command(rename_all = "snake_case")]
pub async fn forge_pr_review_threads(
    repo_id: RepoId,
    number: u64,
    state: State<'_, RepoManager>,
) -> Result<Vec<forge::ForgeReviewThread>, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    tauri::async_runtime::spawn_blocking(move || forge::forge_pr_review_threads_sync(&repo, number))
        .await
        .map_err(|e| format!("forge_pr_review_threads task failed: {e}"))?
}

/// Create a PR via `gh pr create` (base, title, body via temp file, draft).
/// Rejection is the structured [`ForgeError`]; `AlreadyExists` carries the
/// existing PR's URL when one is parseable from gh's output.
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_create(
    repo_id: RepoId,
    base: String,
    title: String,
    body: String,
    draft: bool,
    state: State<'_, RepoManager>,
) -> Result<PrCreated, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_create_sync(&repo, &base, &title, &body, draft)
    })
    .await
    .map_err(|e| ForgeError::other(format!("pr_create task failed: {e}")))?
}

/// PRs of the repo (`gh pr list --state <state> --json … --limit 50`).
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_list(
    repo_id: RepoId,
    state_filter: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<Vec<PrInfo>, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    let filter = state_filter.unwrap_or_else(|| "open".to_string());
    tauri::async_runtime::spawn_blocking(move || forge::pr_list_sync(&repo, &filter))
        .await
        .map_err(|e| format!("pr_list task failed: {e}"))?
}

/// CI checks of one PR (`gh pr checks <n> --json name,state,bucket`, table
/// fallback for older gh); states are canonicalized to
/// pass/fail/pending/skipping.
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_checks(
    repo_id: RepoId,
    number: u64,
    state: State<'_, RepoManager>,
) -> Result<Vec<CheckInfo>, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    tauri::async_runtime::spawn_blocking(move || forge::pr_checks_sync(&repo, number))
        .await
        .map_err(|e| format!("pr_checks task failed: {e}"))?
}

/// Merge a PR (`merge`/`squash`/`rebase`), optionally deleting the branch or
/// enabling auto-merge.
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_merge(
    repo_id: RepoId,
    number: u64,
    method: String,
    delete_branch: bool,
    auto: bool,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_merge_sync(&repo, number, &method, delete_branch, auto)
    })
    .await
    .map_err(|e| ForgeError::other(format!("pr_merge task failed: {e}")))?
}

/// Close a PR with an optional comment.
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_close(
    repo_id: RepoId,
    number: u64,
    comment: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_close_sync(&repo, number, comment.as_deref())
    })
    .await
    .map_err(|e| ForgeError::other(format!("pr_close task failed: {e}")))?
}

/// Reopen a closed PR with an optional comment.
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_reopen(
    repo_id: RepoId,
    number: u64,
    comment: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_reopen_sync(&repo, number, comment.as_deref())
    })
    .await
    .map_err(|e| ForgeError::other(format!("pr_reopen task failed: {e}")))?
}

/// Mark a PR ready for review (`ready = true`) or convert it back to draft.
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_ready(
    repo_id: RepoId,
    number: u64,
    ready: bool,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || forge::pr_ready_sync(&repo, number, ready))
        .await
        .map_err(|e| ForgeError::other(format!("pr_ready task failed: {e}")))?
}

/// Check out a PR's head branch into the working tree (`gh pr checkout`).
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_checkout(
    repo_id: RepoId,
    number: u64,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || forge::pr_checkout_sync(&repo, number))
        .await
        .map_err(|e| ForgeError::other(format!("pr_checkout task failed: {e}")))?
}

/// Post a comment on a PR.
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_comment(
    repo_id: RepoId,
    number: u64,
    body: String,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || forge::pr_comment_sync(&repo, number, &body))
        .await
        .map_err(|e| ForgeError::other(format!("pr_comment task failed: {e}")))?
}

/// Submit a PR review: `approve`, `request-changes`, or `comment`.
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_review(
    repo_id: RepoId,
    number: u64,
    action: String,
    body: String,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_review_sync(&repo, number, &action, &body)
    })
    .await
    .map_err(|e| ForgeError::other(format!("pr_review task failed: {e}")))?
}

/// Edit a PR's title and/or body (`gh pr edit`; at least one must be given).
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_edit(
    repo_id: RepoId,
    number: u64,
    title: Option<String>,
    body: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_edit_sync(&repo, number, title.as_deref(), body.as_deref())
    })
    .await
    .map_err(|e| ForgeError::other(format!("pr_edit task failed: {e}")))?
}

/// The repo's pull-request template (first of
/// [`forge::PR_TEMPLATE_PATHS`] present under the workdir; `None` when the
/// repo has none). Remote repos read through the workspace file channel.
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_template(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Option<String>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "pr_template",
            move |_git, ws| {
                Ok(forge::pick_pr_template(|rel| {
                    if ws.exists(rel).unwrap_or(false) {
                        ws.read_file(rel)
                            .ok()
                            .and_then(|bytes| String::from_utf8(bytes).ok())
                    } else {
                        None
                    }
                }))
            },
        ))
        .await;
    }
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || Ok(forge::pr_template_sync(&root)))
        .await
        .map_err(|e| format!("pr_template task failed: {e}"))?
}

// -- Issues ------------------------------------------------------------------

/// Issues of the repo (`gh issue list --state <state> --json … --limit 50`).
#[tauri::command(rename_all = "snake_case")]
pub async fn issue_list(
    repo_id: RepoId,
    state_filter: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<Vec<forge::ForgeIssue>, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    let filter = state_filter.unwrap_or_else(|| "open".to_string());
    tauri::async_runtime::spawn_blocking(move || forge::issue_list_sync(&repo, &filter))
        .await
        .map_err(|e| format!("issue_list task failed: {e}"))?
}

/// One issue with its body and comments (`gh issue view <n> --json …`).
#[tauri::command(rename_all = "snake_case")]
pub async fn issue_view(
    repo_id: RepoId,
    number: u64,
    state: State<'_, RepoManager>,
) -> Result<forge::ForgeIssueDetail, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    tauri::async_runtime::spawn_blocking(move || forge::issue_view_sync(&repo, number))
        .await
        .map_err(|e| format!("issue_view task failed: {e}"))?
}

/// Create an issue (`gh issue create --title <t> [--body-file <tmp>]`; the
/// body is optional). Rejection is the structured [`ForgeError`].
#[tauri::command(rename_all = "snake_case")]
pub async fn issue_create(
    repo_id: RepoId,
    title: String,
    body: String,
    state: State<'_, RepoManager>,
) -> Result<forge::IssueCreated, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || forge::issue_create_sync(&repo, &title, &body))
        .await
        .map_err(|e| ForgeError::other(format!("issue_create task failed: {e}")))?
}

/// Close an issue with an optional comment.
#[tauri::command(rename_all = "snake_case")]
pub async fn issue_close(
    repo_id: RepoId,
    number: u64,
    comment: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::issue_close_sync(&repo, number, comment.as_deref())
    })
    .await
    .map_err(|e| ForgeError::other(format!("issue_close task failed: {e}")))?
}

/// Reopen a closed issue with an optional comment.
#[tauri::command(rename_all = "snake_case")]
pub async fn issue_reopen(
    repo_id: RepoId,
    number: u64,
    comment: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::issue_reopen_sync(&repo, number, comment.as_deref())
    })
    .await
    .map_err(|e| ForgeError::other(format!("issue_reopen task failed: {e}")))?
}

// -- Notifications -----------------------------------------------------------

/// The user's unread notification threads (`gh api notifications`; GitHub's
/// default listing is the unread set).
#[tauri::command(rename_all = "snake_case")]
pub async fn notifications_list(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<forge::ForgeNotification>, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    tauri::async_runtime::spawn_blocking(move || forge::notifications_list_sync(&repo))
        .await
        .map_err(|e| format!("notifications_list task failed: {e}"))?
}

/// Mark one notification thread read (`gh api threads/<id> -X PATCH`).
#[tauri::command(rename_all = "snake_case")]
pub async fn notification_mark_read(
    repo_id: RepoId,
    id: u64,
    state: State<'_, RepoManager>,
) -> Result<(), ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || forge::notification_mark_read_sync(&repo, id))
        .await
        .map_err(|e| ForgeError::other(format!("notification_mark_read task failed: {e}")))?
}

/// Mark every notification thread read (`gh api notifications -X PUT`).
#[tauri::command(rename_all = "snake_case")]
pub async fn notifications_mark_all_read(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<(), ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || forge::notifications_mark_all_read_sync(&repo))
        .await
        .map_err(|e| ForgeError::other(format!("notifications_mark_all_read task failed: {e}")))?
}

// -- GitHub Actions ---------------------------------------------------------

/// Recent workflow runs (`gh run list`), optionally filtered by branch,
/// status, and workflow.
#[tauri::command(rename_all = "snake_case")]
pub async fn workflow_runs(
    repo_id: RepoId,
    limit: Option<u32>,
    branch: Option<String>,
    status_filter: Option<String>,
    workflow: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<Vec<WorkflowRun>, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    let limit = limit.unwrap_or(forge::RUN_LIST_LIMIT);
    let branch = branch.unwrap_or_default();
    let status = status_filter.unwrap_or_default();
    let workflow = workflow.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        forge::workflow_runs_sync(&repo, limit, &branch, &status, &workflow)
    })
    .await
    .map_err(|e| format!("workflow_runs task failed: {e}"))?
}

/// One workflow run with its jobs and steps (`gh run view --json …jobs…`).
#[tauri::command(rename_all = "snake_case")]
pub async fn workflow_run_detail(
    repo_id: RepoId,
    run_id: u64,
    state: State<'_, RepoManager>,
) -> Result<WorkflowRunDetail, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    tauri::async_runtime::spawn_blocking(move || forge::workflow_run_detail_sync(&repo, run_id))
        .await
        .map_err(|e| format!("workflow_run_detail task failed: {e}"))?
}

/// Logs of a whole run or one job (`gh run view <id> [--job <id>] --log`);
/// `failed_only` requests only failed steps (`--log-failed`).
#[tauri::command(rename_all = "snake_case")]
pub async fn workflow_run_logs(
    repo_id: RepoId,
    run_id: u64,
    job_id: Option<u64>,
    failed_only: bool,
    state: State<'_, RepoManager>,
) -> Result<String, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::workflow_run_logs_sync(&repo, run_id, job_id, failed_only)
    })
    .await
    .map_err(|e| format!("workflow_run_logs task failed: {e}"))?
}

/// Cancel a queued or running workflow run (`gh run cancel`).
#[tauri::command(rename_all = "snake_case")]
pub async fn workflow_run_cancel(
    repo_id: RepoId,
    run_id: u64,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || forge::workflow_run_cancel_sync(&repo, run_id))
        .await
        .map_err(|e| ForgeError::other(format!("workflow_run_cancel task failed: {e}")))?
}

/// Rerun a completed run, or only its failed jobs (`--failed`).
#[tauri::command(rename_all = "snake_case")]
pub async fn workflow_run_rerun(
    repo_id: RepoId,
    run_id: u64,
    failed_only: bool,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::workflow_run_rerun_sync(&repo, run_id, failed_only)
    })
    .await
    .map_err(|e| ForgeError::other(format!("workflow_run_rerun task failed: {e}")))?
}

/// Trigger a `workflow_dispatch` run (`gh workflow run <workflow> --ref …`).
#[tauri::command(rename_all = "snake_case")]
pub async fn workflow_dispatch(
    repo_id: RepoId,
    workflow: String,
    git_ref: String,
    inputs: Vec<WorkflowInput>,
    state: State<'_, RepoManager>,
) -> Result<String, ForgeError> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id).map_err(ForgeError::other)?);
    tauri::async_runtime::spawn_blocking(move || {
        forge::workflow_dispatch_sync(&repo, &workflow, &git_ref, &inputs)
    })
    .await
    .map_err(|e| ForgeError::other(format!("workflow_dispatch task failed: {e}")))?
}

/// Workflow files of the repo, including disabled ones
/// (`gh workflow list --all --json id,name,path,state`).
#[tauri::command(rename_all = "snake_case")]
pub async fn workflow_list(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<WorkflowInfo>, String> {
    let repo = forge_repo_of(&get_handle(&state, &repo_id)?);
    tauri::async_runtime::spawn_blocking(move || forge::workflow_list_sync(&repo))
        .await
        .map_err(|e| format!("workflow_list task failed: {e}"))?
}
