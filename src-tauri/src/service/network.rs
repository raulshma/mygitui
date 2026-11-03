use std::sync::Arc;

use tokio::sync::oneshot;

use crate::cli;
use crate::engine::git_engine::{EngineError, FetchProgress, PushProgress};
use crate::engine::types::{FetchOptions, NetStats, PullOptions, PushOptions, RemoteInfo, RepoId};
use crate::ops::OpCtx;
use crate::repo::RepoHandle;
use crate::service::{
    engine_err, enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read,
    finish_op, get_handle, AppServices,
};

pub async fn remotes(services: &AppServices, repo_id: RepoId) -> Result<Vec<RemoteInfo>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "fetch",
            move |git, _ws| git.remotes(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "fetch", move |_ctx, repo| {
        engine.remotes(repo)
    }))
    .await
}

pub async fn remote_add(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    url: String,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "fetch",
            move |git, _ws| git.remote_add(&name, &url),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "fetch",
        move |_ctx, repo| engine.remote_add(repo, &name, &url),
    ))
    .await
}

pub async fn remote_remove(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "fetch",
            move |git, _ws| git.remote_remove(&name),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "fetch",
        move |_ctx, repo| engine.remote_remove(repo, &name),
    ))
    .await
}

pub async fn remote_set_url(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    url: String,
    push: bool,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "fetch",
            move |git, _ws| git.remote_set_url(&name, &url, push),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "fetch",
        move |_ctx, repo| engine.remote_set_url(repo, &name, &url, push),
    ))
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

pub async fn fetch(
    services: &AppServices,
    repo_id: RepoId,
    options: FetchOptions,
) -> Result<NetStats, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Remote net op: same queue kind, opening line, and throttled
        // progress forwarding as the local net op (via `OpCtx`); the device
        // session brings its own SSH credentials, so no auth-broker context.
        let remote = Arc::clone(remote);
        let opening = format!("fetching {}", options.remote);
        let generation = handle.generation_shared();
        let (_op_id, rx) = handle.ops().enqueue("fetch", move |ctx| {
            ctx.emit_progress(&opening, None);
            let git = remote.git();
            let result = git.fetch(&options, &mut |p| emit_fetch_progress(&ctx, &p));
            generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            result
        });
        return finish_op(rx).await;
    }
    let repo = handle.repo()?;
    let opening = format!("fetching {}", options.remote);
    finish_op(enqueue_net_op(
        &handle,
        "fetch",
        opening,
        move |ctx, progress| {
            // Cancellation-aware ext path: `op_cancel` flips the ctx token
            // and the fetch polls it between phases and inside the transfer
            // callbacks. DELIBERATE `Libgit2Engine` exception to the dispatch
            // rule: the cancellation variants (`fetch_impl_ext`) take the
            // cancel token and are not on a trait — the engine is stateless,
            // so a fresh instance behaves identically to `handle.engine()`.
            let engine = crate::engine::libgit2::Libgit2Engine::new();
            let guard = repo.lock();
            engine.fetch_impl_ext(&guard, &options, progress, ctx.cancel_token().as_deref())
        },
    ))
    .await
}

pub async fn pull(
    services: &AppServices,
    repo_id: RepoId,
    options: PullOptions,
) -> Result<NetStats, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Remote net op (see `fetch`): queue + progress forwarding.
        let remote = Arc::clone(remote);
        let opening = format!("pulling {}", options.remote);
        let generation = handle.generation_shared();
        let (_op_id, rx) = handle.ops().enqueue("pull", move |ctx| {
            ctx.emit_progress(&opening, None);
            let git = remote.git();
            let result = git.pull(&options, &mut |p| emit_fetch_progress(&ctx, &p));
            generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            result
        });
        return finish_op(rx).await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
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

pub async fn push(
    services: &AppServices,
    repo_id: RepoId,
    options: PushOptions,
) -> Result<NetStats, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Remote net op (see `fetch`): queue + progress forwarding.
        let remote = Arc::clone(remote);
        let opening = if options.branch.is_empty() {
            format!("pushing to {}", options.remote)
        } else {
            format!("pushing {} to {}", options.branch, options.remote)
        };
        let generation = handle.generation_shared();
        let (_op_id, rx) = handle.ops().enqueue("push", move |ctx| {
            ctx.emit_progress(&opening, None);
            let git = remote.git();
            let result = git.push(&options, &mut |p| emit_push_progress(&ctx, &p));
            generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            result
        });
        return finish_op(rx).await;
    }
    let repo = handle.repo()?;
    let workdir = handle.root.clone();
    let generation = handle.generation_shared();
    let net_repo = handle.id.clone();
    let opening = if options.branch.is_empty() {
        format!("pushing to {}", options.remote)
    } else {
        format!("pushing {} to {}", options.branch, options.remote)
    };
    let (_op_id, rx) = handle.ops().enqueue("push", move |ctx| {
        ctx.emit_progress(&opening, None);
        // Cancellation-aware ext path (see fetch): op_cancel flips the ctx
        // token; push polls it between phases. Deliberate `Libgit2Engine`
        // exception to the dispatch rule — `push_impl_ext` is not on a
        // trait (same reason as fetch).
        let engine = crate::engine::libgit2::Libgit2Engine::new();
        let result = crate::auth::with_netop_context(&net_repo, || {
            let outcome = {
                let guard = repo.lock();
                engine.push_impl_ext(
                    &guard,
                    &options,
                    &mut |p| {
                        emit_push_progress(&ctx, &p);
                    },
                    ctx.cancel_token().as_deref(),
                )
            };
            match outcome {
                // libgit2 cannot run hooks: the engine asks for the CLI
                // (pre-push present). Stream the hook output into op-log.
                Err(EngineError::Unsupported(reason)) => {
                    ctx.emit_log(&format!("using git CLI: {reason}"));
                    cli::push_via_cli(&workdir, &options, &mut |line| ctx.emit_log(line))
                        .map_err(EngineError::Invalid)
                }
                other => other,
            }
        });
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        result.map_err(engine_err)
    });
    finish_op(rx).await
}

/// FE answer to an `auth-request` event: routes `(username, password,
/// store)` to the blocked credentials callback (auth broker).
pub fn auth_respond(
    _services: &AppServices,
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
    use crate::engine::types::StageRequest;
    use crate::repo::RepoManager;

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
