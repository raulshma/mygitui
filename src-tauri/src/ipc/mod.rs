//! M1 IPC command surface (names/shapes fixed by docs/contracts.md), split
//! into per-area modules:
//!
//! - [`status`] — repo lifecycle + status/refs
//! - [`log`] — log/file-history streams, commit metadata, notes, stats
//! - [`diff`] — diff/blob/blame reads
//! - [`net`] — fetch/pull/push/clone/auth answers
//! - [`mutations`] — stage/commit/discard/checkpoints/clean/trash
//! - [`merge`] — merge/rebase/bisect/conflicts/mergetool
//! - [`stash`] — stash + worktrees
//! - [`health`] — maintenance/health/sparse/LFS/archive/bundle
//! - [`branches`] — branches/tags/remotes/submodules
//! - [`workspace`] — .gitignore quick-add
//! - [`forge`] — gh-assisted GitHub flow
//! - [`issues`] — GitHub Issues REST workspace
//! - [`misc`] — CLI args, accent color, actions, project detection
//!
//! Engine calls run on the blocking pool (`tauri::async_runtime`) because
//! git2 is synchronous; `EngineError`s surface as the structured error
//! envelope via [`engine_err`]. The shared op-queue helpers below are the
//! only sanctioned paths for engine ops.

pub(crate) mod branches;
pub(crate) mod diff;
pub(crate) mod forge;
pub(crate) mod health;
pub(crate) mod issues;
pub(crate) mod log;
pub(crate) mod merge;
pub(crate) mod misc;
pub(crate) mod mutations;
pub(crate) mod net;
pub(crate) mod stash;
pub(crate) mod status;
pub(crate) mod workspace;

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::engine::git_engine::{CheckoutProgress, EngineError};
use crate::engine::types::{IpcErrorEnvelope, RepoId};
use crate::forge::ForgeRepo;
use crate::ops::OpCtx;
use crate::repo::{RepoHandle, RepoManager};

/// Shared forge execution context. Local repositories run gh in their workdir;
/// device repositories run gh and git on the device with its own auth.
pub(crate) fn forge_repo_of(handle: &Arc<RepoHandle>) -> ForgeRepo {
    ForgeRepo {
        root: handle.root.clone(),
        remote: handle.remote().map(Arc::clone),
    }
}

/// Resolve an open repo's handle or fail the command with the standard
/// "repo not open" error.
pub(crate) fn get_handle(state: &RepoManager, repo_id: &RepoId) -> Result<Arc<RepoHandle>, String> {
    state
        .get(repo_id)
        .ok_or_else(|| format!("repo not open: {}", repo_id.0))
}

/// Engine-originated failures cross the boundary as the structured error
/// envelope (a JSON string with the `mygitui_error` marker — see
/// docs/contracts.md "Errors"); the FE rebuilds a typed `IpcError` from it.
/// Command signatures stay `Result<T, String>`; misc commands that format
/// raw strings (`.map_err(|e| e.to_string())`) keep them raw and the FE
/// falls back to its text heuristics.
pub(crate) fn engine_err(err: EngineError) -> String {
    IpcErrorEnvelope::from(&err).to_json()
}

// ---------------------------------------------------------------------------
// M2 (lane C3): mutations, branches/tags, remotes, net ops, auth — all
// routed through the per-repo serial op queue (contracts.md M2).
//
// DISPATCH RULE: `enqueue_mutation` / `enqueue_read` are the only sanctioned
// paths for engine ops. Every handler that locks the repo and calls the
// engine goes through one of them (mutations bump the generation, reads do
// not). Deliberately off-queue: `stream_log` (its own cancellation loop and
// stream tokens), `mergetool_run` (interactive, minutes-long — see its
// comment), and the commands that never lock the repo (FS/CLI reads and
// spawns: maintenance probes, forge, clone, actions, pty).
//
// Progress: mutations and net ops stream `op-progress` (net ops forward
// engine callbacks to `OpCtx::emit_progress`, throttled ≥80ms) and the
// queue emits their final `done: true` event, success or error. Queue-
// dispatched reads are silent (`enqueue_read` → `OpQueue::enqueue_read_lane`):
// no progress, no done event — read failures surface only as command
// errors, matching the pre-queue direct-call reads. Mutations bump the
// repo generation afterwards so in-flight log streams abort and clients
// re-request (streaming rules).
//
// Op kinds started as the fixed M2 set from contracts.md (`stage`, `commit`,
// `branch`, `fetch`, `pull`, `push`, `clone`); later lanes added their own
// (`merge`, `rebase`, `bisect`, `stash`, `worktree`, `checkpoint`,
// `submodule`, `maintenance`, `archive`, `sparse`, `lfs`, `clean`). Queue-
// dispatched reads carry the touched area as their queue kind (`status`,
// `refs`, `blame`, `diff`, `preview`, `stats`, `reflog`, `describe`) but
// emit no events (see Progress above). Tags report as `branch`
// (ref-affecting), remote config ops as `fetch` (net category).
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

/// Forward checkout/reset workdir progress ([`CheckoutProgress`], libgit2's
/// per-path callback) onto the op's throttled `op-progress` surface: the
/// phase is the message, completed/total the percentage. Throttling lives
/// in [`OpCtx::emit_progress`] (≥80 ms), matching fetch/push progress.
pub(crate) fn emit_checkout_progress(ctx: &OpCtx, progress: CheckoutProgress) {
    let pct = if progress.total > 0 {
        Some(progress.completed as f64 * 100.0 / progress.total as f64)
    } else {
        None
    };
    ctx.emit_progress(progress.phase, pct);
}

/// Enqueue a read-only engine op on the repo's read lane: `f` runs with the
/// repo lock held, without the mutation generation bump, on the queue's
/// second (read) worker — a long mutation or net op never queues reads
/// behind it (the repo mutex still serializes reads against mutations).
/// Silent on the op-progress surface ([`OpQueue::enqueue_read_lane`]): reads
/// emit no progress/done events, exactly like the pre-queue direct reads —
/// their failures surface only through the command's error return. The kind
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
    let (_op_id, rx) = handle.ops().enqueue_read_lane(kind, move |ctx| {
        let guard = repo.lock();
        f(ctx, &guard).map_err(engine_err)
    });
    rx
}

/// Remote twin of [`enqueue_read`]: runs a remote-engine read on the repo's
/// read lane (silent: no progress events) without the git2 lock.
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
        .enqueue_read_lane(kind, move |_ctx| f(&remote.git(), &remote.workspace()));
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
mod m2_tests {
    use super::*;
    use crate::engine::types::StageRequest;

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

    /// Reads stay invisible to the op-progress surface: `enqueue_read` runs
    /// on the queue (serialization, no generation bump) but emits no events,
    /// success or error — matching the pre-queue direct reads, whose
    /// failures surface only through the command's error return.
    #[test]
    fn enqueue_read_emits_no_events_success_or_error() {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-ipc-read-silent-{}-{}",
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
        let repo = handle.repo().expect("local repo");
        let ok = enqueue_read(&handle, &repo, "status", move |_ctx, _repo| {
            Ok::<(), EngineError>(())
        })
        .blocking_recv()
        .expect("queue alive");
        assert!(ok.is_ok(), "read succeeds");
        let failed = enqueue_read(&handle, &repo, "blame", move |_ctx, _repo| {
            Err::<(), EngineError>(EngineError::Invalid("missing file".into()))
        })
        .blocking_recv()
        .expect("queue alive");
        assert!(failed.is_err(), "failing read reports through the command");
        assert_eq!(
            handle.generation(),
            start_generation,
            "reads never bump the generation"
        );
        assert!(
            sink.snapshot().is_empty(),
            "reads emit no op-progress events: {:?}",
            sink.snapshot()
        );

        manager.close(&info.repo_id);
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
            let repo = handle.repo().expect("local repo");
            enqueue_mutation(&handle, &repo, "stage", move |_ctx, repo| {
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
