//! Service surface: the functions the iced UI calls in-process (contract in
//! docs/contracts.md).
//!
//! Engine calls run on the process-wide tokio runtime (`crate::rt`) because
//! git2 is synchronous; `EngineError`s surface as strings. Streams push
//! pages over the backend event bus ([`events::BackendEvent`]): log pages
//! of [`LOG_PAGE_SIZE`] commits laid out by the graph module, tagged with a
//! UI-minted request id. Streams stop early when nobody is subscribed (UI
//! gone) or the stream was superseded/cancelled; log streams abort when the
//! repo generation moves (the client re-requests).
//!
//! The function bodies live in the per-domain submodules below; this module
//! owns the cross-cutting plumbing (handle lookup, serial-queue enqueue
//! helpers, page chunking) and re-exports every service fn so UI call sites
//! can import from one place.

pub mod ai;
pub mod app_services;
pub mod branches;
pub mod diff;
pub mod events;
pub mod files;
pub mod forge;
pub mod log;
pub mod maintenance;
pub mod merge;
pub mod misc;
pub mod network;
pub mod repo;
pub mod stash;
pub mod worktree;

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::engine::git_engine::EngineError;
use crate::engine::types::RepoId;
use crate::ops::OpCtx;
use crate::repo::{RepoHandle, RepoManager};

pub use ai::*;
pub use app_services::AppServices;
pub use branches::*;
pub use diff::*;
pub use files::*;
pub use forge::*;
pub use log::*;
pub use maintenance::*;
pub use merge::*;
pub use misc::*;
pub use network::*;
pub use repo::*;
pub use stash::*;
// Intentionally empty for now (see the module docs): keeps the per-module
// re-export pattern uniform.
#[allow(unused_imports)]
pub use worktree::*;

pub(crate) fn get_handle(state: &RepoManager, repo_id: &RepoId) -> Result<Arc<RepoHandle>, String> {
    state
        .get(repo_id)
        .ok_or_else(|| format!("repo not open: {}", repo_id.0))
}

pub(crate) fn engine_err(err: EngineError) -> String {
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

// ---------------------------------------------------------------------------
// Mutations, branches/tags, remotes, net ops, auth — all routed through the
// per-repo serial op queue.
//
// DISPATCH RULE: `enqueue_mutation` / `enqueue_read` are the only sanctioned
// paths for engine ops. Every service fn that locks the repo and calls the
// engine goes through one of them (mutations bump the generation, reads do
// not). Deliberately off-queue: `stream_log` (its own cancellation loop and
// stream tokens), `mergetool_run` (interactive, minutes-long — see its
// comment), and the fns that never lock the repo (FS/CLI reads and spawns:
// maintenance probes, forge, clone, actions, pty).
//
// Progress: mutations and net ops stream `op-progress` (net ops forward
// engine callbacks to `OpCtx::emit_progress`, throttled ≥80ms) and the
// queue emits their final `done: true` event, success or error. Queue-
// dispatched reads are silent (`enqueue_read` → `OpQueue::enqueue_silent`):
// no progress, no done event — read failures surface only as the call's
// error return. Mutations bump the repo generation afterwards so in-flight
// log streams abort and clients re-request (streaming rules).
//
// Op kinds (`stage`, `commit`, `branch`, `fetch`, `pull`, `push`, `clone`,
// `merge`, `rebase`, `bisect`, `stash`, `worktree`, `checkpoint`,
// `submodule`, `maintenance`, `archive`, `sparse`, `lfs`, `clean`) label the
// progress events. Queue-dispatched reads carry the touched area as their
// queue kind (`status`, `refs`, `blame`, `diff`, `preview`, `stats`,
// `reflog`, `describe`) but emit no events (see Progress above). Tags
// report as `branch` (ref-affecting), remote config ops as `fetch` (net
// category).
// ---------------------------------------------------------------------------

/// Enqueue a mutating engine op on the repo's serial queue: `f` runs with
/// the repo lock held, the generation is bumped after it finishes (success
/// or error — hooks may have touched state even on failure), and the
/// command resolves through the returned receiver.
pub(crate) fn enqueue_mutation<T, F>(
    handle: &RepoHandle,
    repo: &Arc<Mutex<git2::Repository>>,
    kind: &'static str,
    f: F,
) -> oneshot::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce(OpCtx, &git2::Repository) -> Result<T, EngineError> + Send + 'static,
{
    let repo = repo.clone();
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

/// Enqueue a read-only engine op on the repo's serial queue: `f` runs with
/// the repo lock held, without the mutation generation bump. Silent on the
/// op-progress surface ([`OpQueue::enqueue_silent`]): reads emit no
/// progress/done events, exactly like the pre-queue direct reads — their
/// failures surface only through the command's error return. The kind
/// remains useful for queue rejection messages and debugging.
pub(crate) fn enqueue_read<T, F>(
    handle: &RepoHandle,
    repo: &Arc<Mutex<git2::Repository>>,
    kind: &'static str,
    f: F,
) -> oneshot::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce(OpCtx, &git2::Repository) -> Result<T, EngineError> + Send + 'static,
{
    let repo = repo.clone();
    let (_op_id, rx) = handle.ops().enqueue_silent(kind, move |ctx| {
        let guard = repo.lock();
        f(ctx, &guard).map_err(engine_err)
    });
    rx
}

/// Remote twin of [`enqueue_read`]: runs a remote-engine read on the repo's
/// serial op queue (silent: no progress events) without the git2 lock.
pub(crate) fn enqueue_remote_read<T, F>(
    handle: &RepoHandle,
    remote: &Arc<crate::repo::RemoteHandle>,
    kind: &'static str,
    f: F,
) -> oneshot::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce(
            &crate::remote::engine::RemoteGitEngine,
            &crate::remote::workspace::RemoteWorkspace,
        ) -> Result<T, String>
        + Send
        + 'static,
{
    let remote = Arc::clone(remote);
    let (_op_id, rx) = handle
        .ops()
        .enqueue_silent(kind, move |_ctx| f(&remote.git(), &remote.workspace()));
    rx
}

/// Remote twin of [`enqueue_mutation`]: serial queue + generation bump after
/// (success or error) so watcher/poller clients resync.
pub(crate) fn enqueue_remote_mutation<T, F>(
    handle: &RepoHandle,
    remote: &Arc<crate::repo::RemoteHandle>,
    kind: &'static str,
    f: F,
) -> oneshot::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce(
            &crate::remote::engine::RemoteGitEngine,
            &crate::remote::workspace::RemoteWorkspace,
        ) -> Result<T, String>
        + Send
        + 'static,
{
    let remote = Arc::clone(remote);
    let generation = handle.generation_shared();
    let (_op_id, rx) = handle.ops().enqueue(kind, move |_ctx| {
        let result = f(&remote.git(), &remote.workspace());
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        result
    });
    rx
}

/// Await an enqueued op's result (worker done or queue closed).
pub(crate) async fn finish_op<T>(rx: oneshot::Receiver<Result<T, String>>) -> Result<T, String> {
    rx.await
        .map_err(|_| "op queue closed before completion".to_string())?
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
