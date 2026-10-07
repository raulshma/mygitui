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
    BlameLine, CommitInfo, DiffSide, FileDiff, LogFilter, RepoId, RepoInfo, RepoStatus,
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
