//! Embedded terminal PTY session manager (M5, contracts.md "Commands (M5)").
//!
//! `pty_create` opens a real pseudo-terminal (portable-pty / ConPTY on
//! Windows, termios on unix) rooted at the repository workdir and spawns an
//! interactive shell in it. Frontend input flows back via `pty_write`, the
//! shell's output streams to the frontend as `pty-output` events
//! `{session_id, data}` (lossy UTF-8 — a multibyte character split across
//! read boundaries degrades to U+FFFD instead of breaking the event
//! channel; the terminal renders the replacement and carries on). When the
//! child exits the session is unregistered and a final `pty-exit` event
//! `{session_id, code}` fires.
//!
//! Throttling: the raw reader thread forwards byte chunks through a small
//! channel to a coalescer thread that collects everything arriving within a
//! [`COALESCE_WINDOW`] (16 ms ≈ one display frame) into one event, so a
//! chatty `dir` produces ~60 events/s instead of one per 8 KiB read. A
//! single event never exceeds [`MAX_EVENT_BYTES`] (the surplus stays queued
//! in the channel for the next window — nothing is dropped).
//!
//! ConPTY handshake: conhost opens every session with a cursor-position
//! probe (`ESC[6n`) and, on current Windows builds, withholds all child
//! output until it is answered. The reader thread answers it backend-side
//! (cursor home) and strips the probe from the forwarded stream — the FE
//! never sees it and must not reply to it (see [`CURSOR_PROBE`]).
//!
//! Registry: a process-global [`PtyRegistry`] (tauri-managed state) owns
//! every live session, ids are `pty-<counter hex>`, and at most
//! [`MAX_SESSIONS`] run at once (further `pty_create` rejects). Sessions
//! are NOT tied to the repo lifecycle backend-side: closing a repo tab is
//! the frontend's job to `pty_kill` its terminal panels; as a backstop the
//! registry's `Drop` kills every remaining child on app exit (and
//! [`Session::drop`] kills again if an `Arc` outlives the registry in a
//! waiter thread).
//!
//! Threading: the child handle itself lives in the exit-waiter thread
//! (blocked in `wait`); the session keeps a `clone_killer()` handle so
//! `pty_kill` never needs to reach into that thread. The reader, coalescer
//! and waiter threads all exit on their own once the child dies (EOF /
//! wait returns). Design mirrors `actions.rs`: the emitter is abstracted
//! behind the [`PtySink`] trait ([`TauriPtySink`] in production, a
//! collector in tests), so the manager compiles and tests headless without
//! an `AppHandle`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;

use crate::engine::types::RepoId;
use crate::repo::RepoManager;

/// `pty-output` event name (contracts.md, Events M5).
pub const PTY_OUTPUT_EVENT: &str = "pty-output";

/// `pty-exit` event name (contracts.md, Events M5).
pub const PTY_EXIT_EVENT: &str = "pty-exit";

/// Max sessions live at once (further `pty_create` calls reject until one
/// exits or is killed).
pub const MAX_SESSIONS: usize = 8;

/// Coalescing window for `pty-output` events: reads arriving within one
/// window are merged into a single event (~60 events/s ceiling).
pub const COALESCE_WINDOW: Duration = Duration::from_millis(16);

/// Upper bound on one `pty-output` event's payload; a burst past this is
/// split across consecutive events (queued bytes are never dropped).
pub const MAX_EVENT_BYTES: usize = 256 * 1024;

/// Size of one raw read from the pty master.
const READ_CHUNK: usize = 8 * 1024;

/// conhost's cursor-position probe: every ConPTY session opens with it and,
/// on current Windows builds, conhost withholds ALL child output until the
/// terminal host answers it with a cursor-position report. Windows Terminal
/// and VS Code answer the same probe; we answer backend-side because the
/// probe fires at spawn time — possibly before the frontend terminal has
/// attached or wired its `onData` handler — and a missing answer leaves the
/// panel dead. The probe is also stripped from the forwarded stream so the
/// FE cannot double-answer (a stray report would pollute the shell's input).
pub(crate) const CURSOR_PROBE: &[u8] = b"\x1b[6n";
/// The answer: cursor at home row/col (the fresh console's actual position).
pub(crate) const CURSOR_PROBE_REPLY: &[u8] = b"\x1b[1;1R";

/// Terminal defaults (contracts.md: rows/cols default 24/80).
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

/// `pty-output` payload — field names and shapes are fixed by
/// docs/contracts.md and mirrored in `src/lib/ipc/types.ts`
/// (`PtyOutputEvent`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PtyOutput {
    pub session_id: String,
    /// Lossy-UTF-8 decoded terminal bytes for one coalesced window.
    pub data: String,
}

/// `pty-exit` payload — the final event of a session.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PtyExit {
    pub session_id: String,
    /// Process exit code (`1` when the child died by signal / was killed).
    pub code: i32,
}

/// Where `pty-output` / `pty-exit` events go (see `ops.rs::EventSink` and
/// `actions.rs::ActionSink` for the same pattern elsewhere).
pub trait PtySink: Send + Sync {
    fn emit_output(&self, event: &PtyOutput);
    fn emit_exit(&self, event: &PtyExit);
}

/// Production sink: emits both events on the app handle.
pub struct TauriPtySink {
    app: tauri::AppHandle,
}

impl TauriPtySink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl PtySink for TauriPtySink {
    fn emit_output(&self, event: &PtyOutput) {
        use tauri::Emitter;
        if let Err(err) = self.app.emit(PTY_OUTPUT_EVENT, event) {
            tracing::warn!(
                session = %event.session_id,
                error = %err,
                "emit pty-output failed"
            );
        }
    }

    fn emit_exit(&self, event: &PtyExit) {
        use tauri::Emitter;
        if let Err(err) = self.app.emit(PTY_EXIT_EVENT, event) {
            tracing::warn!(
                session = %event.session_id,
                error = %err,
                "emit pty-exit failed"
            );
        }
    }
}

/// One live terminal session. The child process itself lives in the
/// exit-waiter thread; everything the registry needs after the spawn is
/// here.
pub(crate) struct Session {
    /// Master-side writer (frontend input). Shared with the reader thread,
    /// which answers conhost's cursor probe through it; all writers are
    /// lock-serialized (frontend `pty_write` vs probe replies).
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Independent kill handle (`clone_killer`), so `pty_kill` works while
    /// the waiter thread holds the child in `wait`.
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// Master end: kept for `pty_resize` (mutex-wrapped so `Session` is
    /// `Sync` — tauri-managed state requires it — not for write
    /// concurrency: resize is the only caller).
    master: Mutex<Box<dyn MasterPty + Send>>,
}

impl Session {
    fn write(&self, data: &str) -> Result<(), String> {
        let mut writer = self.writer.lock();
        writer
            .write_all(data.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|e| format!("pty write failed: {e}"))
    }

    fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        self.master
            .lock()
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("pty resize failed: {e}"))
    }

    fn kill(&self) {
        // Errors are fine: the child may have exited already.
        let _ = self.killer.lock().kill();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Backstop kill: if an Arc<Session> outlives the registry (a waiter
        // thread still draining), the child must not outlive the app.
        self.kill();
    }
}

/// Process-global registry of terminal sessions (tauri-managed state): id
/// allocation, lookup, the concurrency cap and app-exit cleanup.
#[derive(Default)]
pub struct PtyRegistry {
    inner: Arc<RegistryInner>,
}

#[derive(Default)]
struct RegistryInner {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    counter: AtomicU64,
}

impl PtyRegistry {
    fn next_session_id(&self) -> String {
        format!(
            "pty-{:x}",
            self.inner.counter.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// How many sessions are live right now (the cap is [`MAX_SESSIONS`]).
    pub fn live_count(&self) -> usize {
        self.inner.sessions.lock().len()
    }

    fn insert(&self, session_id: &str, session: Arc<Session>) {
        self.inner
            .sessions
            .lock()
            .insert(session_id.to_owned(), session);
    }

    /// Live session lookup (also the tests' registry probe).
    pub(crate) fn get(&self, session_id: &str) -> Option<Arc<Session>> {
        self.inner.sessions.lock().get(session_id).cloned()
    }

    /// Remove a session (exit waiter or explicit kill); returns it so the
    /// caller can still act on it after the removal.
    fn remove(&self, session_id: &str) -> Option<Arc<Session>> {
        self.inner.sessions.lock().remove(session_id)
    }

    /// Kill and forget every live session (app exit / Drop). The sessions'
    /// `Drop` impls re-kill, so this is belt and braces.
    fn kill_all(&self) {
        let sessions: Vec<Arc<Session>> =
            self.inner.sessions.lock().drain().map(|(_, s)| s).collect();
        for session in sessions {
            session.kill();
        }
    }
}

impl Drop for PtyRegistry {
    fn drop(&mut self) {
        // Orphan-kill on app exit: every remaining child dies here, which
        // also unblocks the exit-waiter threads so their Arcs drop.
        self.kill_all();
    }
}

impl Clone for PtyRegistry {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Everything needed to launch one session (validated by the caller).
pub(crate) struct SessionSpec {
    /// Working directory for the shell (the repo workdir root).
    pub cwd: PathBuf,
    pub rows: u16,
    pub cols: u16,
    /// Test override: exact command to spawn. `None` discovers the default
    /// interactive shell (see [`default_shell_command`]).
    pub command: Option<CommandBuilder>,
}

/// Spawn `spec` in a fresh pty and wire up input/output/exit plumbing (see
/// module docs). Returns immediately after the spawn; output flows through
/// the sink. Fails without side effects if the repo cap is hit, the pty
/// cannot be opened or the command cannot be spawned.
pub(crate) fn spawn_session(
    registry: &PtyRegistry,
    spec: SessionSpec,
    sink: Arc<dyn PtySink>,
) -> Result<String, String> {
    if registry.live_count() >= MAX_SESSIONS {
        return Err(format!(
            "{MAX_SESSIONS} terminal sessions already running; close one first"
        ));
    }
    let rows = spec.rows.max(1);
    let cols = spec.cols.max(1);

    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("failed to open pty: {e}"))?;

    let mut cmd = spec.command.unwrap_or_else(default_shell_command);
    cmd.cwd(&spec.cwd);
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("failed to spawn shell in pty: {e}"))?;
    let writer = Arc::new(Mutex::new(
        pair.master
            .take_writer()
            .map_err(|e| format!("failed to take pty writer: {e}"))?,
    ));
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("failed to clone pty reader: {e}"))?;
    let killer = child.clone_killer();

    let session_id = registry.next_session_id();
    // The reader thread gets its own writer Arc: it answers conhost's
    // cursor probe through it (see CURSOR_PROBE).
    let pipeline_writer = writer.clone();
    let session = Arc::new(Session {
        writer,
        killer: Mutex::new(killer),
        master: Mutex::new(pair.master),
    });
    registry.insert(&session_id, session.clone());

    spawn_output_pipeline(&session_id, reader, pipeline_writer, sink.clone());
    spawn_exit_waiter(registry.clone(), &session_id, child, sink);

    Ok(session_id)
}

/// Pure decision at the heart of the output coalescer: a burst flushes as
/// one `pty-output` event when it holds bytes and either the coalescing
/// window since the burst's first chunk has elapsed or the buffer reached
/// the event-size cap (a burst larger than one event splits across
/// consecutive events — queued bytes are never dropped). Extracted so the
/// accumulation logic stays unit-testable without a live pty.
pub(crate) fn coalesce_ready(buffer_len: usize, burst_age: Duration) -> bool {
    buffer_len > 0 && (burst_age >= COALESCE_WINDOW || buffer_len >= MAX_EVENT_BYTES)
}

/// Pure half of the cursor-probe handshake: removes [`CURSOR_PROBE`]
/// occurrences from `data` (a partial probe at the chunk end is carried in
/// `carry` across reads) and returns the bytes to forward plus how many
/// probes were found — the caller writes one [`CURSOR_PROBE_REPLY`] per
/// probe through the pty writer. Unit-tested without a live pty.
pub(crate) fn strip_cursor_probes(data: &[u8], carry: &mut Vec<u8>) -> (Vec<u8>, usize) {
    let mut bytes = std::mem::take(carry);
    bytes.extend_from_slice(data);
    let mut out = Vec::with_capacity(bytes.len());
    let mut answers = 0usize;
    let mut i = 0usize;
    'scan: while i < bytes.len() {
        let mut j = 0usize;
        while j < CURSOR_PROBE.len() {
            if i + j >= bytes.len() {
                // Data ends mid-probe: hold the tail for the next read.
                carry.extend_from_slice(&bytes[i..]);
                break 'scan;
            }
            if bytes[i + j] != CURSOR_PROBE[j] {
                break;
            }
            j += 1;
        }
        if j == CURSOR_PROBE.len() {
            answers += 1;
            i += CURSOR_PROBE.len();
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    (out, answers)
}

/// Reader thread (pty → channel) plus coalescer thread (channel → sink,
/// 16 ms windows). Both exit on their own at EOF / channel close. The
/// reader also answers conhost's cursor probe on the writer (output stalls
/// until it is answered) and strips the probe from the forwarded stream.
fn spawn_output_pipeline(
    session_id: &str,
    reader: Box<dyn Read + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    sink: Arc<dyn PtySink>,
) {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();

    std::thread::Builder::new()
        .name(format!("pty-read-{session_id}"))
        .spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; READ_CHUNK];
            let mut carry: Vec<u8> = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF: the child (or pty) is gone
                    Ok(n) => {
                        let (forward, probes) = strip_cursor_probes(&buf[..n], &mut carry);
                        for _ in 0..probes {
                            // The probe must be answered or conhost never
                            // sends child output; errors mean the pty is
                            // already gone.
                            let mut writer = writer.lock();
                            let _ = writer
                                .write_all(CURSOR_PROBE_REPLY)
                                .and_then(|_| writer.flush());
                        }
                        if forward.is_empty() {
                            continue; // nothing left to forward this read
                        }
                        if tx.send(forward).is_err() {
                            break; // coalescer gone: nothing to feed
                        }
                    }
                    Err(_) => break, // pty error: the child is gone
                }
            }
        })
        .expect("spawn pty reader thread");

    let id = session_id.to_owned();
    std::thread::Builder::new()
        .name(format!("pty-coalesce-{session_id}"))
        .spawn(move || {
            // Block until the next burst starts, then drain the rest of the
            // window into the same event.
            while let Ok(first) = rx.recv() {
                let mut acc = first;
                let burst_start = Instant::now();
                while !coalesce_ready(acc.len(), burst_start.elapsed()) {
                    match rx.recv_timeout(COALESCE_WINDOW) {
                        Ok(more) => acc.extend_from_slice(&more),
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                let event = PtyOutput {
                    session_id: id.clone(),
                    data: String::from_utf8_lossy(&acc).into_owned(),
                };
                sink.emit_output(&event);
                // An over-cap burst continues with the next event: bytes
                // already queued in the channel arrive instantly as the next
                // `first`, so the split adds no latency.
            }
        })
        .expect("spawn pty coalescer thread");
}

/// Exit-waiter thread: blocks on the child, then unregisters the session
/// and emits the final `pty-exit` event with the real exit code.
fn spawn_exit_waiter(
    registry: PtyRegistry,
    session_id: &str,
    mut child: Box<dyn Child + Send + Sync>,
    sink: Arc<dyn PtySink>,
) {
    let id = session_id.to_owned();
    std::thread::Builder::new()
        .name(format!("pty-wait-{session_id}"))
        .spawn(move || {
            let code = match child.wait() {
                Ok(status) => status.exit_code() as i32,
                Err(_) => -1,
            };
            registry.remove(&id);
            sink.emit_exit(&PtyExit {
                session_id: id,
                code,
            });
        })
        .expect("spawn pty exit waiter thread");
}

/// Which-style PATH probe: the first `name` found as a file on `PATH`.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The default interactive shell for a session (cwd is applied by the
/// caller from the spec). A minimal env addition (`TERM=xterm-256color` on
/// unix; ConPTY sets up the console environment on Windows) rides on top
/// of the inherited process env. Windows prefers pwsh, then powershell,
/// then falls back to cmd.exe; unix uses `$SHELL`, then bash, then /bin/sh.
pub(crate) fn default_shell_command() -> CommandBuilder {
    #[cfg(windows)]
    {
        let shell = ["pwsh.exe", "powershell.exe"]
            .into_iter()
            .find_map(find_on_path)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "cmd.exe".to_string());
        CommandBuilder::new(shell)
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                find_on_path("bash")
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "/bin/sh".to_string())
            });
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        cmd
    }
}

// ---------------------------------------------------------------------------
// IPC commands (contracts.md M5). Names/shapes fixed by docs/contracts.md.
// ---------------------------------------------------------------------------

/// Spawn a shell in a fresh pty at the repo root. Returns the session id
/// the frontend uses for write/resize/kill and receives in the events.
#[tauri::command(rename_all = "snake_case")]
pub fn pty_create(
    repo_id: RepoId,
    rows: Option<u16>,
    cols: Option<u16>,
    app: tauri::AppHandle,
    repos: tauri::State<'_, RepoManager>,
    registry: tauri::State<'_, PtyRegistry>,
) -> Result<String, String> {
    // An open repo is required: the terminal is always rooted at a
    // repository (no app-cwd sessions).
    let handle = repos
        .get(&repo_id)
        .ok_or_else(|| format!("repo not open: {}", repo_id.0))?;
    spawn_session(
        &registry,
        SessionSpec {
            cwd: handle.root.clone(),
            rows: rows.unwrap_or(DEFAULT_ROWS),
            cols: cols.unwrap_or(DEFAULT_COLS),
            command: None,
        },
        Arc::new(TauriPtySink::new(app)),
    )
}

/// Send frontend input (keystrokes, paste) to the session's shell.
#[tauri::command(rename_all = "snake_case")]
pub fn pty_write(
    session_id: String,
    data: String,
    registry: tauri::State<'_, PtyRegistry>,
) -> Result<(), String> {
    write_to_session(&registry, &session_id, &data)
}

/// Command body of [`pty_write`], split so tests can drive a registry
/// without `State` (same pattern as `clean_op` in ipc_commands).
pub(crate) fn write_to_session(
    registry: &PtyRegistry,
    session_id: &str,
    data: &str,
) -> Result<(), String> {
    let session = registry
        .get(session_id)
        .ok_or_else(|| format!("unknown pty session: {session_id}"))?;
    session.write(data)
}

/// Resize the session's terminal (frontend panel size changed).
#[tauri::command(rename_all = "snake_case")]
pub fn pty_resize(
    session_id: String,
    rows: u16,
    cols: u16,
    registry: tauri::State<'_, PtyRegistry>,
) -> Result<(), String> {
    resize_session(&registry, &session_id, rows, cols)
}

/// Command body of [`pty_resize`] (see [`write_to_session`]).
pub(crate) fn resize_session(
    registry: &PtyRegistry,
    session_id: &str,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    let session = registry
        .get(session_id)
        .ok_or_else(|| format!("unknown pty session: {session_id}"))?;
    session.resize(rows, cols)
}

/// Kill a session's shell and clean up the registry entry. The `pty-exit`
/// event still fires (from the exit waiter, with the real code). Returns
/// an error when the session is unknown or already gone.
#[tauri::command(rename_all = "snake_case")]
pub fn pty_kill(session_id: String, registry: tauri::State<'_, PtyRegistry>) -> Result<(), String> {
    kill_session(&registry, &session_id)
}

/// Command body of [`pty_kill`] (see [`write_to_session`]).
pub(crate) fn kill_session(registry: &PtyRegistry, session_id: &str) -> Result<(), String> {
    let session = registry
        .remove(session_id)
        .ok_or_else(|| format!("unknown pty session: {session_id}"))?;
    session.kill();
    Ok(())
}
