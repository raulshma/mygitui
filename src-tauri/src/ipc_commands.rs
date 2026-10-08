//! M1 IPC command surface (names/shapes fixed by docs/contracts.md).
//!
//! Engine calls run on the blocking pool (`tauri::async_runtime`) because
//! git2 is synchronous; `EngineError`s surface as strings. The two stream
//! commands push pages over `tauri::ipc::Channel`s: diff pages of
//! [`DIFF_PAGE_SIZE`] files, log pages of [`LOG_PAGE_SIZE`] commits laid out
//! by the graph module. Streams stop early when the channel send fails
//! (client gone) or the stream was superseded/cancelled; log streams abort
//! when the repo generation moves (the client re-requests).

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use crate::engine::git_engine::EngineError;
use crate::engine::types::{
    BlameLine, CheckpointInfo, CommitInfo, ConflictFile, ConflictResolution, DiffSide, FileDiff,
    LogFilter, MergeResult, RebaseState, RebaseStep, ReflogEntry, RepoId, RepoInfo, RepoStatus,
    ResetKind, StashInfo, WorktreeInfo,
};
use crate::graph::types::{self, GraphRow};
use crate::repo::{RepoHandle, RepoManager, StreamHandle};

/// Commits per `repo_log_stream` / `repo_file_history` page.
const LOG_PAGE_SIZE: usize = 500;
/// Files per `repo_diff_stream` page.
const DIFF_PAGE_SIZE: usize = 50;

/// One page of streamed history (contracts.md: `LogPage`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogPage {
    pub commits: Vec<CommitInfo>,
    pub rows: Vec<GraphRow>,
    pub next_cursor: Option<String>,
    pub generation: u64,
}

fn get_handle(state: &RepoManager, repo_id: &RepoId) -> Result<Arc<RepoHandle>, String> {
    state
        .get(repo_id)
        .ok_or_else(|| format!("repo not open: {}", repo_id.0))
}

fn engine_err(err: EngineError) -> String {
    err.to_string()
}

/// Split `items` into consecutive chunks of at most `size` (the last may be
/// smaller); empty input yields no pages.
pub fn chunk_vec<T>(items: Vec<T>, size: usize) -> Vec<Vec<T>> {
    assert!(size > 0, "chunk size must be positive");
    let mut pages: Vec<Vec<T>> = Vec::new();
    for item in items {
        match pages.last_mut() {
            Some(page) if page.len() < size => page.push(item),
            _ => pages.push(vec![item]),
        }
    }
    pages
}

/// Open (or reuse) the repository at/above `path` and start its watcher.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_open(
    path: String,
    app: tauri::AppHandle,
    state: State<'_, RepoManager>,
) -> Result<RepoInfo, String> {
    state.open(&app, Path::new(&path)).map_err(engine_err)
}

/// Close a repository: stop its watcher, cancel its streams, drop caches.
#[tauri::command(rename_all = "snake_case")]
pub fn repo_close(repo_id: RepoId, state: State<'_, RepoManager>) {
    state.close(&repo_id);
}

#[tauri::command(rename_all = "snake_case")]
pub async fn repo_status(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<RepoStatus, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.status(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("status task failed: {e}"))?
}

/// Local + remote branches and tags: `(ref name, target sha)` pairs.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_refs(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<(String, String)>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.refs(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("refs task failed: {e}"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn repo_blame(
    repo_id: RepoId,
    path: String,
    from: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<Vec<BlameLine>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine
            .blame(&repo, &path, from.as_deref())
            .map_err(engine_err)
    })
    .await
    .map_err(|e| format!("blame task failed: {e}"))?
}

/// One-shot diff for small/medium changes; large diffs use
/// [`repo_diff_stream`].
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_diff(
    repo_id: RepoId,
    old: DiffSide,
    new: DiffSide,
    paths: Option<Vec<String>>,
    state: State<'_, RepoManager>,
) -> Result<Vec<FileDiff>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine
            .diff(&repo, &old, &new, paths.as_deref())
            .map_err(engine_err)
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))?
}

/// Stream a diff in pages of [`DIFF_PAGE_SIZE`] files over `on_page`.
/// Stops early when the channel send fails (client gone).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_diff_stream(
    repo_id: RepoId,
    old: DiffSide,
    new: DiffSide,
    paths: Option<Vec<String>>,
    on_page: Channel<Vec<FileDiff>>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    let repo = handle.repo();
    let diffs = tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine
            .diff(&repo, &old, &new, paths.as_deref())
            .map_err(engine_err)
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))??;
    for page in chunk_vec(diffs, DIFF_PAGE_SIZE) {
        if on_page.send(page).is_err() {
            break; // client dropped the channel
        }
    }
    Ok(())
}

/// Stream the commit log in [`LOG_PAGE_SIZE`] pages over `on_page`. A new
/// call for the same repo replaces (cancels) the previous stream.
#[tauri::command(rename_all = "snake_case")]
pub fn repo_log_stream(
    repo_id: RepoId,
    filter: LogFilter,
    on_page: Channel<LogPage>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let token = handle.begin_log_stream();
    std::thread::Builder::new()
        .name(format!("log-stream-{}", repo_id.0))
        .spawn(move || stream_log(handle, filter, on_page, token, false))
        .map_err(|e| format!("failed to spawn log stream: {e}"))?;
    Ok(())
}

/// Stream one file's history (follows renames) as [`LogPage`]s over `on_page`.
#[tauri::command(rename_all = "snake_case")]
pub fn repo_file_history(
    repo_id: RepoId,
    path: String,
    on_page: Channel<LogPage>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let filter = LogFilter {
        path: Some(path),
        follow: true,
        ..LogFilter::default()
    };
    let token = handle.begin_history_stream();
    std::thread::Builder::new()
        .name(format!("history-stream-{}", repo_id.0))
        .spawn(move || stream_log(handle, filter, on_page, token, true))
        .map_err(|e| format!("failed to spawn history stream: {e}"))?;
    Ok(())
}

/// Shared log/file-history streaming loop. Ends when the walk is exhausted,
/// the stream is cancelled/superseded, the client drops the channel, or the
/// repo generation moved (stale; the client re-requests).
fn stream_log(
    handle: Arc<RepoHandle>,
    filter: LogFilter,
    on_page: Channel<LogPage>,
    token: Arc<StreamHandle>,
    history: bool,
) {
    let start_generation = handle.generation();
    let mut cursor: Option<String> = None;
    // File history is an independent view with its own lanes; the main log
    // continues the repo's shared LaneState across pages.
    let mut local_lanes = history.then(types::LaneState::default);

    loop {
        if token.is_cancelled() || handle.is_closed() {
            break;
        }
        if handle.generation() != start_generation {
            break; // repo changed under us: abort, client re-requests
        }

        let page = {
            let engine = handle.engine();
            let repo = handle.repo();
            let repo = repo.lock();
            match engine.log(&repo, &filter, LOG_PAGE_SIZE, cursor.as_deref()) {
                Ok(page) => page,
                Err(err) => {
                    tracing::warn!(
                        repo = %handle.id.0,
                        error = %err,
                        "log stream: engine error, aborting"
                    );
                    break;
                }
            }
        };
        let (commits, next_cursor) = page;

        let rows = match local_lanes.as_mut() {
            Some(lanes) => types::layout_page(&commits, lanes),
            None => {
                let mut lanes = handle.lanes.lock();
                types::layout_page(&commits, &mut lanes)
            }
        };

        let done = next_cursor.is_none();
        if on_page
            .send(LogPage {
                commits,
                rows,
                next_cursor: next_cursor.clone(),
                generation: start_generation,
            })
            .is_err()
        {
            break; // client dropped the channel
        }
        if done {
            break;
        }
        cursor = next_cursor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_vec_splits_into_pages() {
        let items: Vec<u32> = (0..120).collect();
        let pages = chunk_vec(items, 50);
        assert_eq!(pages.len(), 3);
        assert_eq!(
            [pages[0].len(), pages[1].len(), pages[2].len()],
            [50, 50, 20]
        );
        assert_eq!(pages[2][0], 100);
        let flat: Vec<u32> = pages.into_iter().flatten().collect();
        assert_eq!(flat, (0..120).collect::<Vec<u32>>());
    }

    #[test]
    fn chunk_vec_exact_multiple_and_empty() {
        assert!(chunk_vec(Vec::<u8>::new(), 50).is_empty());
        let pages = chunk_vec(vec![1u8; 50], 50);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].len(), 50);
    }
}

// ---------------------------------------------------------------------------
// Clone (B4 lane, appended): `repo_clone` + `clone-progress` events.
//
// Clones a remote URL into a local destination with git2's `RepoBuilder`
// (fetch options + transfer-progress callback), emitting throttled
// `clone-progress` events `{url, received, total, objects}` over the app
// emitter. `received` counts bytes; `total`/`objects` count git objects —
// libgit2's transfer progress has no total-bytes estimate, so the FE bar
// fills by objects. Pure helpers (`clone_emit_ready`,
// `clone_progress_event`, `validate_clone_destination`) are unit-tested
// without touching the network.
// ---------------------------------------------------------------------------

use parking_lot::Mutex;
use tauri::Emitter;

/// `clone-progress` event payload (mirrors the FE `CloneProgressEvent`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CloneProgress {
    /// URL of the clone this progress belongs to (multi-clone discriminator).
    pub url: String,
    /// Bytes received so far.
    pub received: usize,
    /// Total objects the remote announced (0 = not yet known).
    pub total: usize,
    /// Objects received so far.
    pub objects: usize,
}

/// Minimum interval between two `clone-progress` events; the libgit2
/// callback fires per network chunk and would flood the IPC bridge.
const CLONE_EMIT_INTERVAL_MS: u64 = 80;

/// Throttle decision: emit when the interval elapsed or the transfer is
/// done (so the final update always lands).
fn clone_emit_ready(elapsed_ms: u64, done: bool) -> bool {
    done || elapsed_ms >= CLONE_EMIT_INTERVAL_MS
}

/// Owned snapshot of `git2::Progress` (its lifetime is bound to the callback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloneStats {
    pub received_bytes: usize,
    pub received_objects: usize,
    pub total_objects: usize,
}

impl<'a> From<&git2::Progress<'a>> for CloneStats {
    fn from(progress: &git2::Progress<'a>) -> Self {
        Self {
            received_bytes: progress.received_bytes(),
            received_objects: progress.received_objects(),
            total_objects: progress.total_objects(),
        }
    }
}

/// Builds the event payload from a stats snapshot.
fn clone_progress_event(url: &str, stats: &CloneStats) -> CloneProgress {
    CloneProgress {
        url: url.to_string(),
        received: stats.received_bytes,
        total: stats.total_objects,
        objects: stats.received_objects,
    }
}

/// Destination guard: missing paths and empty directories are fine;
/// anything else (files, non-empty dirs) would make git2 half-write.
fn validate_clone_destination(path: &Path) -> Result<(), String> {
    if path.exists() && !path.is_dir() {
        return Err(format!(
            "destination exists and is not a directory: {}",
            path.display()
        ));
    }
    match std::fs::read_dir(path) {
        Err(_) => Ok(()), // missing — git2 creates it
        Ok(mut entries) => {
            if entries.next().is_some() {
                Err(format!(
                    "destination exists and is not empty: {}",
                    path.display()
                ))
            } else {
                Ok(())
            }
        }
    }
}

/// Clone `url` into `destination` (missing or empty), optionally shallow
/// (`depth`), emitting `clone-progress` events. Resolves with the path.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_clone(
    url: String,
    destination: String,
    depth: Option<u32>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || clone_sync(&url, &destination, depth, &app))
        .await
        .map_err(|e| format!("clone task failed: {e}"))?
}

fn clone_sync(
    url: &str,
    destination: &str,
    depth: Option<u32>,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    if url.trim().is_empty() {
        return Err("clone url must not be empty".to_string());
    }
    if destination.trim().is_empty() {
        return Err("clone destination must not be empty".to_string());
    }
    let dest = Path::new(destination);
    validate_clone_destination(dest)?;

    let mut callbacks = git2::RemoteCallbacks::new();
    let event_url = url.to_string();
    let last_emit = Mutex::new(std::time::Instant::now());
    let emitter = app.clone();
    callbacks.transfer_progress(move |stats| {
        let stats = CloneStats::from(&stats);
        let done = stats.total_objects > 0 && stats.received_objects >= stats.total_objects;
        let ready = {
            let mut last = last_emit.lock();
            let ready = clone_emit_ready(last.elapsed().as_millis() as u64, done);
            if ready {
                *last = std::time::Instant::now();
            }
            ready
        };
        if ready {
            let payload = clone_progress_event(&event_url, &stats);
            if let Err(err) = emitter.emit("clone-progress", &payload) {
                tracing::warn!(
                    url = %event_url,
                    error = %err,
                    "emit clone-progress failed"
                );
            }
        }
        true
    });

    let mut fetch = git2::FetchOptions::new();
    fetch.remote_callbacks(callbacks);
    if let Some(depth) = depth {
        fetch.depth(depth as i32);
    }
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch);
    builder
        .clone(url, dest)
        .map_err(|e| format!("clone failed: {e}"))?;
    Ok(destination.to_string())
}

#[cfg(test)]
mod clone_tests {
    use super::*;

    #[test]
    fn emit_ready_throttles_by_interval_but_always_when_done() {
        assert!(!clone_emit_ready(0, false));
        assert!(!clone_emit_ready(CLONE_EMIT_INTERVAL_MS - 1, false));
        assert!(clone_emit_ready(CLONE_EMIT_INTERVAL_MS, false));
        // The final callback always flushes.
        assert!(clone_emit_ready(0, true));
    }

    #[test]
    fn progress_event_maps_stats_fields() {
        let stats = CloneStats {
            received_bytes: 4096,
            received_objects: 10,
            total_objects: 42,
        };
        let event = clone_progress_event("https://host/repo.git", &stats);
        assert_eq!(
            event,
            CloneProgress {
                url: "https://host/repo.git".to_string(),
                received: 4096,
                total: 42,
                objects: 10,
            }
        );
        // Serializes with the documented field names.
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["url"], "https://host/repo.git");
        assert_eq!(json["received"], 4096);
        assert_eq!(json["total"], 42);
        assert_eq!(json["objects"], 10);
    }

    #[test]
    fn destination_validation_accepts_missing_and_empty() {
        let dir = std::env::temp_dir().join(format!("mygitui-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Missing: fine.
        assert!(validate_clone_destination(&dir).is_ok());
        // Empty dir: fine.
        std::fs::create_dir_all(&dir).unwrap();
        assert!(validate_clone_destination(&dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn destination_validation_rejects_nonempty_and_files() {
        let dir = std::env::temp_dir().join(format!("mygitui-clone-full-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("occupied.txt"), b"x").unwrap();
        let err = validate_clone_destination(&dir).unwrap_err();
        assert!(err.contains("not empty"), "got: {err}");

        let file = dir.join("occupied.txt");
        let err = validate_clone_destination(&file).unwrap_err();
        assert!(err.contains("not a directory"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---------------------------------------------------------------------------
// M2 (lane C3): mutations, branches/tags, remotes, net ops, auth — all
// routed through the per-repo serial op queue (contracts.md M2). M1 reads
// stay outside the queue: they share the repo mutex, which already
// serializes engine access, so concurrent reads never race a queued
// mutation.
//
// Progress: net ops forward engine callbacks to `OpCtx::emit_progress`
// (throttled ≥80ms); the queue emits the final `done: true` event for every
// op, success or error. Mutations bump the repo generation afterwards so
// in-flight log streams abort and clients re-request (streaming rules).
//
// Op kinds are the fixed set from contracts.md (`stage`, `commit`,
// `branch`, `fetch`, `pull`, `push`, `clone`): tags report as `branch`
// (ref-affecting), remote config ops as `fetch` (net category).
// ---------------------------------------------------------------------------

use tokio::sync::oneshot;

use crate::cli;
use crate::engine::git_engine::{FetchProgress, PushProgress};
use crate::engine::types::{
    BranchInfo, CommitOptions, FetchOptions, HookInfo, NetStats, PullOptions, PushOptions,
    RemoteInfo, SigningInfo, StageRequest,
};
use crate::ops::OpCtx;

/// Enqueue a mutating engine op on the repo's serial queue: `f` runs with
/// the repo lock held, the generation is bumped after it finishes (success
/// or error — hooks may have touched state even on failure), and the
/// command resolves through the returned receiver.
fn enqueue_mutation<T, F>(
    handle: &RepoHandle,
    kind: &'static str,
    f: F,
) -> oneshot::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce(OpCtx, &git2::Repository) -> Result<T, EngineError> + Send + 'static,
{
    let repo = handle.repo();
    let generation = handle.generation_shared();
    let (_op_id, rx) = handle.ops().enqueue(kind, move |ctx| {
        let result = {
            let guard = repo.lock();
            f(ctx, &guard)
        };
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        result.map_err(engine_err)
    });
    rx
}

/// Same as [`enqueue_mutation`] but without the generation bump (M2 read
/// ops that still run on the queue: branches, remotes, hook/signing info).
fn enqueue_read<T, F>(
    handle: &RepoHandle,
    kind: &'static str,
    f: F,
) -> oneshot::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce(OpCtx, &git2::Repository) -> Result<T, EngineError> + Send + 'static,
{
    let repo = handle.repo();
    let (_op_id, rx) = handle.ops().enqueue(kind, move |ctx| {
        let guard = repo.lock();
        f(ctx, &guard).map_err(engine_err)
    });
    rx
}

/// Await an enqueued op's result (worker done or queue closed).
async fn finish_op<T>(rx: oneshot::Receiver<Result<T, String>>) -> Result<T, String> {
    rx.await
        .map_err(|_| "op queue closed before completion".to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stage(
    repo_id: RepoId,
    request: StageRequest,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "stage", move |_ctx, repo| {
        engine.stage(repo, &request)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stage_all(
    repo_id: RepoId,
    unstage: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "stage", move |_ctx, repo| {
        engine.stage_all(repo, unstage)
    }))
    .await
}

/// Commit: engine first; `EngineError::Unsupported` (active hooks/signing
/// per C1's routing) falls back to the git CLI (cli.rs) so hooks and
/// signing actually run. Returns the new sha.
#[tauri::command(rename_all = "snake_case")]
pub async fn commit(
    repo_id: RepoId,
    options: CommitOptions,
    state: State<'_, RepoManager>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    let repo = handle.repo();
    let workdir = handle.root.clone();
    let generation = handle.generation_shared();
    let (_op_id, rx) = handle.ops().enqueue("commit", move |ctx| {
        let outcome = {
            let guard = repo.lock();
            engine.commit(&guard, &options)
        };
        let sha = match outcome {
            Ok(sha) => Ok(sha),
            Err(EngineError::Unsupported(reason)) => {
                ctx.emit_progress(&format!("falling back to git CLI: {reason}"), None);
                cli::commit_via_cli(&workdir, &options)
            }
            Err(err) => Err(engine_err(err)),
        };
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        sha
    });
    finish_op(rx).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn signing_info(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<SigningInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_read(&handle, "commit", move |_ctx, repo| {
        engine.signing_info(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn hooks_list(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<HookInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_read(&handle, "commit", move |_ctx, repo| {
        engine.hooks(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn branches(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<BranchInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_read(&handle, "branch", move |_ctx, repo| {
        engine.branches(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn branch_create(
    repo_id: RepoId,
    name: String,
    from: Option<String>,
    checkout: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "branch", move |_ctx, repo| {
        engine.branch_create(repo, &name, from.as_deref(), checkout)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn branch_switch(
    repo_id: RepoId,
    name: String,
    force: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "branch", move |_ctx, repo| {
        engine.branch_switch(repo, &name, force)
    }))
    .await
}

/// `r#into` unraws to the IPC arg key `into` (contracts.md M2).
#[tauri::command(rename_all = "snake_case")]
pub async fn branch_is_merged(
    repo_id: RepoId,
    name: String,
    r#into: String,
    state: State<'_, RepoManager>,
) -> Result<bool, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_read(&handle, "branch", move |_ctx, repo| {
        engine.branch_is_merged(repo, &name, &r#into)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn branch_delete(
    repo_id: RepoId,
    name: String,
    force: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "branch", move |_ctx, repo| {
        engine.branch_delete(repo, &name, force)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn branch_rename(
    repo_id: RepoId,
    old: String,
    new: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "branch", move |_ctx, repo| {
        engine.branch_rename(repo, &old, &new)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tag_create(
    repo_id: RepoId,
    name: String,
    target: Option<String>,
    message: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "branch", move |_ctx, repo| {
        engine.tag_create(repo, &name, target.as_deref(), message.as_deref())
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tag_delete(
    repo_id: RepoId,
    name: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "branch", move |_ctx, repo| {
        engine.tag_delete(repo, &name)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remotes(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<RemoteInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_read(&handle, "fetch", move |_ctx, repo| {
        engine.remotes(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_add(
    repo_id: RepoId,
    name: String,
    url: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "fetch", move |_ctx, repo| {
        engine.remote_add(repo, &name, &url)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_remove(
    repo_id: RepoId,
    name: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "fetch", move |_ctx, repo| {
        engine.remote_remove(repo, &name)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remote_set_url(
    repo_id: RepoId,
    name: String,
    url: String,
    push: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    finish_op(enqueue_mutation(&handle, "fetch", move |_ctx, repo| {
        engine.remote_set_url(repo, &name, &url, push)
    }))
    .await
}

/// Forward one libgit2 fetch/pull progress callback to the op's throttled
/// `op-progress` stream.
fn emit_fetch_progress(ctx: &OpCtx, progress: &FetchProgress) {
    ctx.emit_progress(
        &format!(
            "{}/{} objects",
            progress.objects_received, progress.objects_total
        ),
        fetch_pct(progress.objects_received, progress.objects_total),
    );
}

/// Pure: fetch/pull percentage; `None` until the remote announces a total
/// (contracts.md: `pct: number | null`).
pub(crate) fn fetch_pct(received: u32, total: u32) -> Option<f64> {
    (total > 0).then(|| received as f64 / total as f64 * 100.0)
}

/// Pure: push percentage, same null rule as [`fetch_pct`].
pub(crate) fn push_pct(current: u32, total: u32) -> Option<f64> {
    (total > 0).then(|| current as f64 / total as f64 * 100.0)
}

fn emit_push_progress(ctx: &OpCtx, progress: &PushProgress) {
    let message = if progress.message.is_empty() {
        format!("{}/{} refs", progress.current, progress.total)
    } else {
        progress.message.clone()
    };
    ctx.emit_progress(&message, push_pct(progress.current, progress.total));
}

/// Common enqueue body for fetch/pull/push: initial progress line, engine
/// op with the repo lock held and credentials attributed to this repo
/// (`auth::with_netop_context`), progress wiring, generation bump.
fn enqueue_net_op<T, F>(
    handle: &RepoHandle,
    kind: &'static str,
    opening: String,
    f: F,
) -> oneshot::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: for<'a> FnOnce(&'a OpCtx, &'a mut dyn FnMut(FetchProgress)) -> Result<T, EngineError>
        + Send
        + 'static,
{
    let generation = handle.generation_shared();
    let net_repo = handle.id.clone();
    let (_op_id, rx) = handle.ops().enqueue(kind, move |ctx| {
        ctx.emit_progress(&opening, None);
        let result = crate::auth::with_netop_context(&net_repo, || {
            f(&ctx, &mut |p| {
                emit_fetch_progress(&ctx, &p);
            })
        });
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        result.map_err(engine_err)
    });
    rx
}

#[tauri::command(rename_all = "snake_case")]
pub async fn fetch(
    repo_id: RepoId,
    options: FetchOptions,
    state: State<'_, RepoManager>,
) -> Result<NetStats, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    let repo = handle.repo();
    let opening = format!("fetching {}", options.remote);
    finish_op(enqueue_net_op(
        &handle,
        "fetch",
        opening,
        move |_ctx, progress| {
            let guard = repo.lock();
            engine.fetch(&guard, &options, progress)
        },
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pull(
    repo_id: RepoId,
    options: PullOptions,
    state: State<'_, RepoManager>,
) -> Result<NetStats, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    let repo = handle.repo();
    let opening = format!("pulling {}", options.remote);
    finish_op(enqueue_net_op(
        &handle,
        "pull",
        opening,
        move |_ctx, progress| {
            let guard = repo.lock();
            engine.pull(&guard, &options, progress)
        },
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn push(
    repo_id: RepoId,
    options: PushOptions,
    state: State<'_, RepoManager>,
) -> Result<NetStats, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.engine();
    let repo = handle.repo();
    let generation = handle.generation_shared();
    let net_repo = handle.id.clone();
    let opening = if options.branch.is_empty() {
        format!("pushing to {}", options.remote)
    } else {
        format!("pushing {} to {}", options.branch, options.remote)
    };
    let (_op_id, rx) = handle.ops().enqueue("push", move |ctx| {
        ctx.emit_progress(&opening, None);
        let result = crate::auth::with_netop_context(&net_repo, || {
            let guard = repo.lock();
            engine.push(&guard, &options, &mut |p| {
                emit_push_progress(&ctx, &p);
            })
        });
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        result.map_err(engine_err)
    });
    finish_op(rx).await
}

/// FE answer to an `auth-request` event: routes `(username, password,
/// store)` to the blocked credentials callback (auth broker).
#[tauri::command(rename_all = "snake_case")]
pub fn auth_respond(
    op_id: String,
    username: Option<String>,
    password: Option<String>,
    store: bool,
) -> Result<(), String> {
    let delivered = crate::auth::broker().answer(
        op_id.clone(),
        crate::auth::AuthAnswer {
            username,
            password,
            store,
        },
    );
    if delivered {
        Ok(())
    } else {
        Err(format!("no pending auth request for op {op_id}"))
    }
}

#[cfg(test)]
mod m2_tests {
    use super::*;

    #[test]
    fn fetch_pct_null_until_total_known() {
        assert_eq!(fetch_pct(5, 0), None);
        assert_eq!(fetch_pct(0, 8), Some(0.0));
        assert_eq!(fetch_pct(4, 8), Some(50.0));
        assert_eq!(fetch_pct(8, 8), Some(100.0));
    }

    #[test]
    fn push_pct_null_until_total_known() {
        assert_eq!(push_pct(1, 0), None);
        assert_eq!(push_pct(3, 4), Some(75.0));
    }

    /// Queue wiring through a real RepoManager handle (no engine involved):
    /// events reach the installed sink and close rejects new ops.
    #[test]
    fn repo_handle_op_queue_emits_progress_and_closes() {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-ipc-queue-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&dir).expect("open temp repo");
        let handle = manager.get(&info.repo_id).unwrap();
        let sink = crate::ops::test_sink::CollectorSink::shared();
        handle.ops().set_sink(sink.clone());

        let start_generation = handle.generation();
        let (op_id, rx) = handle.ops().enqueue("stage", move |ctx| {
            ctx.emit_progress("staging", None);
            Ok(())
        });
        rx.blocking_recv()
            .expect("queue alive")
            .expect("op succeeds");
        let events = sink.snapshot();
        assert_eq!(events.len(), 2, "progress + done: {events:?}");
        assert!(!events[0].done);
        assert_eq!(events[0].message, "staging");
        assert_eq!(events[1].op_id, op_id);
        assert!(events[1].done);
        assert_eq!(events[1].error, None);
        // Reads never touched the generation; queued ops here neither (the
        // bump lives in enqueue_mutation, not the queue).
        assert_eq!(handle.generation(), start_generation);

        manager.close(&info.repo_id);
        let (_id, rx) = handle.ops().enqueue("commit", move |_ctx| Ok(()));
        assert!(
            rx.blocking_recv().expect("receiver resolves").is_err(),
            "enqueue after close must reject"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The generation bump used by mutations: same counter the log streams
    /// snapshot (streaming rules in contracts.md).
    #[test]
    fn mutation_helper_bumps_generation_like_the_watcher() {
        // enqueue_mutation needs the engine trait object; drive the same
        // plumbing through the default engine's `stage`. Its result depends
        // on C1's parallel impl state (Unsupported stub vs real logic) —
        // the generation bump must happen regardless, which is what this
        // asserts.
        let dir = std::env::temp_dir().join(format!(
            "mygitui-ipc-bump-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&dir).unwrap();
        let handle = manager.get(&info.repo_id).unwrap();
        let before = handle.generation();

        let request = StageRequest {
            targets: Vec::new(),
            unstage: false,
        };
        let rx = {
            let engine = handle.engine();
            enqueue_mutation(&handle, "stage", move |_ctx, repo| {
                engine.stage(repo, &request)
            })
        };
        // Resolve (success or engine error both fine); then check the bump.
        let _ = rx.blocking_recv().expect("queue alive");
        assert_eq!(
            handle.generation(),
            before + 1,
            "generation bumps after the mutation op, whatever the engine result"
        );
        manager.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---------------------------------------------------------------------------
// M3 (lane D4): checkpoints (undo system) + dangerous-op previews.
//
// Checkpoints are snapshot commits under hidden refs
// `refs/mygitui/checkpoints/<unix_millis>-<reason>` capturing the full
// workdir state (tracked + untracked) over the current HEAD. Restore
// auto-creates a `pre-restore` checkpoint first (the undo system is itself
// undoable) and never moves HEAD or branch refs; GC keeps the newest
// checkpoint regardless of age.
//
// Routing: create/restore/gc are mutations and go through the serial op
// queue with the generation bump (`enqueue_mutation`, mirroring M2);
// listing and `ops_preview` are pure reads on the M1 read path (repo mutex
// + spawn_blocking, no queue, no generation bump).
//
// WIRING NOTE: these commands call the inherent `*_impl` twins on
// `Libgit2Engine` directly because the single `impl GitEngineM3` block is
// owned by merge.rs; the `GitEngineM3::checkpoint_*` forwarding lines land
// there at integration (afterwards `handle.m3()` is equivalent).
// ---------------------------------------------------------------------------

use crate::engine::libgit2::Libgit2Engine;
use crate::engine::types::PreviewInfo;

#[tauri::command(rename_all = "snake_case")]
pub async fn checkpoint_create(
    repo_id: RepoId,
    reason: String,
    state: State<'_, RepoManager>,
) -> Result<CheckpointInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = Libgit2Engine::new();
    finish_op(enqueue_mutation(
        &handle,
        "checkpoint",
        move |_ctx, repo| engine.checkpoint_create_impl(repo, &reason),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn checkpoints(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<CheckpointInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = Libgit2Engine::new();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.checkpoints_impl(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("checkpoints task failed: {e}"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn checkpoint_restore(
    repo_id: RepoId,
    id: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = Libgit2Engine::new();
    finish_op(enqueue_mutation(
        &handle,
        "checkpoint",
        move |_ctx, repo| engine.checkpoint_restore_impl(repo, &id),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn checkpoint_gc(
    repo_id: RepoId,
    older_than_days: u32,
    state: State<'_, RepoManager>,
) -> Result<u32, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = Libgit2Engine::new();
    finish_op(enqueue_mutation(
        &handle,
        "checkpoint",
        move |_ctx, repo| engine.checkpoint_gc_impl(repo, older_than_days),
    ))
    .await
}

/// Pure what-if preview of a dangerous op (no mutation, no checkpoint):
/// `kind` is one of "reset_hard" / "clean" / "checkout_force" /
/// "branch_delete", `params` carries the kind's arguments (e.g.
/// `{"to": "<commit-ish>"}`, `{"dirs": bool}`, `{"name": "...",
/// "into": "..."}`).
#[tauri::command(rename_all = "snake_case")]
pub async fn ops_preview(
    repo_id: RepoId,
    kind: String,
    params: serde_json::Value,
    state: State<'_, RepoManager>,
) -> Result<PreviewInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = Libgit2Engine::new();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine
            .preview_impl(&repo, &kind, &params)
            .map_err(engine_err)
    })
    .await
    .map_err(|e| format!("preview task failed: {e}"))?
}
/// Snapshot current state as an undo point right before a dangerous op.
/// FE flow: ops_preview → dialog → guard_checkpoint → confirm → op.
#[tauri::command(rename_all = "snake_case")]
pub async fn guard_checkpoint(
    repo_id: RepoId,
    reason: String,
    state: State<'_, RepoManager>,
) -> Result<CheckpointInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.checkpoint_create(&repo, &reason).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("guard checkpoint task failed: {e}"))?
}

// ---------------------------------------------------------------------------
// M3 E1: conflict editor file access
// ---------------------------------------------------------------------------

/// Raw bytes of one workdir file (`path` workdir-relative). Backs the
/// conflict editor: conflicted paths carry marker previews in the workdir
/// (see `engine::merge`) and no other command returns raw file content.
/// A path that escapes the workdir (`..` components, absolute or
/// drive-prefixed forms) or is not a regular file rejects; a file that does
/// not exist resolves to an empty vec (the editor treats that as "no
/// marker preview written yet"). Workdir reads need no engine call, so the
/// repo mutex is not taken.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_read_file(
    repo_id: RepoId,
    path: String,
    state: State<'_, RepoManager>,
) -> Result<Vec<u8>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || read_workdir_file(&root, &path))
        .await
        .map_err(|e| format!("read_file task failed: {e}"))?
}

/// Synchronous body of [`repo_read_file`] (validation + disk read).
fn read_workdir_file(root: &Path, path: &str) -> Result<Vec<u8>, String> {
    let relative = Path::new(path);
    if path.is_empty() {
        return Err("path is empty".into());
    }
    if relative.is_absolute() || has_windows_prefix(relative) {
        return Err(format!("path must be workdir-relative: `{path}`"));
    }
    if relative
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        return Err(format!("path must not contain `..`: `{path}`"));
    }
    let full = root.join(relative);
    // Belt and braces: a drive-relative form like `C:foo` slips past the
    // absolute check but `join` replaces the base with it — the containment
    // check catches every replacement case.
    if full.strip_prefix(root).is_err() {
        return Err(format!("path escapes the repository: `{path}`"));
    }
    match std::fs::metadata(&full) {
        // Missing file → empty content (documented "no preview written").
        Err(_) => Ok(Vec::new()),
        Ok(meta) if !meta.is_file() => Err(format!("`{path}` is not a regular file")),
        Ok(_) => std::fs::read(&full).map_err(|e| format!("reading `{path}` failed: {e}")),
    }
}

/// True when `path` carries a Windows prefix (`C:\`, `\\?\C:\`, `\\server`).
fn has_windows_prefix(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Component;
        matches!(
            path.components().next(),
            Some(Component::Prefix(_)) | Some(Component::RootDir)
        )
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

// ---------- M3: power + safety commands ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn merge_branch(
    repo_id: RepoId,
    ref_name: String,
    no_ff: bool,
    state: State<'_, RepoManager>,
) -> Result<MergeResult, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "merge", move |_ctx, repo| {
        engine.merge_branch(repo, &ref_name, no_ff)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn merge_abort(repo_id: RepoId, state: State<'_, RepoManager>) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "merge", move |_ctx, repo| {
        engine.merge_abort(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn sequencer_abort(repo_id: RepoId, state: State<'_, RepoManager>) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "merge", move |_ctx, repo| {
        engine.sequencer_abort(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conflicts(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<ConflictFile>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.conflicts(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("conflicts task failed: {e}"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conflict_resolve(
    repo_id: RepoId,
    path: String,
    resolution: ConflictResolution,
    custom_content: Option<Vec<u8>>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "merge", move |_ctx, repo| {
        engine.conflict_resolve(repo, &path, resolution, custom_content.as_deref())
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cherry_pick(
    repo_id: RepoId,
    shas: Vec<String>,
    state: State<'_, RepoManager>,
) -> Result<MergeResult, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "merge", move |_ctx, repo| {
        engine.cherry_pick(repo, &shas)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn revert(
    repo_id: RepoId,
    shas: Vec<String>,
    state: State<'_, RepoManager>,
) -> Result<MergeResult, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "merge", move |_ctx, repo| {
        engine.revert(repo, &shas)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reset(
    repo_id: RepoId,
    kind: ResetKind,
    to: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "merge", move |_ctx, repo| {
        engine.reset(repo, kind, &to)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rebase_start(
    repo_id: RepoId,
    plan: Vec<RebaseStep>,
    onto: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<RebaseState, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "rebase", move |_ctx, repo| {
        engine.rebase_start(repo, &plan, onto.as_deref())
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rebase_state(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<RebaseState, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.rebase_state(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("rebase state task failed: {e}"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rebase_continue(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<RebaseState, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "rebase", move |_ctx, repo| {
        engine.rebase_continue(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rebase_abort(repo_id: RepoId, state: State<'_, RepoManager>) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "rebase", move |_ctx, repo| {
        engine.rebase_abort(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stash_list(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<StashInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.stash_list(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("stash list task failed: {e}"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stash_push(
    repo_id: RepoId,
    message: Option<String>,
    keep_index: bool,
    include_untracked: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "stash", move |_ctx, repo| {
        engine.stash_push(repo, message.as_deref(), keep_index, include_untracked)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stash_apply(
    repo_id: RepoId,
    index: u32,
    pop: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "stash", move |_ctx, repo| {
        engine.stash_apply(repo, index, pop)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stash_drop(
    repo_id: RepoId,
    index: u32,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "stash", move |_ctx, repo| {
        engine.stash_drop(repo, index)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stash_branch(
    repo_id: RepoId,
    name: String,
    index: u32,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "stash", move |_ctx, repo| {
        engine.stash_branch(repo, &name, index)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn worktrees(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<WorktreeInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.worktrees(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("worktrees task failed: {e}"))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn worktree_add(
    repo_id: RepoId,
    path: String,
    branch: Option<String>,
    new_branch: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "worktree", move |_ctx, repo| {
        engine.worktree_add(repo, &path, branch.as_deref(), new_branch.as_deref())
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn worktree_remove(
    repo_id: RepoId,
    name: String,
    force: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    finish_op(enqueue_mutation(&handle, "worktree", move |_ctx, repo| {
        engine.worktree_remove(repo, &name, force)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reflog(
    repo_id: RepoId,
    name: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<Vec<ReflogEntry>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = handle.m3();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.reflog(&repo, name.as_deref()).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("reflog task failed: {e}"))?
}

// ---------------------------------------------------------------------------
// M4 (lane F4): git clean execution + custom shell actions (contracts.md
// "Commands (M4)").
//
// `repo_clean` deletes a caller-supplied list of workdir-relative untracked
// paths (as computed by `ops_preview` kind "clean"). Safety chain: every
// path is validated first (relative, no `..`, inside the repo root — one
// bad path aborts the whole op with zero side effects), then a checkpoint
// of the full workdir state is created (reason argument, FE passes
// "pre-clean"), then the paths are removed. Missing paths are skipped;
// symlinks are unlinked, never traversed. Returns the count of removed
// paths. It is an op-queue mutation: serialized with other mutations and
// bumps the generation after (the watcher-driven clients resync).
//
// `action_run` / `action_cancel` delegate to the `actions` runner; the
// registry is process-global managed state (see actions.rs).
// ---------------------------------------------------------------------------

use crate::actions::{ActionRegistry, ActionSpec, TauriActionSink};

/// git clean execution: checkpoint first, then delete each validated path.
/// Returns the number of paths removed.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_clean(
    repo_id: RepoId,
    paths: Vec<String>,
    checkpoint_reason: String,
    state: State<'_, RepoManager>,
) -> Result<u32, String> {
    let handle = get_handle(&state, &repo_id)?;
    finish_op(clean_op(&handle, paths, checkpoint_reason)).await
}

/// Enqueued `clean` body (op-queue mutation with the generation bump, like
/// [`enqueue_mutation`]). Split from the command so tests can drive it
/// through a real handle without `State`.
fn clean_op(
    handle: &RepoHandle,
    paths: Vec<String>,
    checkpoint_reason: String,
) -> oneshot::Receiver<Result<u32, String>> {
    let engine = handle.m3();
    let repo = handle.repo();
    let root = handle.root.clone();
    let generation = handle.generation_shared();
    let (_op_id, rx) = handle.ops().enqueue("clean", move |_ctx| {
        let result = (|| -> Result<u32, String> {
            // Validate FIRST so a rejected op has zero side effects —
            // notably no checkpoint.
            let resolved = validate_clean_paths(&root, &paths)?;
            // SAFETY SECOND: the exact pre-clean state becomes an undo
            // point (checkpoint refs never move HEAD/branches, status
            // untouched).
            {
                let guard = repo.lock();
                engine
                    .checkpoint_create(&guard, &checkpoint_reason)
                    .map_err(engine_err)?;
            }
            remove_resolved(&resolved)
        })();
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        result
    });
    rx
}

/// Validation half of the clean deletion: every path must be
/// workdir-relative, non-empty, free of `..` and resolve inside `root`.
/// One bad path fails the whole op before anything is touched.
fn validate_clean_paths(root: &Path, paths: &[String]) -> Result<Vec<std::path::PathBuf>, String> {
    let mut resolved = Vec::with_capacity(paths.len());
    for path in paths {
        if path.trim().is_empty() || path.trim() == "." {
            return Err(format!(
                "clean path must be a non-empty workdir-relative path: `{path}`"
            ));
        }
        let relative = Path::new(path);
        if relative.is_absolute() || has_windows_prefix(relative) {
            return Err(format!("clean path must be workdir-relative: `{path}`"));
        }
        if relative
            .components()
            .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(format!("clean path must not contain `..`: `{path}`"));
        }
        let full = root.join(relative);
        // Belt and braces (same containment rule as `read_workdir_file`),
        // plus an explicit root guard: "." forms resolve onto the repo root.
        if full.strip_prefix(root).is_err() || full == root {
            return Err(format!("clean path escapes the repository: `{path}`"));
        }
        resolved.push(full);
    }
    Ok(resolved)
}

/// Deletion half of the clean deletion: remove already-validated
/// paths. A missing path (vanished between preview and confirm) is
/// skipped; a symlink is unlinked, never traversed. Returns the count of
/// paths removed.
fn remove_resolved(resolved: &[std::path::PathBuf]) -> Result<u32, String> {
    let mut removed = 0u32;
    for full in resolved {
        // symlink_metadata does not follow links: a symlinked "directory"
        // goes through remove_file below instead of remove_dir_all.
        let Ok(meta) = std::fs::symlink_metadata(full) else {
            continue; // gone already — nothing to clean
        };
        let result = if meta.is_dir() {
            std::fs::remove_dir_all(full)
        } else {
            std::fs::remove_file(full)
        };
        result.map_err(|e| format!("removing `{}` failed: {e}", full.display()))?;
        removed += 1;
    }
    Ok(removed)
}

/// Run a user-defined shell action with cwd = repo root, streaming its
/// output as `action-output` events. Returns the run id (cancel token).
#[tauri::command(rename_all = "snake_case")]
pub fn action_run(
    repo_id: RepoId,
    name: String,
    command: String,
    app: tauri::AppHandle,
    state: State<'_, RepoManager>,
    runs: State<'_, ActionRegistry>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
    let launch = crate::actions::run_action(
        &runs,
        ActionSpec {
            repo_id: repo_id.0,
            name,
            command,
            cwd: handle.root.clone(),
        },
        Arc::new(TauriActionSink::new(app)),
    )?;
    Ok(launch.run_id)
}

/// Kill a running action; `false` when the run is unknown or already done.
#[tauri::command(rename_all = "snake_case")]
pub fn action_cancel(run_id: String, runs: State<'_, ActionRegistry>) -> Result<bool, String> {
    Ok(runs.cancel(&run_id))
}

#[cfg(test)]
mod m4_clean_tests {
    use super::*;

    fn temp_repo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-clean-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        // Clean writes a checkpoint commit, which needs a committer identity.
        // CI runners have no global git config, so set it locally (same as the
        // other engine test helpers).
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Clean Test")?;
                c.set_str("user.email", "clean@test.local")
            })
            .unwrap();
        dir
    }

    #[test]
    fn clean_removal_removes_files_dirs_and_counts() {
        let root = temp_repo("del");
        std::fs::write(root.join("loose.txt"), b"x").unwrap();
        std::fs::create_dir_all(root.join("nested/deep")).unwrap();
        std::fs::write(root.join("nested/deep/a.txt"), b"x").unwrap();
        std::fs::write(root.join("nested/b.txt"), b"x").unwrap();

        let resolved = validate_clean_paths(
            &root,
            &["loose.txt".into(), "nested".into(), "missing.txt".into()],
        )
        .unwrap();
        let removed = remove_resolved(&resolved).unwrap();
        assert_eq!(removed, 2, "missing paths are skipped, not counted");
        assert!(!root.join("loose.txt").exists());
        assert!(!root.join("nested").exists());
        assert!(root.join(".git").exists(), "repo itself untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_validation_rejects_escaping_paths_before_touching_disk() {
        let root = temp_repo("escape");
        std::fs::write(root.join("keep.txt"), b"x").unwrap();
        let outside = root.parent().unwrap().join("outside-target.txt");
        std::fs::write(&outside, b"x").unwrap();

        let bad: Vec<String> = vec![
            "../outside-target.txt".into(),
            "a/../../b".into(),
            std::fs::canonicalize(&outside)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            String::new(),
            ".".into(),
        ];
        for path in bad {
            let err = validate_clean_paths(&root, std::slice::from_ref(&path)).unwrap_err();
            assert!(
                err.contains("relative") || err.contains("..") || err.contains("escapes"),
                "`{path}` rejected with a clear reason, got: {err}"
            );
        }
        assert!(root.join("keep.txt").exists(), "nothing deleted on reject");
        assert!(outside.exists(), "outside target untouched");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn clean_removal_unlinks_symlinks_without_traversing() {
        let root = temp_repo("symlink");
        let target_dir =
            std::env::temp_dir().join(format!("mygitui-clean-target-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&target_dir);
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("precious.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(&target_dir, root.join("linked")).unwrap();

        let resolved = validate_clean_paths(&root, &["linked".into()]).unwrap();
        let removed = remove_resolved(&resolved).unwrap();
        assert_eq!(removed, 1);
        assert!(!root.join("linked").exists(), "link removed");
        assert!(target_dir.join("precious.txt").exists(), "target survived");
        let _ = std::fs::remove_dir_all(&target_dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// End-to-end through the op queue: checkpoint lands first, files are
    /// gone, generation bumps, and the checkpoint is listed afterwards.
    #[test]
    fn clean_op_checkpoints_then_deletes_and_bumps_generation() {
        let root = temp_repo("e2e");
        std::fs::write(root.join("junk.log"), b"x").unwrap();
        std::fs::write(root.join("keep.txt"), b"x").unwrap();

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&root).unwrap();
        let handle = manager.get(&info.repo_id).unwrap();
        let before = handle.generation();

        let removed = clean_op(
            &handle,
            vec!["junk.log".to_string()],
            "pre-clean".to_string(),
        )
        .blocking_recv()
        .expect("queue alive")
        .expect("clean succeeds");
        assert_eq!(removed, 1);
        assert!(!root.join("junk.log").exists());
        assert!(root.join("keep.txt").exists());
        assert_eq!(handle.generation(), before + 1, "generation bumps");

        // The undo point exists and names the reason.
        let engine = Libgit2Engine::new();
        let checkpoints = {
            let repo = handle.repo();
            let repo = repo.lock();
            engine.checkpoints_impl(&repo).unwrap()
        };
        assert!(
            checkpoints.iter().any(|cp| cp.reason == "pre-clean"),
            "pre-clean checkpoint present: {checkpoints:?}"
        );

        manager.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_op_rejects_escaping_paths_without_checkpoint_or_deletion() {
        let root = temp_repo("reject");
        std::fs::write(root.join("junk.log"), b"x").unwrap();

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&root).unwrap();
        let handle = manager.get(&info.repo_id).unwrap();

        let err = clean_op(
            &handle,
            vec!["../evil".to_string()],
            "pre-clean".to_string(),
        )
        .blocking_recv()
        .expect("queue alive")
        .unwrap_err();
        assert!(err.contains(".."), "got: {err}");
        assert!(root.join("junk.log").exists(), "file survived the reject");

        // No checkpoint was taken for the rejected op.
        let engine = Libgit2Engine::new();
        let checkpoints = {
            let repo = handle.repo();
            let repo = repo.lock();
            engine.checkpoints_impl(&repo).unwrap()
        };
        assert!(
            !checkpoints.iter().any(|cp| cp.reason == "pre-clean"),
            "no checkpoint on rejected clean: {checkpoints:?}"
        );

        manager.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&root);
    }
}

// ---------------------------------------------------------------------------
// M4 (lane F3): submodule management + gitignore quick-add.
//
// `submodules` is a plain read (M1 read path: repo mutex + spawn_blocking);
// `submodule_update` / `submodule_sync` are mutations on the serial op queue
// (contracts.md M4: queue kind `submodule`); `gitignore_add` is a pure
// workdir file write (no git objects touched — the FS-only twin of
// `repo_read_file`'s no-mutex shortcut), and `gitignore_templates` is a
// static builtin list with no repo involved.
// ---------------------------------------------------------------------------

use crate::engine::types::SubmoduleInfo;

/// One curated builtin gitignore template (the `gitignore_templates` reply,
/// mirroring the contracts.md shape `{name, description, patterns}`).
/// `patterns` is ONE newline-joined `.gitignore` snippet (no trailing
/// newline); the FE applies it line-by-line via `gitignore_add`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GitignoreTemplate {
    pub name: String,
    pub description: String,
    pub patterns: String,
}

/// The curated template list. Small and accurate on purpose: real
/// `.gitignore` patterns only (GitHub gitignore / gitignore.io canon).
pub fn gitignore_templates_list() -> Vec<GitignoreTemplate> {
    let entry = |name: &str, description: &str, lines: &[&str]| GitignoreTemplate {
        name: name.to_owned(),
        description: description.to_owned(),
        patterns: lines.join("\n"),
    };
    vec![
        entry(
            "Node",
            "Node.js / npm / yarn / pnpm projects",
            &[
                "node_modules/",
                "npm-debug.log*",
                "yarn-debug.log*",
                "yarn-error.log*",
                "pnpm-debug.log*",
                ".npm",
                ".eslintcache",
                "coverage/",
            ],
        ),
        entry(
            "Rust",
            "Cargo projects (build output, backups)",
            &["/target", "**/*.rs.bk"],
        ),
        entry(
            "Python",
            "CPython bytecode, packaging, tool caches",
            &[
                "__pycache__/",
                "*.py[cod]",
                "*.egg-info/",
                ".eggs/",
                "build/",
                "dist/",
                ".venv/",
                "venv/",
                ".pytest_cache/",
                ".mypy_cache/",
                ".ruff_cache/",
            ],
        ),
        entry(
            "C++",
            "Object files, binaries, build directories",
            &[
                "*.o", "*.obj", "*.out", "*.a", "*.lib", "*.so", "*.so.*", "*.dylib", "*.dll",
                "*.exe", "build/", "bin/", "Debug/", "Release/", "x64/", "x86/",
            ],
        ),
        entry(
            "Go",
            "Compiled binaries, test artifacts, vendor tree",
            &[
                "*.exe", "*.exe~", "*.dll", "*.so", "*.dylib", "*.test", "*.out", "vendor/",
            ],
        ),
        entry(
            "Java",
            "Class/jar output, Maven + Gradle artifacts",
            &[
                "*.class",
                "*.jar",
                "*.war",
                "*.ear",
                "target/",
                "build/",
                "out/",
                ".gradle/",
                ".settings/",
                ".classpath",
                ".project",
            ],
        ),
        entry(
            "macOS",
            "Finder metadata and resource forks",
            &[
                ".DS_Store",
                ".AppleDouble",
                ".LSOverride",
                "._*",
                ".Spotlight-V100",
                ".Trashes",
                ".fseventsd",
            ],
        ),
        entry(
            "Windows",
            "Explorer metadata and the recycle bin",
            &[
                "Thumbs.db",
                "Thumbs.db:encryptable",
                "ehthumbs.db",
                "ehthumbs_vista.db",
                "Desktop.ini",
                "$RECYCLE.BIN/",
                "*.lnk",
            ],
        ),
        entry(
            "VS Code",
            "Editor settings except shared workspace files",
            &[
                ".vscode/*",
                "!.vscode/settings.json",
                "!.vscode/tasks.json",
                "!.vscode/launch.json",
                "!.vscode/extensions.json",
                "*.code-workspace",
            ],
        ),
        entry(
            "JetBrains",
            "IDE workspace files and build output",
            &[".idea/", "*.iml", "*.ipr", "*.iws", "out/"],
        ),
        entry(
            "Svelte",
            "SvelteKit / Svelte build and cache dirs",
            &[".svelte-kit/", "build/", "dist/", ".vercel", ".netlify"],
        ),
        entry(
            "Vite",
            "Vite output, caches, local env files",
            &["dist/", "dist-ssr/", "*.local", ".vite/"],
        ),
        entry(
            "Terraform",
            "State, variable values, crash logs, overrides",
            &[
                "*.tfstate",
                "*.tfstate.*",
                "*.tfvars",
                "*.tfvars.json",
                ".terraform/",
                "crash.log",
                "crash.*.log",
                "override.tf",
                "override.tf.json",
                "*_override.tf",
                "*_override.tf.json",
                ".terraformrc",
                "terraform.rc",
            ],
        ),
        entry(
            "Unreal",
            "Unreal Engine generated content",
            &[
                "Build/",
                "Binaries/",
                "DerivedDataCache/",
                "Intermediate/",
                "Saved/",
                ".vs/",
            ],
        ),
        entry(
            "Unity",
            "Unity library, temp, logs and build output",
            &[
                "[Ll]ibrary/",
                "[Tt]emp/",
                "[Oo]bj/",
                "[Bb]uild/",
                "[Bb]uilds/",
                "[Ll]ogs/",
                "[Uu]serSettings/",
                "MemoryCaptures/",
                "sysinfo.txt",
                "crashlytics-build.properties",
            ],
        ),
    ]
}

/// Append `pattern` to the repo-root `.gitignore` (creating the file when
/// missing). Idempotent: an existing exact line match is left untouched.
/// Validation: the pattern must be non-empty and a single line.
fn gitignore_add_sync(root: &Path, pattern: &str) -> Result<(), String> {
    if pattern.contains('\n') || pattern.contains('\r') {
        return Err("gitignore pattern must be a single line".to_string());
    }
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err("gitignore pattern is empty".to_string());
    }
    let file = root.join(".gitignore");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    if existing.lines().any(|line| line == pattern) {
        return Ok(()); // already present (exact line match)
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(pattern);
    updated.push('\n');
    std::fs::write(&file, updated).map_err(|e| format!("writing .gitignore failed: {e}"))
}

/// List every submodule with checked-out vs recorded state (contracts.md:
/// `SubmoduleInfo[]`; status strings mirror `git status` submodule wording).
#[tauri::command(rename_all = "snake_case")]
pub async fn submodules(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<SubmoduleInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = Libgit2Engine::new();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine.submodules_impl(&repo).map_err(engine_err)
    })
    .await
    .map_err(|e| format!("submodules task failed: {e}"))?
}

/// `git submodule update [--init] [--recursive] <path>` for one submodule.
/// Network fetches (missing clone / missing target commit) route credential
/// prompts through the auth broker attributed to this repo.
#[tauri::command(rename_all = "snake_case")]
pub async fn submodule_update(
    repo_id: RepoId,
    path: String,
    init: bool,
    recursive: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = Libgit2Engine::new();
    finish_op(enqueue_mutation(&handle, "submodule", move |_ctx, repo| {
        crate::auth::with_netop_context(&repo_id, || {
            engine.submodule_update_impl(repo, &path, init, recursive)
        })
    }))
    .await
}

/// `git submodule sync [path]`: rewrite the submodule URL from `.gitmodules`
/// into the repository config. `None`/empty `path` syncs all submodules.
#[tauri::command(rename_all = "snake_case")]
pub async fn submodule_sync(
    repo_id: RepoId,
    path: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let engine = Libgit2Engine::new();
    finish_op(enqueue_mutation(&handle, "submodule", move |_ctx, repo| {
        engine.submodule_sync_impl(repo, path.as_deref())
    }))
    .await
}

/// Append one pattern to the repo-root `.gitignore` (created when missing).
/// The watcher picks the change up; the FE refreshes status afterwards.
#[tauri::command(rename_all = "snake_case")]
pub async fn gitignore_add(
    repo_id: RepoId,
    pattern: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || gitignore_add_sync(&root, &pattern))
        .await
        .map_err(|e| format!("gitignore_add task failed: {e}"))?
}

/// The curated builtin gitignore templates (`{name, description, patterns}`).
#[tauri::command(rename_all = "snake_case")]
pub fn gitignore_templates() -> Vec<GitignoreTemplate> {
    gitignore_templates_list()
}

#[cfg(test)]
mod m4_gitignore_tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mygitui-gi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn gitignore_add_creates_appends_and_dedups() {
        let dir = scratch("ok");
        // Creates the file.
        gitignore_add_sync(&dir, "node_modules/").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "node_modules/\n"
        );
        // Appends, normalizing a missing trailing newline first.
        std::fs::write(dir.join(".gitignore"), "target").unwrap();
        gitignore_add_sync(&dir, "*.log").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "target\n*.log\n"
        );
        // Exact-line dedup is a no-op (whitespace-trimmed pattern).
        gitignore_add_sync(&dir, "  *.log  ").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
            "target\n*.log\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gitignore_add_rejects_empty_and_newlines() {
        let dir = scratch("bad");
        assert!(gitignore_add_sync(&dir, "   ").is_err());
        assert!(gitignore_add_sync(&dir, "a\nb").is_err());
        assert!(gitignore_add_sync(&dir, "a\r\nb").is_err());
        assert!(
            !dir.join(".gitignore").exists(),
            "nothing written on rejection"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn templates_are_curated_unique_and_wellformed() {
        let templates = gitignore_templates_list();
        assert!(templates.len() >= 15, "curated list: {}", templates.len());
        let mut names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "template names must be unique");
        for t in &templates {
            assert!(!t.description.is_empty(), "{}: description", t.name);
            assert!(!t.patterns.is_empty(), "{}: patterns", t.name);
            assert!(!t.patterns.contains('\r'), "{}: no CR", t.name);
            assert!(
                !t.patterns.ends_with('\n'),
                "{}: no trailing newline (FE joins lines)",
                t.name
            );
            assert!(
                t.patterns.lines().all(|l| !l.trim().is_empty()),
                "{}: no blank pattern lines",
                t.name
            );
        }
        // Serializes with the documented field names.
        let json = serde_json::to_value(&templates).unwrap();
        assert_eq!(json[0]["name"], templates[0].name);
        assert_eq!(json[0]["patterns"], templates[0].patterns);
    }
}

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

use crate::forge::{self, CheckInfo, ForgeContext, ForgeError, ForgeStatus, PrCreated, PrInfo};

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

/// Open PRs of the repo (`gh pr list --json … --limit 50`).
#[tauri::command(rename_all = "snake_case")]
pub async fn pr_list(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<PrInfo>, String> {
    let root = get_handle(&state, &repo_id)?.root.clone();
    tauri::async_runtime::spawn_blocking(move || forge::pr_list_sync(&root))
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

// ---------------------------------------------------------------------------
// M7 (lane I1): contribution statistics — commit activity heatmap buckets +
// contributor rollups (contracts.md "Commands (M7 stats)"). Pure reads on
// the M1 read path (repo mutex + spawn_blocking, no op queue, no generation
// bump); the logic lives in `engine/stats.rs` as inherent `*_impl` twins on
// `Libgit2Engine` (the M2-lane convention).
// ---------------------------------------------------------------------------

use crate::engine::stats::{Contributor, DayCount};

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
    let engine = Libgit2Engine::new();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine
            .commit_activity_impl(&repo, max_days, author.as_deref())
            .map_err(engine_err)
    })
    .await
    .map_err(|e| format!("commit_activity task failed: {e}"))?
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
    let engine = Libgit2Engine::new();
    let repo = handle.repo();
    tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine
            .contributor_stats_impl(&repo, max_days)
            .map_err(engine_err)
    })
    .await
    .map_err(|e| format!("contributor_stats task failed: {e}"))?
}
