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

/// Consecutive poll failures before the loop runs the ops' ensure/re-dial
/// path on the pooled session. Every exec already re-dials transparently,
/// but after repeated failures an explicit `ensure_connected` guarantees
/// the session heals (or reports Disconnected) instead of quietly failing
/// forever — the dial emits the same connecting/connected/disconnected
/// `device-status` transitions a user-triggered op would, so recovery is
/// visible without anything new on the wire.
const POLL_HEAL_AFTER_FAILURES: u32 = 3;

/// Path-list ceiling, mirroring the local watcher's
/// [`crate::watcher`] batch cap: beyond this the event still fires, the
/// remaining paths are just not enumerated.
const MAX_POLL_PATHS: usize = 2000;

/// One poll tick's outcome, computed by [`poll_event`] — kept pure so the
/// fingerprint→event semantics are unit-testable without a device session.
#[derive(Debug, PartialEq, Eq)]
pub struct PollEvent {
    pub head_moved: bool,
    pub full: bool,
    pub paths: Vec<String>,
}

/// Compare this tick's fingerprint against the previous one.
///
/// Flag semantics mirror the local watcher (`watcher.rs`): `head_moved`
/// means HEAD changed, `full` means "resync blind" — which the poller only
/// asserts on its baseline tick. Worktree churn is NOT a `full` resync:
/// `full` feeds `isSignificant`, which restarts the history log stream and
/// reloads the refs caches, so stamping every file save `full: true` made
/// busy remote workdirs kill and restart the log walk every poll — the
/// eternal "Streaming…" spinner. The churned paths are parsed from the two
/// porcelain snapshots so targeted consumers (StatusPanel's submodule
/// reload) still see what changed.
fn poll_event(last: Option<&(String, String)>, current: &(String, String)) -> PollEvent {
    let Some(last) = last else {
        // Baseline tick after repo open: one settling event, like the
        // watcher's initial flush.
        return PollEvent {
            head_moved: true,
            full: true,
            paths: Vec::new(),
        };
    };
    let head_moved = last.0 != current.0;
    if last.1 == current.1 {
        // HEAD-only movement (e.g. a commit with a clean tree): refs/HEAD
        // signal, no worktree paths to enumerate.
        return PollEvent {
            head_moved,
            full: false,
            paths: Vec::new(),
        };
    }
    let before: std::collections::HashMap<String, String> =
        status_entries(&last.1).into_iter().collect();
    let after_entries = status_entries(&current.1);
    let after: std::collections::HashMap<String, String> = after_entries.iter().cloned().collect();
    let mut paths: Vec<String> = Vec::new();
    for (path, line) in &after_entries {
        if before.get(path) != Some(line) && !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    for (path, line) in &before {
        if after.get(path) != Some(line) && !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    paths.truncate(MAX_POLL_PATHS);
    PollEvent {
        head_moved,
        full: false,
        paths,
    }
}

/// `(path, raw line)` per status entry in a `status --porcelain=v2 --branch
/// -uall` snapshot (header lines dropped). The raw line doubles as the
/// entry's state fingerprint: a path whose line is unchanged between polls
/// did not churn.
fn status_entries(status: &str) -> Vec<(String, String)> {
    status
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .flat_map(|line| {
            status_entry_paths(line)
                .into_iter()
                .map(move |path| (path, line.to_string()))
        })
        .collect()
}

/// The changed path(s) of one porcelain-v2 entry line: `1` (7 fixed tokens
/// then path), `2` (8 fixed tokens then `path\torigPath` — the ORIG side is
/// reported too so a renamed directory entry still matches its old path),
/// `u` (9 fixed tokens then path), `?` (untracked, path only).
fn status_entry_paths(line: &str) -> Vec<String> {
    let Some((kind, rest)) = line.split_once(' ') else {
        return Vec::new();
    };
    let tail = match kind {
        "?" => Some(rest),
        "1" => nth_token(rest, 7),
        "2" => nth_token(rest, 8),
        "u" => nth_token(rest, 9),
        _ => None,
    };
    let Some(tail) = tail else {
        return Vec::new();
    };
    let mut paths: Vec<String> = tail
        .split('\t')
        .map(unquote_path)
        .filter(|p| !p.is_empty())
        .collect();
    paths.dedup();
    if kind == "2" && paths.len() == 2 {
        // Report both ends of the rename; consumers match either.
        return paths;
    }
    paths.into_iter().take(1).collect()
}

/// The remainder after `n` space-separated tokens (porcelain paths are the
/// tail and may themselves contain spaces).
fn nth_token(rest: &str, n: usize) -> Option<&str> {
    let mut parts = rest.splitn(n + 1, ' ');
    let _ = parts.nth(n.checked_sub(1)?);
    parts.next()
}

/// Best-effort unquote of a C-quoted porcelain path (`"weird\tname"`).
fn unquote_path(path: &str) -> String {
    let Some(inner) = path.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
        return path.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
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
                let mut consecutive_failures: u32 = 0;
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
                            consecutive_failures = 0;
                            let changed = last.as_ref() != Some(&current);
                            if changed {
                                let event = poll_event(last.as_ref(), &current);
                                tracing::debug!(
                                    repo = %repo_id.0,
                                    head_moved = event.head_moved,
                                    full = event.full,
                                    paths = event.paths.len(),
                                    "remote repo changed (poll)"
                                );
                                generation.fetch_add(1, Ordering::Relaxed);
                                let batch = RepoChangedEvent {
                                    repo_id: repo_id.clone(),
                                    paths: event.paths,
                                    head_moved: event.head_moved,
                                    full: event.full,
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
                            consecutive_failures += 1;
                            tracing::debug!(repo = %repo_id.0, error = %err, "poll failed");
                            // Don't sit on a stale closed handle: after a
                            // few failures, run the same ensure/re-dial
                            // path the ops use so the pooled session heals
                            // (and the FE sees the status transition).
                            if consecutive_failures >= POLL_HEAL_AFTER_FAILURES {
                                consecutive_failures = 0;
                                let session = engine.session.clone();
                                if let Err(heal_err) =
                                    tauri::async_runtime::block_on(session.ensure_connected())
                                {
                                    tracing::debug!(
                                        repo = %repo_id.0,
                                        error = %heal_err,
                                        "poll re-dial failed"
                                    );
                                }
                            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic `status --porcelain=v2 --branch -uall` snapshot with
    /// every entry kind, headers, a space-bearing path and a quoted path.
    fn snapshot(head: &str, entries: &[&str]) -> (String, String) {
        let mut status = format!("# branch.oid {head}\n# branch.head main\n# branch.ab +0 -0\n");
        for entry in entries {
            status.push_str(entry);
            status.push('\n');
        }
        (head.to_string(), status)
    }

    #[test]
    fn baseline_tick_is_full_resync() {
        let current = snapshot("abc", &["1 .M N... 100644 100644 100644 abc def src/a.ts"]);
        let event = poll_event(None, &current);
        assert_eq!(
            event,
            PollEvent {
                head_moved: true,
                full: true,
                paths: Vec::new(),
            }
        );
    }

    #[test]
    fn worktree_churn_is_not_a_full_resync() {
        // A plain file save must not read as "refs/HEAD moved or resync
        // blind" — `full: true` here restarted the history log stream on
        // every poll (the eternal Streaming… spinner on busy remotes).
        let last = snapshot("abc", &["1 .M N... 100644 100644 100644 abc def src/a.ts"]);
        let current = snapshot("abc", &["1 .M N... 100644 100644 100644 abc 012 src/a.ts"]);
        let event = poll_event(Some(&last), &current);
        assert_eq!(
            event,
            PollEvent {
                head_moved: false,
                full: false,
                paths: vec!["src/a.ts".to_string()],
            }
        );
    }

    #[test]
    fn head_move_reports_head_moved_without_paths() {
        let last = snapshot("abc", &[]);
        let current = snapshot("def", &[]);
        let event = poll_event(Some(&last), &current);
        assert!(event.head_moved);
        assert!(!event.full);
        assert!(event.paths.is_empty());
    }

    #[test]
    fn head_move_plus_churn_reports_both() {
        let last = snapshot("abc", &["? notes.md"]);
        let current = snapshot("def", &["? notes.md", "? scratch.md"]);
        let event = poll_event(Some(&last), &current);
        assert!(event.head_moved);
        assert!(!event.full);
        assert_eq!(event.paths, vec!["scratch.md".to_string()]);
    }

    #[test]
    fn unchanged_entries_are_not_reported() {
        let last = snapshot(
            "abc",
            &[
                "1 .M N... 100644 100644 100644 abc def src/a.ts",
                "1 .. N... 100644 100644 100644 abc abc src/stable.ts",
                "? untracked.txt",
            ],
        );
        let current = snapshot(
            "abc",
            &[
                "1 M. N... 100644 100644 100644 abc 555 src/a.ts",
                "1 .. N... 100644 100644 100644 abc abc src/stable.ts",
                "? untracked.txt",
                "? new.txt",
            ],
        );
        let event = poll_event(Some(&last), &current);
        assert_eq!(
            event.paths,
            vec!["src/a.ts".to_string(), "new.txt".to_string()]
        );
    }

    #[test]
    fn rename_reports_both_ends() {
        let last = snapshot(
            "abc",
            &["1 .. N... 100644 100644 100644 abc abc old/one.rs"],
        );
        let current = snapshot(
            "abc",
            &["2 R. N... 100644 100644 100644 abc abc R100 new/one.rs\told/one.rs"],
        );
        let event = poll_event(Some(&last), &current);
        assert_eq!(
            event.paths,
            vec!["new/one.rs".to_string(), "old/one.rs".to_string(),]
        );
    }

    #[test]
    fn unmerged_and_space_paths_parse() {
        let entries = status_entries(
            "u AA N... 100644 100644 100644 100644 h1 h2 h3 src/conflict.rs\n\
             1 .M N... 100644 100644 100644 abc def docs/my file.md\n\
             ? \"quoted\\tname.txt\"\n\
             # branch.ab +1 -1\n",
        );
        let paths: Vec<&str> = entries.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            vec!["src/conflict.rs", "docs/my file.md", "quoted\tname.txt"]
        );
    }

    #[test]
    fn path_list_is_capped() {
        let entries: Vec<String> = (0..MAX_POLL_PATHS + 50)
            .map(|i| format!("? file{i}.txt"))
            .collect();
        let line_refs: Vec<&str> = entries.iter().map(String::as_str).collect();
        let last = snapshot("abc", &[]);
        let current = snapshot("abc", &line_refs);
        let event = poll_event(Some(&last), &current);
        assert!(!event.full);
        assert_eq!(event.paths.len(), MAX_POLL_PATHS);
    }
}
