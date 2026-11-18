//! Network lane: fetch/pull/push (queued net ops with credential routing +
//! throttled progress), the git2 `repo_clone` with `clone-progress` events,
//! the CLI `clone_blobless`, and the FE's `auth_respond` answer to
//! `auth-request` events.
//!
//! Net ops must not touch the shared long-lived repo handle ([`
//! open_net_handle`]); credentials are attributed to the repo through
//! `auth::with_netop_context`, and the CLI fallbacks (unshallow fetch,
//! pre-push hooks) poll the op's cancel token so `op_cancel` can kill a
//! hung child.

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use tokio::sync::oneshot;
use ts_rs::TS;

use crate::cli;
use crate::engine::git_engine::{EngineError, FetchProgress, PushProgress};
use crate::engine::types::{FetchOptions, NetStats, PullOptions, PushOptions, RepoId};
use crate::ops::OpCtx;
use crate::repo::{RepoHandle, RepoManager};

use super::{engine_err, finish_op, get_handle};

// ---------------------------------------------------------------------------
// Clone (B4 lane, appended): `repo_clone` + `clone-progress` events.
//
// Clones a remote URL into a local destination with git2's `RepoBuilder`
// (fetch options + transfer-progress callback), emitting throttled
// `clone-progress` events `{url, received, total, objects, run_id}` over
// the app emitter. `received` counts bytes; `total`/`objects` count git objects —
// libgit2's transfer progress has no total-bytes estimate, so the FE bar
// fills by objects. Pure helpers (`clone_emit_ready`,
// `clone_progress_event`, `validate_clone_destination`) are unit-tested
// without touching the network.
// ---------------------------------------------------------------------------

use parking_lot::Mutex;
use tauri::Emitter;

/// `clone-progress` event payload (TS name `CloneProgressEvent`; generated
/// to `src/lib/ipc/gen/` and re-exported under the FE name by types.ts).
#[derive(Debug, Clone, Serialize, PartialEq, Eq, TS)]
#[ts(export)]
pub struct CloneProgress {
    /// URL of the clone this progress belongs to (multi-clone discriminator).
    pub url: String,
    /// Bytes received so far.
    pub received: usize,
    /// Total objects the remote announced (0 = not yet known).
    pub total: usize,
    /// Objects received so far.
    pub objects: usize,
    /// FE-minted run id (`repo_clone`'s `run_id` argument) — the strict
    /// multi-clone discriminator (URLs repeat; run ids do not). `None`
    /// only for callers predating the argument.
    pub run_id: Option<String>,
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
fn clone_progress_event(url: &str, run_id: Option<&str>, stats: &CloneStats) -> CloneProgress {
    CloneProgress {
        url: url.to_string(),
        received: stats.received_bytes,
        total: stats.total_objects,
        objects: stats.received_objects,
        run_id: run_id.map(String::from),
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
    // FE-minted correlation id echoed in every `clone-progress` event for
    // this run (additive argument; `None` keeps URL-keyed matching).
    run_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        clone_sync(&url, &destination, depth, run_id.as_deref(), &app)
    })
    .await
    .map_err(|e| format!("clone task failed: {e}"))?
}

fn clone_sync(
    url: &str,
    destination: &str,
    depth: Option<u32>,
    run_id: Option<&str>,
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
    // Same credential path as fetch/push (`engine/netops.rs`): the broker's
    // chain covers HTTPS (cache → keyring → git credential helpers → FE
    // prompt) and SSH (agent → passphrase prompt). No repo exists yet, so
    // auth-request events carry an empty repo_id (no `with_netop_context`).
    callbacks.credentials(crate::auth::broker().credentials());
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
            let payload = clone_progress_event(&event_url, run_id, &stats);
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
        let event = clone_progress_event("https://host/repo.git", None, &stats);
        assert_eq!(
            event,
            CloneProgress {
                url: "https://host/repo.git".to_string(),
                received: 4096,
                total: 42,
                objects: 10,
                run_id: None,
            }
        );
        // With a run id it is echoed verbatim (the strict discriminator).
        let identified = clone_progress_event("https://host/repo.git", Some("clone-1"), &stats);
        assert_eq!(identified.run_id.as_deref(), Some("clone-1"));
        // Serializes with the documented field names.
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["url"], "https://host/repo.git");
        assert_eq!(json["received"], 4096);
        assert_eq!(json["total"], 42);
        assert_eq!(json["objects"], 10);
        assert_eq!(json["run_id"], serde_json::Value::Null);
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

/// Fresh repository handle for a network op. Net ops must not touch the
/// shared long-lived handle: holding its mutex across a transfer would
/// stall every status/log/diff/blame refresh for the repo until the network
/// finishes. Git's own file locking keeps concurrent ref/object writes
/// (fetch here, user commit on the shared handle) safe.
fn open_net_handle(workdir: &Path) -> Result<git2::Repository, EngineError> {
    git2::Repository::open(workdir).map_err(EngineError::from)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn fetch(
    repo_id: RepoId,
    options: FetchOptions,
    state: State<'_, RepoManager>,
) -> Result<NetStats, String> {
    let handle = get_handle(&state, &repo_id)?;
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
    let workdir = handle.root.clone();
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
            let outcome = open_net_handle(&workdir).and_then(|net_repo| {
                engine.fetch_impl_ext(&net_repo, &options, progress, ctx.cancel_token().as_deref())
            });
            match outcome {
                // libgit2 cannot unshallow (`FetchOptions.unshallow`): the
                // engine asks for the CLI, same routing as the atomic push.
                // Stream the fetch report into op-log.
                Err(EngineError::Unsupported(reason)) => {
                    ctx.emit_log(&format!("using git CLI: {reason}"));
                    let cancel = ctx.cancel_token();
                    cli::fetch_via_cli(
                        &workdir,
                        &options,
                        &mut |line| ctx.emit_log(line),
                        cancel.as_deref(),
                    )
                    .map_err(EngineError::cli)
                }
                other => other,
            }
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
    let workdir = handle.root.clone();
    let opening = format!("pulling {}", options.remote);
    finish_op(enqueue_net_op(
        &handle,
        "pull",
        opening,
        move |ctx, progress| {
            // Cancellation-aware ext path (see fetch): `op_cancel` flips the
            // ctx token and pull polls it inside the fetch transfer callbacks
            // and between the fetch + merge phases. DELIBERATE `Libgit2Engine`
            // exception to the dispatch rule: the cancellation variants
            // (`pull_impl_ext`) take the cancel token and are not on a trait —
            // the engine is stateless, so a fresh instance behaves identically
            // to `handle.engine()`.
            let engine = crate::engine::libgit2::Libgit2Engine::new();
            let net_repo = open_net_handle(&workdir)?;
            engine.pull_impl_ext(&net_repo, &options, progress, ctx.cancel_token().as_deref())
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
            let outcome = open_net_handle(&workdir).and_then(|net_handle| {
                engine.push_impl_ext(
                    &net_handle,
                    &options,
                    &mut |p| {
                        emit_push_progress(&ctx, &p);
                    },
                    ctx.cancel_token().as_deref(),
                )
            });
            match outcome {
                // libgit2 cannot run hooks: the engine asks for the CLI
                // (pre-push present). Stream the hook output into op-log.
                Err(EngineError::Unsupported(reason)) => {
                    ctx.emit_log(&format!("using git CLI: {reason}"));
                    // Cancel-aware CLI path (see git_stream): the polled
                    // child wait kills a hung pre-push hook on op_cancel.
                    let cancel = ctx.cancel_token();
                    cli::push_via_cli(
                        &workdir,
                        &options,
                        &mut |line| ctx.emit_log(line),
                        cancel.as_deref(),
                    )
                    .map_err(EngineError::cli)
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
}

/// Argument guard for the CLI clone, same family as [`archive_arg_ok`]
/// (the check itself lives in `maintenance` next to the other CLI-lane
/// guards, shared with `submodule_add`).
fn clone_args_ok(url: &str, destination: &str) -> bool {
    crate::maintenance::clone_args_ok(url, destination)
}

/// Blobless (partial) clone via the git CLI: `git clone --filter=blob:none`.
#[tauri::command(rename_all = "snake_case")]
pub async fn clone_blobless(
    url: String,
    destination: String,
    depth: Option<u32>,
) -> Result<String, String> {
    if !clone_args_ok(&url, &destination) {
        return Err("invalid clone arguments".into());
    }
    let parent = std::path::Path::new(&destination)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    tauri::async_runtime::spawn_blocking(move || {
        let mut args: Vec<String> = vec![
            "clone".into(),
            "--filter=blob:none".into(),
            url.clone(),
            destination.clone(),
        ];
        if let Some(depth) = depth {
            args.insert(2, format!("--depth={depth}"));
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        crate::maintenance::git_run(&parent, &arg_refs)?;
        Ok(destination)
    })
    .await
    .map_err(|e| format!("clone_blobless task failed: {e}"))?
}

#[cfg(test)]
mod clone_blobless_tests {
    use super::clone_args_ok;

    #[test]
    fn url_guard_rejects_option_shaped_and_metacharacters() {
        assert!(clone_args_ok("https://example.com/repo.git", "/tmp/dest"));
        assert!(clone_args_ok("git@github.com:me/repo.git", "dest"));
        // A leading dash would be parsed as a git flag (option injection).
        assert!(!clone_args_ok("--upload-pack=touch /tmp/x", "dest"));
        assert!(!clone_args_ok("-ufoo", "dest"));
        // The original metacharacter contract stays.
        assert!(!clone_args_ok("https://example.com/x.git;rm -rf /", "dest"));
        assert!(!clone_args_ok("", "dest"));
        assert!(!clone_args_ok("https://example.com/repo.git", ""));
    }
}
