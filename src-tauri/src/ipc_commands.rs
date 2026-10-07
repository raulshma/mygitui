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
