use tauri::State;

use crate::engine::stats::{Contributor, DayCount};
use crate::engine::types::RepoId;
use crate::forge::{
    self, CheckInfo, ForgeContext, ForgeError, ForgeStatus, PrCreated, PrInfo, WorkflowInfo,
    WorkflowInput, WorkflowRun, WorkflowRunDetail,
};
use crate::repo::RepoManager;
use crate::service::{enqueue_read, enqueue_remote_read, finish_op, get_handle};

// ---------------------------------------------------------------------------
// M6 (lane H2): forge — gh-assisted GitHub flow (contracts.md "Commands (M6
// forge)"). All GitHub traffic shells out to the user's `gh` CLI (JSON mode)
// with the repo workdir as cwd; mygitui stores no GitHub tokens. The logic
// lives in `forge.rs` (process runner with hard timeouts + pure parsers,
// fixture-tested in `forge_tests.rs`); these are thin wrappers. None of the
// commands join the op queue — `pr_create` is a spec'd long op (120 s
// timeout inside the runner, kill on exceed) and the rest are
// `spawn_blocking` reads, following the actions.rs pattern minus the event
// stream.
// ---------------------------------------------------------------------------

/// Detect `gh` on PATH and whether the user is authenticated (no repo).
#[tauri::command(rename_all = "snake_case")]
pub async fn forge_status() -> ForgeStatus {
    tauri::async_runtime::spawn_blocking(forge::status_sync)
        .await
        .unwrap_or_default()
}

/// Owner/repo/branch context of one open repo (derived from the `origin`
/// remote + HEAD; errors when the remote is not GitHub or HEAD is detached).
#[tauri::command(rename_all = "snake_case")]
pub async fn forge_context(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<ForgeContext, String> {
    let root = get_handle(&state, &repo_id)?.root.clone();
    tauri::async_runtime::spawn_blocking(move || forge::context_sync(&root))
        .await
        .map_err(|e| format!("forge_context task failed: {e}"))?
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_create_sync(&root, &base, &title, &body, draft)
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
    let root = get_handle(&state, &repo_id)?.root.clone();
    let filter = state_filter.unwrap_or_else(|| "open".to_string());
    tauri::async_runtime::spawn_blocking(move || forge::pr_list_sync(&root, &filter))
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
    let root = get_handle(&state, &repo_id)?.root.clone();
    tauri::async_runtime::spawn_blocking(move || forge::pr_checks_sync(&root, number))
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_merge_sync(&root, number, &method, delete_branch, auto)
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_close_sync(&root, number, comment.as_deref())
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_reopen_sync(&root, number, comment.as_deref())
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || forge::pr_ready_sync(&root, number, ready))
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || forge::pr_checkout_sync(&root, number))
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || forge::pr_comment_sync(&root, number, &body))
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        forge::pr_review_sync(&root, number, &action, &body)
    })
    .await
    .map_err(|e| ForgeError::other(format!("pr_review task failed: {e}")))?
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
    let root = get_handle(&state, &repo_id)?.root.clone();
    let limit = limit.unwrap_or(forge::RUN_LIST_LIMIT);
    let branch = branch.unwrap_or_default();
    let status = status_filter.unwrap_or_default();
    let workflow = workflow.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        forge::workflow_runs_sync(&root, limit, &branch, &status, &workflow)
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
    let root = get_handle(&state, &repo_id)?.root.clone();
    tauri::async_runtime::spawn_blocking(move || forge::workflow_run_detail_sync(&root, run_id))
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
    let root = get_handle(&state, &repo_id)?.root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        forge::workflow_run_logs_sync(&root, run_id, job_id, failed_only)
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || forge::workflow_run_cancel_sync(&root, run_id))
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        forge::workflow_run_rerun_sync(&root, run_id, failed_only)
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
    let root = get_handle(&state, &repo_id)
        .map_err(ForgeError::other)?
        .root
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        forge::workflow_dispatch_sync(&root, &workflow, &git_ref, &inputs)
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
    let root = get_handle(&state, &repo_id)?.root.clone();
    tauri::async_runtime::spawn_blocking(move || forge::workflow_list_sync(&root))
        .await
        .map_err(|e| format!("workflow_list task failed: {e}"))?
}

/// Detected project types + runnable scripts under one repository (bounded
/// root + immediate-child scan; see `projects.rs`).
#[tauri::command(rename_all = "snake_case")]
pub async fn project_detect(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<crate::projects::ProjectInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // A local fs scan cannot see the device's POSIX paths; snapshot the
        // tree over SSH and run the same detectors on it.
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "projects",
            move |git, _ws| crate::remote::projects::detect(&git.session, &git.root),
        ))
        .await;
    }
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || crate::projects::detect_sync(&root))
        .await
        .map_err(|e| format!("project_detect task failed: {e}"))
}

// ---------------------------------------------------------------------------
// M7 (lane I1): contribution statistics — commit activity heatmap buckets +
// contributor rollups (contracts.md "Commands (M7 stats)"). Queued reads on
// the op queue (`enqueue_read`, no generation bump); the logic lives in
// `engine/stats.rs` as inherent `*_impl` twins on `Libgit2Engine`, reached
// through the `GitEngineM12` forwarding (trash.rs hosts the single impl
// block).
// ---------------------------------------------------------------------------

/// Commits per local day inside the `max_days` window, day ascending.
/// `author` (when set) is a case-insensitive substring matched against the
/// author name OR email; blank/None = no filter.
#[tauri::command(rename_all = "snake_case")]
pub async fn commit_activity(
    repo_id: RepoId,
    max_days: u32,
    author: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<Vec<DayCount>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "stats",
            move |git, _ws| git.commit_activity(max_days, author.as_deref()),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "stats", move |_ctx, repo| {
        engine.commit_activity(repo, max_days, author.as_deref())
    }))
    .await
}

/// Per-author rollups (identity = exact name+email pair) inside the
/// `max_days` window, count descending.
#[tauri::command(rename_all = "snake_case")]
pub async fn contributor_stats(
    repo_id: RepoId,
    max_days: u32,
    state: State<'_, RepoManager>,
) -> Result<Vec<Contributor>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "stats",
            move |git, _ws| git.contributor_stats(max_days),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "stats", move |_ctx, repo| {
        engine.contributor_stats(repo, max_days)
    }))
    .await
}
