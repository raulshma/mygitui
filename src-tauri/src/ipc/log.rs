//! History streaming and commit metadata: `repo_log_stream` /
//! `repo_file_history` page commits over `tauri::ipc::Channel`s — log pages
//! of [`LOG_PAGE_SIZE`] commits laid out by the graph module — plus the
//! on-demand `repo_commit_message`, `reflog`, git notes (CLI lane), and the
//! M7 contribution statistics (`commit_activity` / `contributor_stats`).
//!
//! Streams stop early when the channel send fails (client gone) or the
//! stream was superseded/cancelled. A generation move does NOT truncate a
//! log walk: churn batches would otherwise kill and restart the walk on
//! every file save; freshness comes from the client restarting on
//! significant `repo-changed` events.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;
use ts_rs::TS;

use crate::engine::git_engine::EngineError;
use crate::engine::types::{CommitInfo, LogFilter, NoteInfo, ReflogEntry, RepoId};
use crate::graph::types::{self, GraphRow};
use crate::repo::{RepoHandle, RepoManager, StreamHandle, StreamKind};

use super::{
    enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read, finish_op,
    get_handle,
};

/// Commits per `repo_log_stream` / `repo_file_history` page.
const LOG_PAGE_SIZE: usize = 500;

/// Whole-walk cap for the remote log stream: the stream walks in
/// [`LOG_PAGE_SIZE`] pages (one SSH round-trip each, cancel checks between
/// pages) and stops hard at this many fetched commits. Generous bound
/// protects device + host memory.
const REMOTE_LOG_LIMIT: usize = 100_000;
/// Entry cap for the remote reflog read (the remote engine has no local
/// `name` argument; see the `reflog` command's remote branch).
const REMOTE_REFLOG_LIMIT: usize = 500;

/// One page of streamed history (contracts.md: `LogPage`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LogPage {
    pub commits: Vec<CommitInfo>,
    pub rows: Vec<GraphRow>,
    pub next_cursor: Option<String>,
    pub generation: u64,
}

/// Stream the commit log in [`LOG_PAGE_SIZE`] pages over `on_page`. Streams
/// carry PER-CONSUMER slots: a new call for the same repo+kind+consumer
/// replaces (cancels) only that consumer's previous stream, so concurrent
/// consumers (e.g. main history view and a popout detail) never cancel
/// each other. `consumer_id` absent → "default". The command resolves when
/// the walk ENDS — exhausted, superseded, or cancelled — so the caller can
/// tell a truncated walk from a complete one (the history store's
/// abort→restart contract relies on it; a plain spawn would resolve the
/// promise at open time and every open would read as an abort).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_log_stream(
    repo_id: RepoId,
    filter: LogFilter,
    consumer_id: Option<String>,
    on_page: Channel<LogPage>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let token = handle.begin_stream(StreamKind::Log, consumer_id.as_deref());
    tauri::async_runtime::spawn_blocking(move || stream_log(handle, filter, on_page, token))
        .await
        .map_err(|e| format!("log stream task failed: {e}"))?
}

/// Stream one file's history (follows renames) as [`LogPage`]s over
/// `on_page`. Resolves when the walk ends (see [`repo_log_stream`]).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_file_history(
    repo_id: RepoId,
    path: String,
    consumer_id: Option<String>,
    on_page: Channel<LogPage>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    let filter = LogFilter {
        path: Some(path),
        follow: true,
        ..LogFilter::default()
    };
    let token = handle.begin_stream(StreamKind::FileHistory, consumer_id.as_deref());
    tauri::async_runtime::spawn_blocking(move || stream_log(handle, filter, on_page, token))
        .await
        .map_err(|e| format!("history stream task failed: {e}"))?
}

/// Full commit message of one commit (on-demand: log pages carry only
/// `summary`). `None` when the sha does not resolve to a commit.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_commit_message(
    repo_id: RepoId,
    sha: String,
    state: State<'_, RepoManager>,
) -> Result<Option<String>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "commit_message",
            move |git, _ws| git.commit_message(&sha),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(
        &handle,
        &repo,
        "commit_message",
        move |_ctx, repo| engine.commit_message(repo, &sha),
    ))
    .await
}

/// Shared log/file-history streaming loop. Ends when the walk is exhausted,
/// the stream is cancelled/superseded, or the client drops the channel.
/// Cancellation ends silently (`Ok`) — the client re-requests on its own;
/// an engine error is surfaced (`Err`) so the UI can show it instead of
/// treating a broken walk as a mere abort.
///
/// There is NO generation-abort: every watcher batch bumps the generation,
/// including paths-only workdir churn the `isSignificant` filter ignores,
/// so aborting on a move killed and restarted the walk on every file save
/// (the eternal "Streaming…" spinner on busy workdirs). The revwalk is a
/// consistent snapshot: a ref move under it yields a complete-but-older
/// log, and the significant `repo-changed` event (head_moved/full)
/// restarts the stream for freshness. Pages carry their snapshot
/// `generation` for the client's stale-page gate.
fn stream_log(
    handle: Arc<RepoHandle>,
    filter: LogFilter,
    on_page: Channel<LogPage>,
    token: Arc<StreamHandle>,
) -> Result<(), String> {
    if let Some(remote) = handle.remote() {
        return stream_log_remote(&handle, remote.clone(), filter, on_page, token);
    }
    let Ok(repo) = handle.repo() else {
        return Ok(()); // unreachable: remote repos took the branch above
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
                    return Err(err.to_string());
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
    Ok(())
}

/// Remote twin of [`stream_log`]: the local walk's cursor loop over SSH
/// round-trips — one queued read per [`LOG_PAGE_SIZE`] page with cancel
/// checks between pages, so a slow device fetch never outruns a superseded
/// stream and memory stays bounded by one page instead of the whole walk.
/// Overall fetch volume is capped at [`REMOTE_LOG_LIMIT`] commits; reaching
/// the cap ends the walk (final page ships `next_cursor: null`, read as
/// exhausted). Graph rows come from the same pure
/// `graph::types::layout_page` with lane state carried across pages. There
/// is NO generation-abort (same rule as the local walk): the commit list of
/// each page was fully read before its send, so it is complete and self-
/// consistent even if the repo changed during the (multi-second,
/// churn-prone) SSH read — aborting would waste the entire read and just
/// restart the same race. Freshness is the client's job: significant
/// `repo-changed` events restart the stream, and pages carry their snapshot
/// `generation`. An EMPTY walk still ships one empty final page — only an
/// exhausted walk emits `next_cursor: null`, so a zero-page resolve would
/// read as an abort on the client (an endless restart loop for empty
/// remote repos).
fn stream_log_remote(
    handle: &Arc<RepoHandle>,
    remote: Arc<crate::repo::RemoteHandle>,
    filter: LogFilter,
    on_page: Channel<LogPage>,
    token: Arc<StreamHandle>,
) -> Result<(), String> {
    let start_generation = handle.generation();
    // Lane state belongs to this stream (same rule as the local loop).
    let mut lanes = types::LaneState::default();
    let mut cursor: Option<String> = None;
    let mut fetched = 0usize;
    loop {
        if token.is_cancelled() || handle.is_closed() {
            return Ok(());
        }
        let page_limit = LOG_PAGE_SIZE.min(REMOTE_LOG_LIMIT - fetched);
        let filter = filter.clone();
        let after = cursor.clone();
        let rx = enqueue_remote_read(handle, &remote, "log", move |git, _ws| {
            git.log(&filter, page_limit, after.as_deref())
        });
        let (commits, next_cursor) = match rx.blocking_recv() {
            Ok(Ok(result)) => result,
            Ok(Err(err)) => {
                tracing::warn!(
                    repo = %handle.id.0,
                    error = %err,
                    "remote log stream: engine error, aborting"
                );
                return Err(err);
            }
            Err(_) => return Ok(()), // queue closed before completion
        };
        // Cancel may have landed during the SSH read: stop before sending.
        if token.is_cancelled() || handle.is_closed() {
            return Ok(());
        }
        fetched += commits.len();
        // Hard stop at the overall cap: end as exhausted so a huge history
        // can never loop the device forever.
        let done = next_cursor.is_none() || fetched >= REMOTE_LOG_LIMIT;
        let next_cursor = if done { None } else { next_cursor };
        let rows = types::layout_page(&commits, &mut lanes);
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A paths-only watcher batch (workdir churn) bumps the generation but
    /// must NOT truncate the log walk: aborting on any generation move
    /// killed and restarted the walk on every file save — the eternal
    /// "Streaming…" spinner on busy workdirs. The bump lands
    /// deterministically mid-walk from the page callback (`send` is
    /// synchronous), i.e. between page 1 and page 2.
    #[test]
    fn log_walk_completes_despite_mid_walk_generation_bump() {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-ipc-churn-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        // More than LOG_PAGE_SIZE commits so the walk ships two pages and
        // the bump lands between them.
        {
            let sig = git2::Signature::now("t", "t@example.test").unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let mut parent: Option<git2::Commit> = None;
            for i in 0..=LOG_PAGE_SIZE {
                let parents: Vec<&git2::Commit> = parent.iter().collect();
                let id = repo
                    .commit(Some("HEAD"), &sig, &sig, &format!("c{i}"), &tree, &parents)
                    .unwrap();
                parent = Some(repo.find_commit(id).unwrap());
            }
        }
        drop(repo);

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&dir).unwrap();
        let handle = manager.get(&info.repo_id).unwrap();
        let generation = handle.generation_shared();
        let pages: Arc<parking_lot::Mutex<Vec<LogPage>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = pages.clone();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                let page: LogPage = serde_json::from_str(&json).unwrap();
                let mut guard = sink.lock();
                if guard.is_empty() {
                    // First page shipped: a churn batch lands mid-walk.
                    generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                guard.push(page);
            }
            Ok(())
        });
        let token = handle.begin_stream(StreamKind::Log, None);
        stream_log(handle.clone(), LogFilter::default(), channel, token).unwrap();

        let pages = pages.lock();
        assert!(
            pages.len() >= 2,
            "walk must page past the bump: {}",
            pages.len()
        );
        assert!(
            pages.last().unwrap().next_cursor.is_none(),
            "final page must prove exhaustion, not a silent abort"
        );
        let total: usize = pages.iter().map(|p| p.commits.len()).sum();
        assert_eq!(total, LOG_PAGE_SIZE + 1);
        manager.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reflog(
    repo_id: RepoId,
    name: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<Vec<ReflogEntry>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The remote engine reads HEAD's reflog only; a specific ref gets a
        // clear error instead of silently wrong data.
        if name
            .as_deref()
            .map(str::trim)
            .is_some_and(|n| !n.is_empty() && n != "HEAD")
        {
            return Err("reflog for a specific ref is not supported for remote projects".into());
        }
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "reflog",
            move |git, _ws| git.reflog(REMOTE_REFLOG_LIMIT),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "reflog", move |_ctx, repo| {
        engine.reflog(repo, name.as_deref())
    }))
    .await
}

// ---------- Phase 5: git notes (CLI lane) ----------

/// Every note attached to `sha`, one entry per notes ref that holds one
/// (`git notes` CLI edges; the local lane parses refs + shows each note).
#[tauri::command(rename_all = "snake_case")]
pub async fn commit_notes(
    repo_id: RepoId,
    sha: String,
    state: State<'_, RepoManager>,
) -> Result<Vec<NoteInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "notes",
            move |git, _ws| git.commit_notes(&sha),
        ))
        .await;
    }
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::maintenance::commit_notes(std::path::Path::new(&root), &sha)
    })
    .await
    .map_err(|e| format!("commit_notes task failed: {e}"))?
}

/// `git notes add [--ref <r>] <sha>` — attaches `text` (multi-line safe: it
/// travels via the commit-message temp-file helper, never a shell).
/// `note_ref` none/empty = the default `refs/notes/commits`.
#[tauri::command(rename_all = "snake_case")]
pub async fn note_add(
    repo_id: RepoId,
    sha: String,
    text: String,
    note_ref: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "notes",
            move |git, _ws| git.note_add(&sha, &text, note_ref.as_deref()),
        ))
        .await;
    }
    let root = handle.root.clone();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "notes",
        move |_ctx, _repo| {
            crate::maintenance::note_add(
                std::path::Path::new(&root),
                &sha,
                &text,
                note_ref.as_deref(),
            )
            .map_err(EngineError::cli)
        },
    ))
    .await
}

/// `git notes remove [--ref <r>] <sha>` — deletes the note under that ref
/// (default `refs/notes/commits`).
#[tauri::command(rename_all = "snake_case")]
pub async fn note_remove(
    repo_id: RepoId,
    sha: String,
    note_ref: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "notes",
            move |git, _ws| git.note_remove(&sha, note_ref.as_deref()),
        ))
        .await;
    }
    let root = handle.root.clone();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "notes",
        move |_ctx, _repo| {
            crate::maintenance::note_remove(std::path::Path::new(&root), &sha, note_ref.as_deref())
                .map_err(EngineError::cli)
        },
    ))
    .await
}

// ---------------------------------------------------------------------------
// M7 (lane I1): contribution statistics — commit activity heatmap buckets +
// contributor rollups (contracts.md "Commands (M7 stats)"). Queued reads on
// the op queue (`enqueue_read`, no generation bump); the logic lives in
// `engine/stats.rs` as inherent `*_impl` twins on `Libgit2Engine`, reached
// through the `GitEngineM12` forwarding (trash.rs hosts the single impl
// block).
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
