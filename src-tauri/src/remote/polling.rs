//! Remote repo change polling: the remote twin of the local FS watcher.
//!
//! There is no inotify over SSH, so each remote repo runs a poll thread
//! that snapshots `git status --porcelain` + HEAD on a timer (default 5s,
//! per-device configurable) and, on change, bumps the repo generation and
//! emits `repo-changed` — the exact pipeline the local watcher feeds, so
//! every panel refreshes identically. `--no-optional-locks` (in
//! `engine::git_command`) keeps the poll from ever writing the device's
//! index.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::Emitter;

use crate::engine::types::RepoId;
use crate::remote::engine::RemoteGitEngine;
use crate::watcher::{self, RepoChangedEvent};

/// Global pause flag: the FE flips it on `visibilitychange` (spec: polling
/// is "paused when tab hidden"). Paused pollers skip the probe execs but
/// keep their threads (and the last fingerprint) so resume is instant.
static POLL_PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Pause/resume every remote repo poller (see [`POLL_PAUSED`]).
pub fn set_paused(paused: bool) {
    POLL_PAUSED.store(paused, Ordering::SeqCst);
}

pub struct RepoPoller {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RepoPoller {
    /// Start polling one remote repo. The thread exits (and joins on drop)
    /// when the handle is dropped.
    pub fn start(
        repo_id: RepoId,
        generation: Arc<std::sync::atomic::AtomicU64>,
        engine: Arc<RemoteGitEngine>,
        interval: Duration,
        app: tauri::AppHandle,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name(format!("remote-poll-{}", repo_id.0))
            .spawn(move || {
                let mut last: Option<(String, String)> = None;
                loop {
                    // Sleep in slices so a close is honored within ~200ms.
                    let mut slept = Duration::ZERO;
                    while slept < interval {
                        if stop_flag.load(Ordering::SeqCst) {
                            return;
                        }
                        let slice = (interval - slept).min(Duration::from_millis(200));
                        std::thread::sleep(slice);
                        slept += slice;
                    }
                    if stop_flag.load(Ordering::SeqCst) {
                        return;
                    }
                    if POLL_PAUSED.load(Ordering::SeqCst) {
                        continue; // tab hidden: skip the probe, keep sleeping
                    }
                    let fingerprint = engine.change_fingerprint();
                    match fingerprint {
                        Ok(Some(current)) => {
                            let changed = last.as_ref() != Some(&current);
                            if changed {
                                let head_moved = last
                                    .as_ref()
                                    .map(|(old, _)| old != &current.0)
                                    .unwrap_or(true);
                                tracing::debug!(
                                    repo = %repo_id.0,
                                    head_moved,
                                    "remote repo changed (poll)"
                                );
                                generation.fetch_add(1, Ordering::Relaxed);
                                let batch = RepoChangedEvent {
                                    repo_id: repo_id.clone(),
                                    paths: Vec::new(),
                                    head_moved,
                                    full: true,
                                };
                                if let Err(err) = app.emit(watcher::REPO_CHANGED_EVENT, &batch) {
                                    tracing::warn!(error = %err, "emit repo-changed failed");
                                }
                            }
                            last = Some(current);
                        }
                        Ok(None) => {}
                        Err(err) => {
                            // Device unreachable: stay quiet, the session
                            // status broadcast covers the UX; keep the last
                            // fingerprint so a reconnect re-notifies.
                            tracing::debug!(repo = %repo_id.0, error = %err, "poll failed");
                        }
                    }
                }
            })
            .map_err(|e| format!("spawn poller: {e}"))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for RepoPoller {
    fn drop(&mut self) {
        self.stop();
    }
}
