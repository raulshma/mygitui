//! Remote terminal sessions: SSH shell channels wired to the same
//! `PtyOutput` / `PtyExit` events the local PTY panel consumes, so the
//! terminal panel works unchanged against a device shell.
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

use crate::engine::types::RepoId;
use crate::pty::{PtyExit, PtyOutput};
use crate::service::events::{global, BackendEvent};
use crate::service::AppServices;

/// Max live remote shells (mirrors `pty::MAX_SESSIONS`).
pub const MAX_SESSIONS: usize = 8;

/// Coalescing window for output events (mirrors the local PTY).
const COALESCE_WINDOW: Duration = Duration::from_millis(16);

/// Max bytes in one output event.
const MAX_EVENT_BYTES: usize = 256 * 1024;

/// Emitter seam (same pattern as `pty::PtySink`) — keeps the event bus out
/// of unit tests.
pub trait ShellSink: Send + Sync {
    fn emit_output(&self, event: &PtyOutput);
    fn emit_exit(&self, event: &PtyExit);
}

/// Production sink: publishes [`PtyOutput`] / [`PtyExit`] on the
/// process-wide backend event bus.
pub struct BusShellSink;

impl ShellSink for BusShellSink {
    fn emit_output(&self, event: &PtyOutput) {
        global().publish(BackendEvent::PtyOutput(event.clone()));
    }

    fn emit_exit(&self, event: &PtyExit) {
        global().publish(BackendEvent::PtyExit(event.clone()));
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
        crate::rt::block_on(async {
            write
                .data(data.as_bytes())
                .await
                .map_err(|e| format!("shell write failed: {e}"))
        })
    }

    fn resize(&self, rows: u32, cols: u32) -> Result<(), String> {
        let write = self.write.lock();
        crate::rt::block_on(async {
            write
                .window_change(cols, rows, 0, 0)
                .await
                .map_err(|e| format!("window change failed: {e}"))
        })
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let write = self.write.lock();
        let _ = crate::rt::block_on(async { write.close().await });
    }
}

impl Drop for ShellSession {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        // write.close() best-effort; the reader also ends on EOF/Close from
        // the server side.
        let write = self.write.lock();
        let _ = crate::rt::block_on(async { write.close().await });
    }
}

/// Process-global registry of remote shell sessions (owned by
/// [`AppServices::remote_shells`](crate::service::AppServices)).
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
        let channel = crate::rt::block_on(
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
                    let msg = crate::rt::block_on(async {
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

// ---------- service entry points ----------

/// Open a remote shell for `repo_id` (must be a remote project).
pub fn rshell_create(
    services: &AppServices,
    repo_id: RepoId,
    rows: Option<u16>,
    cols: Option<u16>,
) -> Result<String, String> {
    let handle = services
        .repos
        .get(&repo_id)
        .ok_or_else(|| format!("repo not open: {}", repo_id.0))?;
    let remote = handle
        .remote()
        .ok_or_else(|| "terminal over SSH requires a remote project".to_string())?;
    services.remote_shells.create(
        &repo_id,
        remote,
        rows.unwrap_or(24),
        cols.unwrap_or(80),
        Arc::new(BusShellSink),
    )
}

/// Send frontend input (keystrokes, paste) to the remote shell.
pub fn rshell_write(
    services: &AppServices,
    session_id: String,
    data: String,
) -> Result<(), String> {
    services.remote_shells.write(&session_id, &data)
}

/// Resize the remote shell's pty.
pub fn rshell_resize(
    services: &AppServices,
    session_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    services.remote_shells.resize(&session_id, rows, cols)
}

/// Kill the remote shell session.
pub fn rshell_kill(services: &AppServices, session_id: String) -> Result<(), String> {
    services.remote_shells.kill(&session_id);
    Ok(())
}

/// `rshell_*` payload re-export for the FE (identical to the local PTY
/// events, listed here for discoverability).
#[derive(Debug, Clone, Serialize)]
pub struct RemoteShellSessionInfo {
    pub session_id: String,
}
