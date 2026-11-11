//! App self-update over the GitHub-releases updater flow (M12 wiring
//! completed: the plugins were registered in `lib.rs` from the start, this
//! module adds the IPC surface).
//!
//! The flow is three commands, deliberately split instead of one
//! check-and-install:
//!
//! 1. [`updater_check`] polls the endpoint (`latest.json` on this repo's
//!    releases) and returns `Some(UpdateInfo)` when a newer release exists.
//!    The plugin's [`Update`] handle is parked in [`PendingUpdate`] so the
//!    download installs *exactly* the release the user was shown, not
//!    whatever a re-check would return minutes later.
//! 2. [`updater_download_and_install`] streams the artifact (progress over
//!    a [`Channel`], throttled like the pty output coalescer) and runs the
//!    platform install. On Windows the NSIS/MSI installer exits the app by
//!    itself; on macOS/Linux the new version is installed in place.
//! 3. [`updater_relaunch`] restarts the process — never called
//!    automatically: killing the user's session unannounced is worse than
//!    one extra click on "Restart to update".
//!
//! Errors surface as strings (frontend displays them); the plugin's errors
//! (network, signature verification, config) all render readably via
//! `Display`.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_updater::UpdaterExt;
use ts_rs::TS;

/// Progress forwarder cadence (the pty coalescer's ≈one-display-frame
/// window): chunks closer together than this collapse into the next
/// emission, unless the integer percent moved (≥1% granularity).
const PROGRESS_EMIT_WINDOW: Duration = Duration::from_millis(16);

/// One poll result, mapped from the plugin's `Update` (the raw handle
/// carries the download URL/signature — never sent across IPC).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct UpdateInfo {
    /// New version announced by the release manifest.
    pub version: String,
    /// Version the running app was built with.
    pub current_version: String,
    /// Release notes (may hold markdown; the frontend renders plain text).
    pub notes: Option<String>,
    /// Publish date from the manifest, formatted for display.
    pub date: Option<String>,
}

/// One throttled download-progress emission (`updater_download_and_install`'s
/// channel payload). `total` is `None` when the server sent no
/// Content-Length — the frontend then shows an indeterminate bar.
#[derive(Debug, Clone, Copy, Serialize, TS)]
#[ts(export)]
pub struct UpdateProgress {
    /// Bytes received so far.
    pub downloaded: u64,
    /// Total bytes when known.
    pub total: Option<u64>,
}

/// The update found by the latest [`updater_check`], parked for the
/// subsequent [`updater_download_and_install`]. Swapped out on every check
/// (including the up-to-date one, which clears it) so a stale release can
/// never be installed after the user saw a newer one.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<tauri_plugin_updater::Update>>);

impl PendingUpdate {
    fn store(&self, update: tauri_plugin_updater::Update) {
        *self.0.lock().unwrap() = Some(update);
    }

    fn take(&self) -> Option<tauri_plugin_updater::Update> {
        self.0.lock().unwrap().take()
    }
}

/// Pure throttle decision at the heart of the progress forwarder: an
/// emission is due when the integer percent moved (a ≥1% change whenever
/// the total is known) or `elapsed` reached the emit window. With an
/// unknown total only the time rule applies. The download-finished
/// emission is always sent by the caller regardless of this verdict.
pub(crate) fn progress_due(
    elapsed: Duration,
    percent: Option<i64>,
    last_percent: Option<i64>,
) -> bool {
    if percent.is_some() && percent != last_percent {
        return true;
    }
    elapsed >= PROGRESS_EMIT_WINDOW
}

/// Mutable throttle state for one download (owned by the `on_chunk`
/// closure).
struct ProgressThrottle {
    last_emit: Instant,
    last_percent: Option<i64>,
}

impl ProgressThrottle {
    fn new() -> Self {
        Self {
            // The window starts at the download's first chunk, so the very
            // first progress always emits immediately.
            last_emit: Instant::now(),
            last_percent: None,
        }
    }

    /// Returns `Some(progress)` when this chunk should cross the channel.
    fn gate(&mut self, downloaded: u64, total: Option<u64>) -> Option<UpdateProgress> {
        let percent = total
            .filter(|t| *t > 0)
            .map(|t| ((downloaded.min(t) as f64 / t as f64) * 100.0).floor() as i64);
        if !progress_due(self.last_emit.elapsed(), percent, self.last_percent) {
            return None;
        }
        self.last_emit = Instant::now();
        self.last_percent = percent;
        Some(UpdateProgress { downloaded, total })
    }
}

/// Polls the updater endpoint. `Ok(None)` = running the latest version.
/// Also clears any stale [`PendingUpdate`] so "check" always resets the
/// cycle.
#[tauri::command(rename_all = "snake_case")]
pub async fn updater_check(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<Option<UpdateInfo>, String> {
    let update = app
        .updater()
        .map_err(|err| err.to_string())?
        .check()
        .await
        .map_err(|err| err.to_string())?;
    let Some(update) = update else {
        // Up to date — forget whatever an earlier check may have parked.
        *pending.0.lock().unwrap() = None;
        return Ok(None);
    };
    let info = UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
        date: update.date.map(|date| date.to_string()),
    };
    pending.store(update);
    Ok(Some(info))
}

/// Downloads the pending update (forwarding throttled progress on
/// `on_progress`) and runs the platform install. Platform notes from the
/// plugin: on Windows the installer exits the app when it succeeds; on
/// macOS/Linux the app keeps running until [`updater_relaunch`].
#[tauri::command(rename_all = "snake_case")]
pub async fn updater_download_and_install(
    pending: State<'_, PendingUpdate>,
    on_progress: Channel<UpdateProgress>,
) -> Result<(), String> {
    let Some(update) = pending.take() else {
        return Err("no pending update: run updater_check first".into());
    };
    let downloaded = Arc::new(AtomicU64::new(0));
    // The finish callback runs after the chunk callback is done with its
    // state, but both need the byte count — hence the atomic rather than a
    // shared `&mut`.
    let finished_bytes = Arc::downgrade(&downloaded);
    let mut throttle = ProgressThrottle::new();
    let forward = Arc::clone(&downloaded);
    // Both callbacks get their own channel clone (Channel is a cheap
    // shared handle).
    let on_finish = on_progress.clone();
    update
        .download_and_install(
            move |chunk, total| {
                let done = forward.fetch_add(chunk as u64, Ordering::Relaxed) + chunk as u64;
                if let Some(progress) = throttle.gate(done, total) {
                    let _ = on_progress.send(progress);
                }
            },
            move || {
                // The download stream ended: announce the final count so a
                // UI can snap to 100% even before the command resolves
                // (and before a Windows installer possibly exits the app).
                if let Some(count) = finished_bytes.upgrade() {
                    let _ = on_finish.send(UpdateProgress {
                        downloaded: count.load(Ordering::Relaxed),
                        total: None,
                    });
                }
            },
        )
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// Exits and relaunches the process (`AppHandle::restart` diverges — the
/// invoke never resolves because the webview goes away first).
#[tauri::command(rename_all = "snake_case")]
pub fn updater_relaunch(app: AppHandle) -> Result<(), String> {
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::{progress_due, ProgressThrottle, PROGRESS_EMIT_WINDOW};
    use std::time::Duration;

    #[test]
    fn percent_move_emits_even_inside_the_window() {
        assert!(progress_due(Duration::from_millis(1), Some(1), None));
        assert!(progress_due(Duration::from_millis(1), Some(2), Some(1)));
    }

    #[test]
    fn same_percent_waits_for_the_window() {
        assert!(!progress_due(Duration::from_millis(1), Some(5), Some(5)));
        assert!(progress_due(PROGRESS_EMIT_WINDOW, Some(5), Some(5)));
    }

    #[test]
    fn unknown_total_falls_back_to_time_only() {
        assert!(!progress_due(Duration::from_millis(5), None, None));
        assert!(progress_due(PROGRESS_EMIT_WINDOW, None, None));
    }

    #[test]
    fn gate_clamps_progress_at_100_and_tracks_percent() {
        let mut throttle = ProgressThrottle::new();
        // First chunk always emits (fresh window).
        let first = throttle.gate(1, Some(1000)).expect("first emits");
        assert_eq!(first.downloaded, 1);
        // Sub-poll same percent: suppressed.
        assert!(throttle.gate(2, Some(1000)).is_none());
        // Jump to 100% (server total smaller than streamed bytes): emits,
        // clamped. (`gate` sees only the running total the command closure
        // keeps; accumulation is not its job.)
        let last = throttle.gate(1500, Some(1000)).expect("percent move emits");
        assert_eq!(last.downloaded, 1500);
        assert_eq!(throttle.last_percent, Some(100));
    }
}
