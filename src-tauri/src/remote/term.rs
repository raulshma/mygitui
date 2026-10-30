//! Remote terminal sessions: SSH shell channels wired to the same
//! `pty-output` / `pty-exit` events the local PTY panel consumes, so
//! `TerminalPanel.svelte` works unchanged against a device shell.
//!
//! Each session splits the russh channel into read/write halves: the reader
//! task streams decoded output chunks (16 ms coalesce window like the local
//! PTY), writes go through the write half. Dropping both halves (kill /
//! registry teardown / app exit) closes the channel and ends the remote
//! shell.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use russh::client::Msg;
use russh::ChannelWriteHalf;
use serde::Serialize;
use ts_rs::TS;

use crate::engine::types::RepoId;
use crate::pty::{PtyExit, PtyOutput};
use crate::repo::RepoManager;

/// Max live remote shells (mirrors `pty::MAX_SESSIONS`).
pub const MAX_SESSIONS: usize = 8;

/// Coalescing window for output events (mirrors the local PTY).
const COALESCE_WINDOW: Duration = Duration::from_millis(16);

/// Max bytes in one output event.
const MAX_EVENT_BYTES: usize = 256 * 1024;

/// Emitter seam (same pattern as `pty::PtySink`) — keeps the tauri emitter
/// out of unit tests.
pub trait ShellSink: Send + Sync {
    fn emit_output(&self, event: &PtyOutput);
    fn emit_exit(&self, event: &PtyExit);
}

/// Production sink on the app handle.
pub struct TauriShellSink {
    app: tauri::AppHandle,
}

impl TauriShellSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl ShellSink for TauriShellSink {
    fn emit_output(&self, event: &PtyOutput) {
        use tauri::Emitter;
        let _ = self.app.emit(crate::pty::PTY_OUTPUT_EVENT, event);
    }

    fn emit_exit(&self, event: &PtyExit) {
        use tauri::Emitter;
        let _ = self.app.emit(crate::pty::PTY_EXIT_EVENT, event);
    }
}

struct ShellSession {
    write: Mutex<ChannelWriteHalf<Msg>>,
    /// Flipped by kill; the reader task exits on sight.
    closed: Arc<AtomicBool>,
}

impl ShellSession {
    fn write_bytes(&self, data: &str) -> Result<(), String> {
        let write = self.write.lock();
        tauri::async_runtime::block_on(async {
            write
                .data(data.as_bytes())
                .await
                .map_err(|e| format!("shell write failed: {e}"))
        })
    }

    fn resize(&self, rows: u32, cols: u32) -> Result<(), String> {
        let write = self.write.lock();
        tauri::async_runtime::block_on(async {
            write
                .window_change(cols, rows, 0, 0)
                .await
                .map_err(|e| format!("window change failed: {e}"))
        })
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let write = self.write.lock();
        let _ = tauri::async_runtime::block_on(async { write.close().await });
    }
}

impl Drop for ShellSession {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        // write.close() best-effort; the reader also ends on EOF/Close from
        // the server side.
        let write = self.write.lock();
        let _ = tauri::async_runtime::block_on(async { write.close().await });
    }
}

/// Process-global registry of remote shell sessions (tauri-managed state).
#[derive(Default)]
pub struct RemoteShellRegistry {
    inner: Arc<RegistryInner>,
}

#[derive(Default)]
struct RegistryInner {
    sessions: Mutex<HashMap<String, Arc<ShellSession>>>,
    counter: AtomicU64,
}

impl RemoteShellRegistry {
    /// Create a shell session on the device behind `repo_id`.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        _repo_id: &RepoId,
        remote: &crate::repo::RemoteHandle,
        rows: u16,
        cols: u16,
        sink: Arc<dyn ShellSink>,
    ) -> Result<String, String> {
        {
            let sessions = self.inner.sessions.lock();
            if sessions.len() >= MAX_SESSIONS {
                return Err(format!("too many terminal sessions (max {MAX_SESSIONS})"));
            }
        }
        let channel = tauri::async_runtime::block_on(
            remote
                .git()
                .session
                .shell(rows.max(1) as u32, cols.max(1) as u32),
        )?;
        let (mut read_half, write_half) = channel.split();
        let id = format!(
            "rshell-{:x}",
            self.inner.counter.fetch_add(1, Ordering::Relaxed)
        );
        let closed = Arc::new(AtomicBool::new(false));
        let session = Arc::new(ShellSession {
            write: Mutex::new(write_half),
            closed: closed.clone(),
        });
        self.inner
            .sessions
            .lock()
            .insert(id.clone(), session.clone());

        // Reader task: stream decoded output through the coalescer.
        let reader_id = id.clone();
        std::thread::Builder::new()
            .name(format!("rshell-read-{reader_id}"))
            .spawn(move || {
                let mut pending: Vec<u8> = Vec::new();
                let mut window_start = std::time::Instant::now();
                loop {
                    if closed.load(Ordering::SeqCst) {
                        break;
                    }
                    let msg = tauri::async_runtime::block_on(async {
                        tokio::time::timeout(Duration::from_millis(200), read_half.wait()).await
                    });
                    match msg {
                        Err(_) => {
                            // Timeout: flush the coalesce window.
                            flush_pending(
                                &mut pending,
                                &mut window_start,
                                &reader_id,
                                sink.as_ref(),
                            );
                        }
                        Ok(None) => break, // channel closed
                        Ok(Some(russh::ChannelMsg::Data { data })) => {
                            pending.extend_from_slice(&data);
                            if pending.len() > MAX_EVENT_BYTES
                                || window_start.elapsed() >= COALESCE_WINDOW
                            {
                                flush_pending(
                                    &mut pending,
                                    &mut window_start,
                                    &reader_id,
                                    sink.as_ref(),
                                );
                            }
                        }
                        Ok(Some(russh::ChannelMsg::ExtendedData { data, .. })) => {
                            pending.extend_from_slice(&data);
                        }
                        Ok(Some(russh::ChannelMsg::ExitStatus { exit_status })) => {
                            sink.emit_output(&PtyOutput {
                                session_id: reader_id.clone(),
                                data: String::new(),
                            });
                            let _ = exit_status;
                        }
                        Ok(Some(_)) => {}
                    }
                }
                // Final flush + exit event.
                flush_pending(&mut pending, &mut window_start, &reader_id, sink.as_ref());
                sink.emit_exit(&PtyExit {
                    session_id: reader_id,
                    code: 0,
                });
            })
            .map_err(|e| format!("spawn shell reader: {e}"))?;
        Ok(id)
    }

    pub fn write(&self, session_id: &str, data: &str) -> Result<(), String> {
        self.get(session_id)?.write_bytes(data)
    }

    pub fn resize(&self, session_id: &str, rows: u16, cols: u16) -> Result<(), String> {
        self.get(session_id)?
            .resize(rows.max(1) as u32, cols.max(1) as u32)
    }

    /// Kill a session (idempotent; false when unknown).
    pub fn kill(&self, session_id: &str) -> bool {
        match self.inner.sessions.lock().remove(session_id) {
            Some(session) => {
                session.close();
                true
            }
            None => false,
        }
    }

    fn get(&self, session_id: &str) -> Result<Arc<ShellSession>, String> {
        self.inner
            .sessions
            .lock()
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("unknown remote shell session: {session_id}"))
    }
}

fn flush_pending(
    pending: &mut Vec<u8>,
    window_start: &mut std::time::Instant,
    session_id: &str,
    sink: &dyn ShellSink,
) {
    if pending.is_empty() {
        *window_start = std::time::Instant::now();
        return;
    }
    let data = String::from_utf8_lossy(pending).into_owned();
    pending.clear();
    *window_start = std::time::Instant::now();
    sink.emit_output(&PtyOutput {
        session_id: session_id.to_string(),
        data,
    });
}

// ---------- IPC commands ----------

/// Open a remote shell for `repo_id` (must be a remote project).
#[tauri::command(rename_all = "snake_case")]
pub fn rshell_create(
    repo_id: RepoId,
    rows: Option<u16>,
    cols: Option<u16>,
    app: tauri::AppHandle,
    repos: tauri::State<'_, RepoManager>,
    registry: tauri::State<'_, RemoteShellRegistry>,
) -> Result<String, String> {
    let handle = repos
        .get(&repo_id)
        .ok_or_else(|| format!("repo not open: {}", repo_id.0))?;
    let remote = handle
        .remote()
        .ok_or_else(|| "terminal over SSH requires a remote project".to_string())?;
    registry.create(
        &repo_id,
        remote,
        rows.unwrap_or(24),
        cols.unwrap_or(80),
        Arc::new(TauriShellSink::new(app)),
    )
}

/// Send frontend input (keystrokes, paste) to the remote shell.
#[tauri::command(rename_all = "snake_case")]
pub fn rshell_write(
    session_id: String,
    data: String,
    registry: tauri::State<'_, RemoteShellRegistry>,
) -> Result<(), String> {
    registry.write(&session_id, &data)
}

/// Resize the remote shell's pty.
#[tauri::command(rename_all = "snake_case")]
pub fn rshell_resize(
    session_id: String,
    rows: u16,
    cols: u16,
    registry: tauri::State<'_, RemoteShellRegistry>,
) -> Result<(), String> {
    registry.resize(&session_id, rows, cols)
}

/// Kill the remote shell session.
#[tauri::command(rename_all = "snake_case")]
pub fn rshell_kill(
    session_id: String,
    registry: tauri::State<'_, RemoteShellRegistry>,
) -> Result<(), String> {
    registry.kill(&session_id);
    Ok(())
}

/// `rshell_*` payload re-export for the FE (identical to the local PTY
/// events, listed here for discoverability).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RemoteShellSessionInfo {
    pub session_id: String,
}
