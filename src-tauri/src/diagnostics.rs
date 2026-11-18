//! Diagnostics: the rotating file log sink and the `diagnostics_bundle`
//! report.
//!
//! **File logging (66a)**: alongside the stderr layer (`tauri dev` console),
//! the tracing subscriber installs a second `fmt` layer writing plain lines
//! to `<app-data>/logs/mygitui.log` (the same app-data root the remote
//! device store lives under). Rotation is a size check on write: at 5 MiB
//! the current file is renamed to `mygitui.log.1` (replacing the previous
//! one), so at most current + one previous exist. Both layers share ONE
//! env-filter (`RUST_LOG` wins; `info` by default) on one registry
//! subscriber. The subscriber is built in `setup` (the app-data dir needs
//! the app handle); a log dir that cannot be opened disables the file layer
//! instead of ever taking the app down.
//!
//! **Report (66b)**: `diagnostics_bundle(repo_id?)` assembles
//! [`DiagnosticsReport`]: app version, OS + arch, per-open-repo runtime
//! facts (root, watcher mode, op-queue depth, device id), live terminal
//! session count, the caught-panic count (`ops::panics_caught`), connected
//! remote device ids, and the last 50 lines of the log file. There is no
//! backend-side op-log ring (the `EventSink` forwards `op-log` lines to the
//! FE and keeps no history) — the log tail covers recent warnings/errors
//! backend-side instead.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::Manager as _;
use ts_rs::TS;

use crate::engine::types::RepoId;

/// Rotation threshold: keep current + one previous at this size.
pub const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Rotated (previous) log file suffix: `mygitui.log` → `mygitui.log.1`.
const LOG_PREVIOUS_SUFFIX: &str = "log.1";

/// How many log lines the report tails.
pub const LOG_TAIL_LINES: usize = 50;

/// The log file actually written by [`init_tracing`] (`None` before setup).
/// The report prefers this over re-resolving so the path always matches
/// what the file layer opened.
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Resolved log file path for the report: the one armed at init when
/// present, else a fresh resolution (identical computation).
fn current_log_path(app: &tauri::AppHandle) -> PathBuf {
    LOG_PATH
        .get()
        .cloned()
        .unwrap_or_else(|| resolve_log_path(app))
}

/// `<app-data>/logs/mygitui.log` — the same app-data root the remote device
/// store (`remote/devices.json`) persists under.
pub fn resolve_log_path(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("logs")
        .join("mygitui.log")
}

/// The previous (rotated) generation of the log file.
fn previous_log_path(current: &Path) -> PathBuf {
    current.with_extension(LOG_PREVIOUS_SUFFIX)
}

// ---------------------------------------------------------------------------
// 66a: rotating file log sink
// ---------------------------------------------------------------------------

/// Append-only log writer with one-generation rotation. `tracing`'s fmt
/// layer hands it one formatted line per event via [`std::io::Write`];
/// every failure (dir creation, open, write) disables the layer instead of
/// surfacing an error — the log file must never take the app down.
pub struct RotatingLogWriter {
    path: PathBuf,
    max_bytes: u64,
    state: Mutex<FileState>,
}

#[derive(Default)]
struct FileState {
    /// Open append handle; `None` until the first line (lazy: the log dir
    /// is created on first write, not at subscriber build time).
    file: Option<std::fs::File>,
    /// Current file size, tracked incrementally to keep the rotation check
    /// allocation-free.
    size: u64,
    /// Set after the first hard failure: no further attempts this session
    /// (a broken log dir must never hot-loop on every log line).
    disabled: bool,
}

impl RotatingLogWriter {
    pub fn new(path: PathBuf) -> Self {
        Self::with_max_bytes(path, LOG_MAX_BYTES)
    }

    /// Test seam: same writer with a tiny rotation threshold.
    fn with_max_bytes(path: PathBuf, max_bytes: u64) -> Self {
        Self {
            path,
            max_bytes,
            state: Mutex::new(FileState::default()),
        }
    }

    /// Append one line, rotating first when the size budget is spent.
    fn write_line(&self, buf: &[u8]) {
        let mut state = self.state.lock();
        if state.disabled {
            return;
        }
        if state.file.is_none() && !open_append(&self.path, &mut state) {
            return; // disabled inside open_append
        }
        if state.size >= self.max_bytes {
            self.rotate(&mut state);
            if state.file.is_none() {
                return; // reopen after rotation failed: disabled
            }
        }
        let file = state
            .file
            .as_mut()
            .expect("open_append/rotate leave a handle");
        match file.write_all(buf).and_then(|()| file.flush()) {
            Ok(()) => state.size += buf.len() as u64,
            Err(_) => {
                state.file = None;
                state.disabled = true;
            }
        }
    }

    /// Close the current file, rename it to `mygitui.log.1` (replacing the
    /// previous generation — at most current + one previous exist), and
    /// open a fresh current file. Every step is best-effort: a failed
    /// rotation keeps appending to the oversized file rather than losing
    /// logs.
    fn rotate(&self, state: &mut FileState) {
        state.file = None; // close the handle before the rename (Windows)
        let previous = previous_log_path(&self.path);
        let _ = std::fs::remove_file(&previous);
        let _ = std::fs::rename(&self.path, &previous);
        let _ = open_append(&self.path, state);
    }
}

/// Open `path` for appending, creating parent dirs; tracks the size and
/// disables the writer on failure.
fn open_append(path: &Path, state: &mut FileState) -> bool {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(file) => {
            state.size = file.metadata().map(|m| m.len()).unwrap_or(0);
            state.file = Some(file);
            true
        }
        Err(_) => {
            state.disabled = true;
            false
        }
    }
}

/// `MakeWriter` adapter: the fmt layer holds the shared writer and asks for
/// a `Write` per event; write errors are swallowed inside `write_line` (the
/// layer drops IO errors silently, but a poisoned file must not surface).
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RotatingLogWriter {
    type Writer = RotatingWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        RotatingWriter(self)
    }
}

/// Per-event `Write` handle handed out by the fmt layer; the writer state
/// lives in the shared [`RotatingLogWriter`].
pub struct RotatingWriter<'a>(&'a RotatingLogWriter);

impl std::io::Write for RotatingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write_line(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Build the process subscriber: stderr layer (dev console) + rotating file
/// layer, both behind ONE env filter (`RUST_LOG` wins; `info` by default —
/// the same contract the stderr-only setup had). Called from `setup` once
/// the app handle exists; a previously installed subscriber wins silently.
pub fn init_tracing(app: &tauri::AppHandle) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,mygitui_lib=info"));
    let log_path = resolve_log_path(app);
    let _ = LOG_PATH.set(log_path.clone());
    // stderr keeps the compact shape the stderr-only setup used (no
    // targets); the file layer keeps targets — the report tail benefits.
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(RotatingLogWriter::new(log_path))
        .with_ansi(false);
    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
        .ok();
}

// ---------------------------------------------------------------------------
// 66b: diagnostics report
// ---------------------------------------------------------------------------

/// Runtime facts for one open repo (`DiagnosticsReport.repos`).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RepoDiagnostics {
    pub repo_id: String,
    /// Absolute workdir root.
    pub root: String,
    /// True when the repo lives on a remote device (no FS watcher: the
    /// poller replaces it, `watch_mode` is `null`).
    pub remote: bool,
    /// Device id for remote repos (`null` for local).
    pub device_id: Option<String>,
    /// Local FS watcher backend (`"recommended"` | `"poll"` | `"dead"`),
    /// `null` for remote repos.
    pub watch_mode: Option<crate::watcher::WatchMode>,
    /// Ops accepted but not yet finished (queued on either lane + running).
    pub op_queue_depth: u64,
}

/// `diagnostics_bundle` payload (ts-rs → `src/lib/ipc/gen/`).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiagnosticsReport {
    /// Running app version (tauri package info).
    pub app_version: String,
    /// Target OS (`std::env::consts::OS`).
    pub os: String,
    /// Target arch (`std::env::consts::ARCH`).
    pub arch: String,
    /// One entry per open repo, unordered. `repo_id` scopes the report to
    /// that repo; absent → every open repo.
    pub repos: Vec<RepoDiagnostics>,
    /// Live embedded terminal sessions.
    pub pty_sessions: usize,
    /// Ops caught panicking since launch (op-queue guards).
    pub op_panics: u64,
    /// Devices with a live pooled session (unordered).
    pub remote_device_ids: Vec<String>,
    /// The log file the file layer writes (empty string when logging never
    /// armed).
    pub log_path: String,
    /// Last 50 lines of the log file, oldest first (empty when missing).
    pub log_tail: Vec<String>,
}

/// Last `n` lines of `path`, oldest first. Best-effort: a missing or
/// undecodable file tails to nothing.
pub fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut tail: Vec<&str> = content.lines().rev().take(n).collect();
    tail.reverse();
    tail.into_iter().map(str::to_owned).collect()
}

/// Per-repo facts from a handle.
fn repo_diagnostics(handle: &std::sync::Arc<crate::repo::RepoHandle>) -> RepoDiagnostics {
    RepoDiagnostics {
        repo_id: handle.id.0.clone(),
        root: handle.root.to_string_lossy().into_owned(),
        remote: handle.is_remote(),
        device_id: handle.device_id(),
        watch_mode: handle.watch_mode(),
        op_queue_depth: handle.ops().depth(),
    }
}

/// Assemble the report (the tauri-free core, unit-tested): everything the
/// command collects is passed in, the log tail is read from `log_path`.
fn build_report(
    app_version: String,
    repos: Vec<RepoDiagnostics>,
    pty_sessions: usize,
    op_panics: u64,
    remote_device_ids: Vec<String>,
    log_path: &Path,
) -> DiagnosticsReport {
    DiagnosticsReport {
        app_version,
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        repos,
        pty_sessions,
        op_panics,
        remote_device_ids,
        log_path: log_path.to_string_lossy().into_owned(),
        log_tail: tail_lines(log_path, LOG_TAIL_LINES),
    }
}

/// Assemble a diagnostics snapshot. `repo_id` scopes `repos` to that repo
/// (error when it is not open); absent → every open repo.
#[tauri::command(rename_all = "snake_case")]
pub async fn diagnostics_bundle(
    repo_id: Option<RepoId>,
    app: tauri::AppHandle,
    repos: tauri::State<'_, crate::repo::RepoManager>,
    pty: tauri::State<'_, crate::pty::PtyRegistry>,
    devices: tauri::State<'_, crate::remote::transport::DeviceRegistry>,
) -> Result<DiagnosticsReport, String> {
    // Cheap state snapshots before the blocking hop.
    let handles = repos.all();
    let scoped = match repo_id {
        Some(id) => vec![handles
            .into_iter()
            .find(|h| h.id == id)
            .ok_or_else(|| format!("repo not open: {}", id.0))?],
        None => handles,
    };
    let repos: Vec<RepoDiagnostics> = scoped.iter().map(repo_diagnostics).collect();
    let pty_sessions = pty.live_count();
    let op_panics = crate::ops::panics_caught();
    let remote_device_ids = devices.connected_device_ids();
    let log_path = current_log_path(&app);
    let app_version = app.package_info().version.to_string();
    // The log tail reads up to a 5 MiB file: keep it off the main thread.
    tauri::async_runtime::spawn_blocking(move || {
        build_report(
            app_version,
            repos,
            pty_sessions,
            op_panics,
            remote_device_ids,
            &log_path,
        )
    })
    .await
    .map_err(|e| format!("diagnostics_bundle join failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-diag-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_line(writer: &RotatingLogWriter, line: &str) {
        writer.write_line(format!("{line}\n").as_bytes());
    }

    /// Rotation: crossing the size budget renames the current file to `.1`
    /// (previous generation replaced) and starts a fresh current file; only
    /// current + one previous ever exist.
    #[test]
    fn rotation_keeps_current_and_one_previous() {
        let dir = temp_dir("rotation");
        let log_path = dir.join("logs").join("mygitui.log");
        // 10-byte lines ("line 000\n"...), budget 100 bytes → ~10 lines per
        // generation.
        let writer = RotatingLogWriter::with_max_bytes(log_path.clone(), 100);
        for i in 0..35 {
            write_line(&writer, &format!("line {i:03}"));
        }
        let current = std::fs::read_to_string(&log_path).unwrap();
        let previous_path = previous_log_path(&log_path);
        let previous = std::fs::read_to_string(&previous_path).unwrap();

        // Structural properties (no byte-exact generation math): rotation
        // happened, the split is current=newest / previous=older, and only
        // current + one previous exist — earlier generations were replaced.
        assert!(
            current.contains("line 034"),
            "the newest line stays in the current file: {current}"
        );
        assert!(
            !current.contains("line 000"),
            "the oldest lines rotated out of the current file: {current}"
        );
        assert!(
            !previous.contains("line 034"),
            "the newest line never sits in the previous generation: {previous}"
        );
        assert!(!previous.is_empty(), "the previous generation is real");
        assert!(
            current.lines().count() <= 12 && previous.lines().count() <= 12,
            "each generation stays within one rotation step of the 100-byte budget: \
             {current:?} / {previous:?}"
        );
        let rotated_out = dir.join("logs").join("mygitui.log.2");
        assert!(!rotated_out.exists(), "never more than one previous");
    }

    /// A writer whose parent dir cannot be created (a FILE in the path)
    /// disables itself on the first line instead of failing forever, and
    /// never panics.
    #[test]
    fn unwritable_log_dir_disables_the_writer() {
        let dir = temp_dir("disabled");
        // `blocker` is a regular file; `<file>/logs` can never be a dir.
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"not a dir").unwrap();
        let log_path = blocker.join("logs").join("mygitui.log");
        let writer = RotatingLogWriter::new(log_path.clone());
        write_line(&writer, "first");
        write_line(&writer, "second");
        assert!(!log_path.exists());
    }

    /// The tail is the last N lines, oldest first.
    #[test]
    fn tail_lines_returns_last_n_oldest_first() {
        let dir = temp_dir("tail");
        let log_path = dir.join("app.log");
        let content: String = (0..60).map(|i| format!("l{i:02}\n")).collect();
        std::fs::write(&log_path, content).unwrap();
        let tail = tail_lines(&log_path, 50);
        assert_eq!(tail.len(), 50);
        assert_eq!(tail[0], "l10");
        assert_eq!(tail[49], "l59");

        // A shorter file tails whole, in order.
        std::fs::write(&log_path, "a\nb\nc\n").unwrap();
        assert_eq!(tail_lines(&log_path, 50), vec!["a", "b", "c"]);
        // Missing file → empty tail, no error.
        assert!(tail_lines(&dir.join("missing.log"), 50).is_empty());
    }

    /// Report assembly: the tauri-free core maps its inputs into the wire
    /// shape and tails the log file.
    #[test]
    fn report_assembles_from_parts() {
        let dir = temp_dir("report");
        let log_path = dir.join("logs").join("mygitui.log");
        std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        let content: String = (0..LOG_TAIL_LINES + 5)
            .map(|i| format!("log line {i:02}\n"))
            .collect();
        std::fs::write(&log_path, content).unwrap();

        let repos = vec![RepoDiagnostics {
            repo_id: "r1".to_owned(),
            root: "/home/dev/project".to_owned(),
            remote: false,
            device_id: None,
            watch_mode: Some(crate::watcher::WatchMode::Poll),
            op_queue_depth: 3,
        }];
        let report = build_report(
            "0.1.36".to_owned(),
            repos,
            2,
            7,
            vec!["dev-1".to_owned()],
            &log_path,
        );
        assert_eq!(report.app_version, "0.1.36");
        assert_eq!(report.os, std::env::consts::OS);
        assert_eq!(report.arch, std::env::consts::ARCH);
        assert_eq!(report.repos.len(), 1);
        assert_eq!(report.repos[0].op_queue_depth, 3);
        assert_eq!(report.pty_sessions, 2);
        assert_eq!(report.op_panics, 7);
        assert_eq!(report.remote_device_ids, vec!["dev-1"]);
        assert_eq!(report.log_path, log_path.to_string_lossy());
        assert_eq!(report.log_tail.len(), LOG_TAIL_LINES);
        assert_eq!(report.log_tail[0], "log line 05");
        assert_eq!(
            report.log_tail.last().unwrap(),
            &format!("log line {:02}", LOG_TAIL_LINES + 4)
        );

        // The payload serializes with the contracted keys (snake_case).
        let json = serde_json::to_value(&report).unwrap();
        for key in [
            "app_version",
            "os",
            "arch",
            "repos",
            "pty_sessions",
            "op_panics",
            "remote_device_ids",
            "log_path",
            "log_tail",
        ] {
            assert!(json.get(key).is_some(), "missing field {key}");
        }
        assert_eq!(json["repos"][0]["watch_mode"], "poll");
    }
}
