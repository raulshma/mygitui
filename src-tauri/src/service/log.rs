use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;
use ts_rs::TS;

use crate::engine::types::{CommitInfo, LogFilter, RepoId};
use crate::graph::types::{self, GraphRow};
use crate::repo::{RepoHandle, RepoManager, StreamHandle};
use crate::service::{chunk_vec, enqueue_remote_read, get_handle};

/// Commits per `repo_log_stream` / `repo_file_history` page.
const LOG_PAGE_SIZE: usize = 500;
/// Whole-walk cap for the remote log stream: the remote engine returns the
/// log in one shot (no cursor paging), so the stream fetches once and pages
/// locally. Generous bound protects device + host memory.
const REMOTE_LOG_LIMIT: usize = 100_000;

/// One page of streamed history (contracts.md: `LogPage`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogPage {
    pub commits: Vec<CommitInfo>,
    pub rows: Vec<GraphRow>,
    pub next_cursor: Option<String>,
    pub generation: u64,
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
        .spawn(move || stream_log(handle, filter, on_page, token))
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
        .spawn(move || stream_log(handle, filter, on_page, token))
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
) {
    if let Some(remote) = handle.remote() {
        stream_log_remote(&handle, remote.clone(), filter, on_page, token);
        return;
    }
    let Ok(repo) = handle.repo() else {
        return; // unreachable: remote repos took the branch above
    };
    let start_generation = handle.generation();
    let mut cursor: Option<String> = None;
    // Lane state belongs to this stream: preserve it across its pages, but
    // never let a restarted or filtered stream inherit lanes from its
    // predecessor.
    let mut lanes = types::LaneState::default();

    loop {
        if token.is_cancelled() || handle.is_closed() {
            break;
        }
        if handle.generation() != start_generation {
            break; // repo changed under us: abort, client re-requests
        }

        let page = {
            let engine = handle.engine();
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

        let rows = types::layout_page(&commits, &mut lanes);

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

/// Remote twin of [`stream_log`]: the remote engine's `log` returns the
/// whole (bounded) walk in one shot, so ONE queued read produces the full
/// result and this loop only lays it out and pages it over the channel.
/// Graph rows come from the same pure `graph::types::layout_page` with lane
/// state carried across pages; abort checks mirror the local loop (stream
/// cancelled/superseded, repo closed, or generation moved between sends).
fn stream_log_remote(
    handle: &Arc<RepoHandle>,
    remote: Arc<crate::repo::RemoteHandle>,
    filter: LogFilter,
    on_page: Channel<LogPage>,
    token: Arc<StreamHandle>,
) {
    let start_generation = handle.generation();
    let rx = enqueue_remote_read(handle, &remote, "log", move |git, _ws| {
        git.log(&filter, REMOTE_LOG_LIMIT, None)
    });
    let (commits, _next_cursor) = match rx.blocking_recv() {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => {
            tracing::warn!(
                repo = %handle.id.0,
                error = %err,
                "remote log stream: engine error, aborting"
            );
            return;
        }
        Err(_) => return, // queue closed before completion
    };

    let pages = chunk_vec(commits, LOG_PAGE_SIZE);
    let total = pages.len();
    // Lane state belongs to this stream (same rule as the local loop).
    let mut lanes = types::LaneState::default();
    for (index, commits) in pages.into_iter().enumerate() {
        if token.is_cancelled() || handle.is_closed() {
            break;
        }
        if handle.generation() != start_generation {
            break; // repo changed under us: abort, client re-requests
        }
        let next_cursor = if index + 1 < total {
            commits.last().map(|c| c.sha.clone())
        } else {
            None
        };
        let rows = types::layout_page(&commits, &mut lanes);
        if on_page
            .send(LogPage {
                commits,
                rows,
                next_cursor,
                generation: start_generation,
            })
            .is_err()
        {
            break; // client dropped the channel
        }
    }
}
