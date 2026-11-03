use std::path::Path;

use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;

use crate::engine::types::{BlameLine, RepoId, RepoInfo, RepoStatus};
use crate::service::events::{global, BackendEvent};
use crate::service::AppServices;
use crate::service::{engine_err, enqueue_read, enqueue_remote_read, finish_op, get_handle};

/// Open (or reuse) the repository at/above `path` and start its watcher.
pub async fn repo_open(services: &AppServices, path: String) -> Result<RepoInfo, String> {
    services.repos.open(Path::new(&path)).map_err(engine_err)
}

/// Close a repository: stop its watcher, cancel its streams, drop caches.
pub fn repo_close(services: &AppServices, repo_id: RepoId) {
    services.repos.close(&repo_id);
}

pub async fn repo_status(services: &AppServices, repo_id: RepoId) -> Result<RepoStatus, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "status",
            move |git, _ws| git.status(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "status", move |_ctx, repo| {
        engine.status(repo)
    }))
    .await
}

/// Local + remote branches and tags: `(ref name, target sha)` pairs.
pub async fn repo_refs(
    services: &AppServices,
    repo_id: RepoId,
) -> Result<Vec<(String, String)>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "refs",
            move |git, _ws| git.refs(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "refs", move |_ctx, repo| {
        engine.refs(repo)
    }))
    .await
}

pub async fn repo_blame(
    services: &AppServices,
    repo_id: RepoId,
    path: String,
    from: Option<String>,
) -> Result<Vec<BlameLine>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "blame",
            move |git, _ws| git.blame(&path, from.as_deref()),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "blame", move |_ctx, repo| {
        engine.blame(repo, &path, from.as_deref())
    }))
    .await
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
/// (`depth`), publishing `clone-progress` events. Resolves with the path.
pub async fn repo_clone(
    _services: &AppServices,
    url: String,
    destination: String,
    depth: Option<u32>,
) -> Result<String, String> {
    crate::rt::spawn_blocking(move || clone_sync(&url, &destination, depth))
        .await
        .map_err(|e| format!("clone task failed: {e}"))?
}

fn clone_sync(url: &str, destination: &str, depth: Option<u32>) -> Result<String, String> {
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
            global().publish(BackendEvent::CloneProgress(payload));
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
