//! Per-repo op queues with `op-progress` events (contracts.md M2).
//!
//! M1 read commands call the engine directly under the repo mutex —
//! concurrent reads are safe because `git2::Repository` access is fully
//! serialized by that lock. Every M2 mutation and network op instead goes
//! through [`OpQueue`] (one per [`crate::repo::RepoHandle`]): a worker
//! thread per repo executes enqueued jobs in FIFO order, so a push never
//! races a stage and one repo's slow fetch never blocks another's commit.
//!
//! Two lanes share the queue's bookkeeping (op ids, cancellation, close):
//! the **mutation lane** (default) serializes mutations and network ops in
//! FIFO order — a pull's merge phase can never interleave with a stage —
//! while the **read lane** ([`OpQueue::enqueue_read_lane`]) runs read-only
//! jobs on a second worker. Reads take the shared repo-handle mutex inside
//! their job, so a mutation still blocks at most one read; network ops
//! never touch the shared handle (fresh handle per op), so a slow
//! fetch/push/pull no longer stalls status/log/diff/blame refreshes.
//!
//! Progress flows through [`OpCtx::emit_progress`], throttled to at most
//! one event per [`THROTTLE_MS`] per op; the queue itself always emits a
//! final `done: true` event when a job finishes (success or error) — except
//! for silent enqueues ([`OpQueue::enqueue_silent`], used for reads), which
//! emit nothing. Events reach the frontend via the [`EventSink`]
//! abstraction — [`TauriEventSink`] in production, a collector in tests —
//! so the queue itself needs no `AppHandle` to compile or run headless.
//!
//! Lifecycle: worker threads are spawned lazily on the first `enqueue` and
//! exit once [`OpQueue::shutdown`] runs (repo close) and the backlog has
//! drained. Jobs still queued at close are executed, not dropped; their
//! oneshot receivers still resolve.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Instant;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use ts_rs::TS;

use crate::engine::types::{ErrCode, IpcErrorEnvelope, RepoId};

/// `op-progress` event name (contracts.md, Events M2).
pub const OP_PROGRESS_EVENT: &str = "op-progress";

/// `op-log` event name: raw, per-line child output (hooks, CLI fallbacks).
pub const OP_LOG_EVENT: &str = "op-log";

/// Minimum interval between two throttled progress events for one op
/// (mirrors the clone-progress throttle in ipc_commands).
pub(crate) const THROTTLE_MS: u64 = 80;

/// LEGACY error string for a caller-aborted op: the bare word the queue's
/// queued-skip, the CLI cancel checks, and (before the error envelope) the
/// engine's cooperative cancellation all returned. Production now emits the
/// envelope with `code: "cancelled"` (`IpcErrorEnvelope::cancelled`), and
/// [`OpOutcome::for_error`] classifies BOTH shapes for one release so
/// backlevel mocks/tests keep working.
pub const CANCELLED_ERR: &str = "cancelled";

/// Parses the structured error envelope out of a terminal op error string.
/// `None` for raw strings (misc lanes keep those until migrated) and for
/// JSON without the version marker.
fn parse_envelope(error: &str) -> Option<IpcErrorEnvelope> {
    let envelope = serde_json::from_str::<IpcErrorEnvelope>(error).ok()?;
    (envelope.mygitui_error == IpcErrorEnvelope::VERSION).then_some(envelope)
}

/// True when the terminal error string means the op was aborted through the
/// op-cancel path: the structured envelope (`code: "cancelled"`) or the
/// legacy bare word (see [`CANCELLED_ERR`]). Every other error is a real
/// failure.
fn is_cancelled_error(error: &str) -> bool {
    error == CANCELLED_ERR || parse_envelope(error).is_some_and(|e| e.code == ErrCode::Cancelled)
}

/// Human text for a terminal op error: envelope errors contribute their
/// message, raw strings pass through unchanged. Used for the done event's
/// display-only `message` field — `error` itself keeps the raw wire form.
fn error_display(error: &str) -> String {
    parse_envelope(error)
        .map(|e| e.message)
        .unwrap_or_else(|| error.to_owned())
}

/// Terminal classification of one op (`OpProgress.outcome`). `cancelled`
/// is reserved for ops aborted through the op-cancel path — any other
/// error is a real failure. Absent on non-final events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OpOutcome {
    Completed,
    Cancelled,
    Failed,
}

impl OpOutcome {
    /// Classify a terminal op state from its error string (the queue's
    /// single sentinel comparison — the FE receives the precomputed
    /// outcome instead of pattern-matching error text).
    pub fn for_error(error: Option<&str>) -> Self {
        match error {
            None => OpOutcome::Completed,
            Some(msg) if is_cancelled_error(msg) => OpOutcome::Cancelled,
            Some(_) => OpOutcome::Failed,
        }
    }
}

/// `op-progress` payload — field names and shapes are fixed by
/// docs/contracts.md; generated to `src/lib/ipc/gen/` via ts-rs and
/// re-exported by `src/lib/ipc/types.ts`.
#[derive(Debug, Clone, Serialize, PartialEq, TS)]
#[ts(export)]
pub struct OpProgress {
    pub repo_id: String,
    pub op_id: String,
    pub kind: String,
    pub message: String,
    /// `null` until a percentage is known.
    pub pct: Option<f64>,
    /// True on the final event of an op.
    pub done: bool,
    /// `Some(msg)` when the op failed: the structured error envelope (JSON
    /// string, `mygitui_error` marker) for engine-originated failures, a raw
    /// string for misc lanes. `message` carries the human text either way.
    pub error: Option<String>,
    /// Terminal classification, set on the final event only.
    pub outcome: Option<OpOutcome>,
}

/// One line of raw op output (`op-log`). Hooks and CLI fallbacks stream
/// their stdout/stderr through this; the FE renders it as a log panel.
#[derive(Debug, Clone, Serialize, PartialEq, TS)]
#[ts(export)]
pub struct OpLog {
    pub repo_id: String,
    pub op_id: String,
    pub kind: String,
    pub line: String,
}

/// Where progress events go. Abstracted so the queue has no hard dependency
/// on tauri: production installs a [`TauriEventSink`] via
/// [`OpQueue::set_sink`] (see `RepoManager::open`), tests install a collector.
/// Without a sink, events are silently dropped.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &OpProgress);

    /// Stream one raw output line for an op ([`OpLog`]). Optional: sinks
    /// that only care about progress can leave the no-op default.
    fn emit_log(&self, _event: &OpLog) {}
}

/// Production sink: emits [`OpProgress`] on the app handle.
pub struct TauriEventSink {
    app: tauri::AppHandle,
}

impl TauriEventSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: &OpProgress) {
        use tauri::Emitter;
        if let Err(err) = self.app.emit(OP_PROGRESS_EVENT, event) {
            tracing::warn!(
                repo = %event.repo_id,
                op = %event.op_id,
                error = %err,
                "emit op-progress failed"
            );
        }
    }

    fn emit_log(&self, event: &OpLog) {
        use tauri::Emitter;
        if let Err(err) = self.app.emit(OP_LOG_EVENT, event) {
            tracing::warn!(
                repo = %event.repo_id,
                op = %event.op_id,
                error = %err,
                "emit op-log failed"
            );
        }
    }
}

/// One queued unit of work; `run` performs the op, emits the final done
/// event, and resolves the caller's oneshot (identity fields are captured
/// inside the closure).
struct Job {
    run: Box<dyn FnOnce() + Send + 'static>,
}

/// Per-repo serial executor (see module docs). Cheap to clone: all clones
/// share one worker channel.
pub struct OpQueue {
    inner: Arc<QueueInner>,
}

impl Clone for OpQueue {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

struct QueueInner {
    repo_id: RepoId,
    /// Lazily spawned mutation worker's channel; `None` after
    /// [`OpQueue::shutdown`].
    sender: Mutex<Option<mpsc::Sender<Job>>>,
    /// Lazily spawned read worker's channel; `None` after shutdown. Reads
    /// run here so they never queue behind a long mutation or net op.
    read_sender: Mutex<Option<mpsc::Sender<Job>>>,
    /// Monotonic op counter (op ids are `<repo>-op-<n hex>`).
    counter: AtomicU64,
    /// Ops accepted but not yet finished (queued on either lane + running).
    /// Diagnostics telemetry ([`OpQueue::depth`]); decremented on every
    /// completion path (normal, error, cancel-skip). The one uncounted leak
    /// is a worker killed by a queue-bookkeeping panic — its dropped
    /// backlog stays counted, and the queue is dead right after anyway.
    depth: AtomicU64,
    sink: RwLock<Option<Arc<dyn EventSink>>>,
    closed: AtomicBool,
    /// M12 cooperative cancellation: one token per live op, keyed by op id.
    /// An entry exists from `enqueue` until the op finishes (or is skipped);
    /// `cancel_op` flips the token and the op observes it via
    /// [`OpCtx::cancelled`]. Queued-not-started ops are skipped entirely.
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

/// Process-global count of ops that panicked and were caught by the op
/// queue's guards (diagnostics: `op_panics`). Both catch sites count: a
/// panicking op closure and a worker killed by queue-bookkeeping panic.
pub(crate) static OP_PANICS: AtomicU64 = AtomicU64::new(0);

/// Ops caught panicking since launch (diagnostics: `op_panics`).
pub fn panics_caught() -> u64 {
    OP_PANICS.load(Ordering::Relaxed)
}

/// Which worker an enqueued job runs on.
#[derive(Clone, Copy)]
enum Lane {
    /// Mutations + net ops: FIFO-serialized with each other.
    Mutation,
    /// Read-only jobs: concurrent with the mutation lane; the shared
    /// repo-handle mutex inside the job still serializes reads against
    /// mutations.
    Read,
}

impl OpQueue {
    pub fn new(repo_id: RepoId) -> Self {
        Self {
            inner: Arc::new(QueueInner {
                repo_id,
                sender: Mutex::new(None),
                read_sender: Mutex::new(None),
                counter: AtomicU64::new(0),
                depth: AtomicU64::new(0),
                sink: RwLock::new(None),
                closed: AtomicBool::new(false),
                cancels: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Request cancellation of one op (M12). Queued-not-started ops are
    /// skipped entirely (the job closure resolves with a "cancelled" error);
    /// running ops observe the flag cooperatively through
    /// [`OpCtx::cancelled`]. Unknown or already-finished ops return `false`.
    pub fn cancel_op(&self, repo_id: &str, op_id: &str) -> bool {
        // Op ids are minted as `<repo_id>-op-<n>`; a foreign repo_id can
        // never match an op of this queue (the registry is per-repo anyway).
        if !repo_id.is_empty() && !op_id.starts_with(repo_id) {
            return false;
        }
        match self.inner.cancels.lock().get(op_id) {
            Some(token) => {
                token.store(true, Ordering::Release);
                true
            }
            None => false,
        }
    }

    /// Install (or replace) the progress sink. Called by `RepoManager::open`
    /// once an `AppHandle` exists; headless callers (tests) skip it.
    pub fn set_sink(&self, sink: Arc<dyn EventSink>) {
        *self.inner.sink.write() = Some(sink);
    }

    /// Ops accepted but not yet finished (queued on either lane + running).
    /// Diagnostics telemetry (`diagnostics_bundle` → `op_queue_depth`);
    /// may over-count by the dropped backlog of a worker killed by a
    /// queue-bookkeeping panic (see [`QueueInner::depth`]).
    pub fn depth(&self) -> u64 {
        self.inner.depth.load(Ordering::Relaxed)
    }

    /// Enqueue `kind` op `run` on this repo's serial queue. Returns the op
    /// id and a receiver resolving with the job's result; the receiver also
    /// resolves (with an error) if the repo closes before the op runs.
    pub fn enqueue<T, F>(
        &self,
        kind: &str,
        run: F,
    ) -> (String, tokio::sync::oneshot::Receiver<Result<T, String>>)
    where
        T: Send + 'static,
        F: FnOnce(OpCtx) -> Result<T, String> + Send + 'static,
    {
        self.enqueue_inner(kind, run, true, Lane::Mutation)
    }

    /// Same as [`OpQueue::enqueue`] but silent: no `op-progress` events at
    /// all — no throttled progress, no final done event, and a
    /// cancelled-while-queued op is skipped without an event (the caller's
    /// receiver still resolves with the "cancelled" error). Used for
    /// queue-dispatched reads, which must stay invisible to the op-progress
    /// surface exactly like the pre-queue direct reads: their failures
    /// surface only through the command's error return. Serialization,
    /// cancellation, and close semantics are identical to `enqueue`.
    pub fn enqueue_silent<T, F>(
        &self,
        kind: &str,
        run: F,
    ) -> (String, tokio::sync::oneshot::Receiver<Result<T, String>>)
    where
        T: Send + 'static,
        F: FnOnce(OpCtx) -> Result<T, String> + Send + 'static,
    {
        self.enqueue_inner(kind, run, false, Lane::Mutation)
    }

    /// Silent read job on the read lane: a second worker that runs
    /// concurrently with the mutation lane, so a long mutation or net op
    /// never queues reads behind it. Reads still take the shared
    /// repo-handle mutex inside their job (see the module docs). Same
    /// silent, cancellation, and close semantics as [`OpQueue::enqueue_silent`].
    pub fn enqueue_read_lane<T, F>(
        &self,
        kind: &str,
        run: F,
    ) -> (String, tokio::sync::oneshot::Receiver<Result<T, String>>)
    where
        T: Send + 'static,
        F: FnOnce(OpCtx) -> Result<T, String> + Send + 'static,
    {
        self.enqueue_inner(kind, run, false, Lane::Read)
    }

    fn enqueue_inner<T, F>(
        &self,
        kind: &str,
        run: F,
        emits_progress: bool,
        lane: Lane,
    ) -> (String, tokio::sync::oneshot::Receiver<Result<T, String>>)
    where
        T: Send + 'static,
        F: FnOnce(OpCtx) -> Result<T, String> + Send + 'static,
    {
        let op_id = format!(
            "{}-op-{:x}",
            self.inner.repo_id.0,
            self.inner.counter.fetch_add(1, Ordering::Relaxed)
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.inner.closed.load(Ordering::Acquire) {
            let _ = tx.send(Err(format!(
                "repo {} is closed; {kind} op rejected",
                self.inner.repo_id.0
            )));
            return (op_id, rx);
        }

        // A silent enqueue gets no sink, which suppresses every event path
        // below (progress, done, cancelled-skip) in one place — the job
        // closure and `OpCtx` already no-op without one.
        let sink = if emits_progress {
            self.inner.sink.read().clone()
        } else {
            None
        };
        let repo_id = self.inner.repo_id.0.clone();
        let kind_owned = kind.to_string();
        let sender = match lane {
            Lane::Mutation => self.ensure_worker(),
            Lane::Read => self.ensure_read_worker(),
        };
        // M12 cancellation token: registered before the job is queued so a
        // cancel that arrives while the op waits in line skips it entirely.
        let token = Arc::new(AtomicBool::new(false));
        self.inner
            .cancels
            .lock()
            .insert(op_id.clone(), token.clone());
        // Accepted: queued or running until the job closure finishes (or
        // the send below fails because the worker is already gone).
        self.inner.depth.fetch_add(1, Ordering::Relaxed);
        // The result sender is consumed by whoever runs first: the job's
        // closure (normal path) or the enqueue call (worker gone) — hence
        // the shared slot.
        let tx = Arc::new(Mutex::new(Some(tx)));
        let job_tx = tx.clone();
        let returned_op_id = op_id.clone();
        let inner = self.inner.clone();
        let job = Job {
            run: Box::new(move || {
                // Final-event copies: `ctx` is consumed by `run`.
                let final_repo = repo_id.clone();
                let final_op = op_id.clone();
                let final_kind = kind_owned.clone();
                let ctx = OpCtx {
                    repo_id,
                    op_id: op_id.clone(),
                    kind: kind_owned,
                    sink: sink.clone(),
                    last_emit: Mutex::new(None),
                    cancel: Some(token.clone()),
                };
                // Cancelled while queued: skip execution, report as
                // cancelled (the envelope shape, matching the engine's
                // cooperative cancellation — see `IpcErrorEnvelope::cancelled`).
                if token.load(Ordering::Acquire) {
                    inner.depth.fetch_sub(1, Ordering::Relaxed);
                    inner.cancels.lock().remove(&final_op);
                    let cancelled = IpcErrorEnvelope::cancelled();
                    let error = cancelled.to_json();
                    if let Some(sink) = &sink {
                        sink.emit(&OpProgress {
                            repo_id: final_repo,
                            op_id: final_op,
                            kind: final_kind,
                            message: cancelled.message,
                            pct: None,
                            done: true,
                            error: Some(error.clone()),
                            outcome: Some(OpOutcome::Cancelled),
                        });
                    }
                    if let Some(tx) = job_tx.lock().take() {
                        let _ = tx.send(Err(error));
                    }
                    return;
                }
                // A panicking op must still resolve the caller's oneshot
                // with a clear error and emit the failed done event — an
                // unresolved promise hangs the awaiting command forever.
                let result =
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(ctx))) {
                        Ok(result) => result,
                        Err(payload) => {
                            OP_PANICS.fetch_add(1, Ordering::Relaxed);
                            tracing::error!(
                                repo = %final_repo,
                                op = %final_op,
                                kind = %final_kind,
                                panic = panic_message(&payload),
                                "op panicked"
                            );
                            Err("internal error: op worker panicked".to_string())
                        }
                    };
                // The registry entry dies with the op: a later cancel_op
                // correctly reports "unknown/finished".
                inner.depth.fetch_sub(1, Ordering::Relaxed);
                inner.cancels.lock().remove(&final_op);
                let error = result.as_ref().err().cloned();
                let event = OpProgress {
                    repo_id: final_repo,
                    op_id: final_op,
                    kind: final_kind,
                    // Display text: envelope errors contribute their message
                    // (see `error_display`); `error` keeps the wire form.
                    message: error
                        .as_deref()
                        .map(error_display)
                        .unwrap_or_else(|| "done".to_string()),
                    pct: if error.is_some() { None } else { Some(100.0) },
                    done: true,
                    outcome: Some(OpOutcome::for_error(error.as_deref())),
                    error,
                };
                if let Some(sink) = &sink {
                    sink.emit(&event);
                }
                if let Some(tx) = job_tx.lock().take() {
                    let _ = tx.send(result);
                }
            }),
        };
        if sender.send(job).is_err() {
            // Worker exited between the closed-check and the send (repo
            // closed underneath us): the job never runs, so the depth
            // counter never sees its decrement.
            self.inner.depth.fetch_sub(1, Ordering::Relaxed);
            if let Some(tx) = tx.lock().take() {
                let _ = tx.send(Err(format!(
                    "op queue for repo {} closed before the {kind} op ran",
                    self.inner.repo_id.0
                )));
            }
        }
        (returned_op_id, rx)
    }

    /// Stop accepting ops. Queued jobs still execute (drain semantics);
    /// the workers exit once the backlog is empty. Called from
    /// `RepoHandle::shutdown` (repo close / handle drop).
    pub fn shutdown(&self) {
        self.inner.closed.store(true, Ordering::Release);
        self.inner.sender.lock().take();
        self.inner.read_sender.lock().take();
    }

    /// Get the mutation worker sender, spawning the worker thread on first
    /// use.
    fn ensure_worker(&self) -> mpsc::Sender<Job> {
        let mut guard = self.inner.sender.lock();
        if let Some(sender) = guard.as_ref() {
            return sender.clone();
        }
        let tx = spawn_worker(
            &format!("op-queue-{}", self.inner.repo_id.0),
            self.inner.clone(),
            Lane::Mutation,
        );
        *guard = Some(tx.clone());
        tx
    }

    /// Get the read worker sender, spawning the worker thread on first use.
    fn ensure_read_worker(&self) -> mpsc::Sender<Job> {
        let mut guard = self.inner.read_sender.lock();
        if let Some(sender) = guard.as_ref() {
            return sender.clone();
        }
        let tx = spawn_worker(
            &format!("op-read-{}", self.inner.repo_id.0),
            self.inner.clone(),
            Lane::Read,
        );
        *guard = Some(tx.clone());
        tx
    }
}

/// Spawn one queue worker: drains until every sender (incl. the queue's own)
/// is gone. A panic that escapes a job closure (op panics are caught inside
/// it — this is the last-resort guard for a panic in the queue's own
/// bookkeeping) must not brick the queue: log it, drop the cached sender so
/// the next enqueue respawns a fresh worker, and exit this loop; the
/// channel's remaining jobs are dropped and their callers resolve via the
/// closed oneshot ("closed before completion") instead of hanging.
fn spawn_worker(name: &str, inner: Arc<QueueInner>, lane: Lane) -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            for job in rx {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (job.run)())).is_err() {
                    OP_PANICS.fetch_add(1, Ordering::Relaxed);
                    match lane {
                        Lane::Mutation => {
                            drop(inner.sender.lock().take());
                        }
                        Lane::Read => {
                            drop(inner.read_sender.lock().take());
                        }
                    }
                    tracing::error!(
                        repo = %inner.repo_id.0,
                        "op worker thread panicked; the next enqueue respawns it"
                    );
                    break;
                }
            }
        })
        .expect("spawn op queue worker thread");
    tx
}

/// Readable message from a panic payload (for the structured error log).
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

/// Handout to one running op: identity fields, the throttled emitter, and
/// the M12 cancellation token.
pub struct OpCtx {
    repo_id: String,
    op_id: String,
    kind: String,
    sink: Option<Arc<dyn EventSink>>,
    /// Last emitted (throttled) progress timestamp; `None` = never.
    last_emit: Mutex<Option<Instant>>,
    /// Cooperative cancellation token ([`OpQueue::cancel_op`] flips it);
    /// always `Some` for queue-enqueued ops.
    cancel: Option<Arc<AtomicBool>>,
}

impl OpCtx {
    /// Emit a throttled `op-progress` event (`done: false`). No-ops without
    /// a sink or when the last event for this op was less than
    /// [`THROTTLE_MS`] ago. The final `done: true` event is emitted by the
    /// queue itself and never throttled.
    pub fn emit_progress(&self, message: &str, pct: Option<f64>) {
        let now = Instant::now();
        let ready = {
            let mut last = self.last_emit.lock();
            let ready =
                throttle_ready(last.map(|then| now.duration_since(then).as_millis() as u64));
            if ready {
                *last = Some(now);
            }
            ready
        };
        if !ready {
            return;
        }
        if let Some(sink) = &self.sink {
            sink.emit(&OpProgress {
                repo_id: self.repo_id.clone(),
                op_id: self.op_id.clone(),
                kind: self.kind.clone(),
                message: message.to_string(),
                pct,
                done: false,
                error: None,
                outcome: None,
            });
        }
    }

    /// Stream one raw output line (`op-log`, unthrottled). Used for hook
    /// output and CLI fallbacks; no-ops without a sink.
    pub fn emit_log(&self, line: &str) {
        if let Some(sink) = &self.sink {
            sink.emit_log(&OpLog {
                repo_id: self.repo_id.clone(),
                op_id: self.op_id.clone(),
                kind: self.kind.clone(),
                line: line.to_string(),
            });
        }
    }

    /// True once [`OpQueue::cancel_op`] targeted this op. Long-running
    /// closures poll this between phases and bail out with an error (the
    /// queue still emits the final done event).
    // Convenience reader; netops paths take `cancel_token()` instead, so
    // this is exercised by tests only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|t| t.load(Ordering::Acquire))
    }

    /// Shared clone of the cancellation token, for passing into engine
    /// entry points that accept `Option<&AtomicBool>` (netops fetch/push).
    pub fn cancel_token(&self) -> Option<Arc<AtomicBool>> {
        self.cancel.clone()
    }
}

/// Pure throttle decision: the first event always fires, subsequent ones
/// only after [`THROTTLE_MS`] (the clone-progress throttle in
/// `ipc_commands` uses the same shape).
pub(crate) fn throttle_ready(elapsed_ms: Option<u64>) -> bool {
    match elapsed_ms {
        None => true,
        Some(ms) => ms >= THROTTLE_MS,
    }
}

#[cfg(test)]
pub(crate) mod test_sink {
    //! Test [`EventSink`] shared with other modules' unit tests.

    use super::{EventSink, OpLog, OpProgress};
    use parking_lot::Mutex;
    use std::sync::Arc;

    #[derive(Default)]
    pub struct CollectorSink {
        pub events: Mutex<Vec<OpProgress>>,
        pub logs: Mutex<Vec<OpLog>>,
    }

    impl CollectorSink {
        pub fn shared() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub fn snapshot(&self) -> Vec<OpProgress> {
            self.events.lock().clone()
        }

        pub fn log_snapshot(&self) -> Vec<OpLog> {
            self.logs.lock().clone()
        }
    }

    impl EventSink for CollectorSink {
        fn emit(&self, event: &OpProgress) {
            self.events.lock().push(event.clone());
        }

        fn emit_log(&self, event: &OpLog) {
            self.logs.lock().push(event.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_sink::CollectorSink;
    use super::*;
    use crate::engine::types::RepoId;
    use std::sync::Barrier;
    use std::time::Duration;

    /// One throttling interval for tests that need to cross it.
    const TEST_TICK: Duration = Duration::from_millis(THROTTLE_MS + 30);

    /// The cancelled envelope classifies as cancelled — the shape the
    /// engine's cooperative cancellation now produces (via `engine_err`).
    #[test]
    fn for_error_classifies_the_cancelled_envelope() {
        let envelope = IpcErrorEnvelope::cancelled().to_json();
        assert_eq!(
            OpOutcome::for_error(Some(&envelope)),
            OpOutcome::Cancelled,
            "{envelope:?}"
        );
    }

    /// The legacy bare sentinel still classifies as cancelled (one-release
    /// backward compatibility — CLI lanes and old mocks return it).
    #[test]
    fn for_error_still_classifies_the_legacy_bare_sentinel() {
        assert_eq!(
            OpOutcome::for_error(Some(CANCELLED_ERR)),
            OpOutcome::Cancelled
        );
    }

    /// Any other shape is a real failure: envelope errors with a non-
    /// cancelled code, raw strings, and malformed/garbage "envelopes".
    #[test]
    fn for_error_classifies_other_errors_as_failed() {
        let engine_env = IpcErrorEnvelope::new(ErrCode::Engine, "git error: boom").to_json();
        assert_eq!(OpOutcome::for_error(Some(&engine_env)), OpOutcome::Failed);
        // A JSON string without the version marker is not an envelope.
        let markerless = r#"{"code":"cancelled","message":"x"}"#;
        assert_eq!(OpOutcome::for_error(Some(markerless)), OpOutcome::Failed);
        // Not JSON at all — misc-lane raw string.
        assert_eq!(
            OpOutcome::for_error(Some("merge conflict")),
            OpOutcome::Failed
        );
        assert_eq!(OpOutcome::for_error(None), OpOutcome::Completed);
    }

    /// The done event's `message` field carries human text even when the
    /// error is the structured envelope; raw strings pass through.
    #[test]
    fn error_display_prefers_the_envelope_message() {
        assert_eq!(
            error_display(&IpcErrorEnvelope::new(ErrCode::Engine, "git error: boom").to_json()),
            "git error: boom"
        );
        assert_eq!(error_display("merge conflict"), "merge conflict");
    }

    fn queue() -> OpQueue {
        let queue = OpQueue::new(RepoId("test-repo".to_string()));
        queue.set_sink(CollectorSink::shared());
        queue
    }

    /// Resolve an op receiver synchronously (test threads are plain std
    /// threads, so `blocking_recv` is legal here).
    fn recv<T>(rx: tokio::sync::oneshot::Receiver<Result<T, String>>) -> Result<T, String> {
        rx.blocking_recv()
            .unwrap_or_else(|_| Err("op dropped before completion".to_string()))
    }

    #[test]
    fn throttle_ready_first_then_interval() {
        assert!(throttle_ready(None));
        assert!(!throttle_ready(Some(0)));
        assert!(!throttle_ready(Some(THROTTLE_MS - 1)));
        assert!(throttle_ready(Some(THROTTLE_MS)));
    }

    #[test]
    fn ops_run_serially_in_enqueue_order() {
        let queue = queue();
        let log = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let mut receivers = Vec::new();
        for i in 0..3 {
            let log = log.clone();
            let (_op_id, rx) = queue.enqueue("stage", move |_ctx| {
                log.lock().push(match i {
                    0 => "1:start",
                    1 => "2:start",
                    _ => "3:start",
                });
                std::thread::sleep(Duration::from_millis(15));
                log.lock().push(match i {
                    0 => "1:end",
                    1 => "2:end",
                    _ => "3:end",
                });
                Ok(())
            });
            receivers.push(rx);
        }
        for rx in receivers {
            recv(rx).expect("queued op completes");
        }
        assert_eq!(
            *log.lock(),
            vec!["1:start", "1:end", "2:start", "2:end", "3:start", "3:end"],
            "ops must not interleave"
        );
    }

    #[test]
    fn results_flow_through_oneshot() {
        let queue = queue();
        let (_id, rx) = queue.enqueue("stage", move |_ctx| Ok(7u32));
        assert_eq!(recv(rx).unwrap(), 7);

        let (_id, rx) = queue.enqueue("commit", move |_ctx| Err::<(), String>("boom".to_string()));
        assert_eq!(recv(rx).unwrap_err(), "boom");
    }

    #[test]
    fn op_ids_are_unique_and_prefixed_with_repo() {
        let queue = queue();
        let (first, rx) = queue.enqueue("stage", move |_ctx| Ok(()));
        let (second, rx2) = queue.enqueue("stage", move |_ctx| Ok(()));
        assert_ne!(first, second);
        assert!(first.starts_with("test-repo-op-"), "got {first}");
        assert!(second.starts_with("test-repo-op-"), "got {second}");
        recv(rx).unwrap();
        recv(rx2).unwrap();
    }

    #[test]
    fn done_event_shape_on_success() {
        let queue = queue();
        let sink = CollectorSink::shared();
        queue.set_sink(sink.clone());
        let (op_id, rx) = queue.enqueue("fetch", move |ctx| {
            ctx.emit_progress("halfway", Some(50.0));
            Ok("stats".to_string())
        });
        let value = recv(rx).unwrap();
        assert_eq!(value, "stats");

        let events = sink.snapshot();
        let last = events.last().expect("final done event");
        assert_eq!(last.repo_id, "test-repo");
        assert_eq!(last.op_id, op_id);
        assert_eq!(last.kind, "fetch");
        assert_eq!(last.message, "done");
        assert_eq!(last.pct, Some(100.0));
        assert!(last.done);
        assert_eq!(last.error, None);
        assert!(
            events
                .iter()
                .any(|e| !e.done && e.message == "halfway" && e.pct == Some(50.0)),
            "intermediate progress present: {events:?}"
        );

        // Contracts payload keys, exactly.
        let json = serde_json::to_value(last).unwrap();
        for key in [
            "repo_id", "op_id", "kind", "message", "pct", "done", "error",
        ] {
            assert!(json.get(key).is_some(), "missing field {key} in {json}");
        }
    }

    #[test]
    fn done_event_carries_error_on_failure() {
        let queue = queue();
        let sink = CollectorSink::shared();
        queue.set_sink(sink.clone());
        let (_op_id, rx) = queue.enqueue("push", move |_ctx| {
            Err::<(), String>("rejected".to_string())
        });
        assert_eq!(recv(rx).unwrap_err(), "rejected");
        let last = sink.snapshot().pop().expect("done event");
        assert!(last.done);
        assert_eq!(last.error.as_deref(), Some("rejected"));
        assert_eq!(last.message, "rejected");
        assert_eq!(last.pct, None);
    }

    /// Silent enqueues (queue-dispatched reads) never touch the sink: no
    /// throttled progress (even when the closure asks for it), no final
    /// done event on success, none on failure.
    #[test]
    fn silent_enqueue_emits_no_events() {
        let queue = queue();
        let sink = CollectorSink::shared();
        queue.set_sink(sink.clone());
        let (_id, rx) = queue.enqueue_silent("status", move |ctx| {
            ctx.emit_progress("halfway", Some(50.0));
            Ok(())
        });
        recv(rx).expect("silent op completes");
        let (_id, rx) =
            queue.enqueue_silent("blame", |_ctx| Err::<(), String>("bad path".to_string()));
        assert_eq!(recv(rx).unwrap_err(), "bad path");
        assert!(
            sink.snapshot().is_empty(),
            "silent enqueues emit nothing: {:?}",
            sink.snapshot()
        );
        queue.shutdown();
    }

    #[test]
    fn op_log_lines_stream_unthrottled_to_the_sink() {
        let queue = queue();
        let sink = CollectorSink::shared();
        queue.set_sink(sink.clone());
        let (_id, rx) = queue.enqueue("commit", move |ctx| {
            for i in 0..5 {
                ctx.emit_log(&format!("hook line {i}"));
            }
            Ok(())
        });
        recv(rx).expect("op completes");
        let logs = sink.log_snapshot();
        assert_eq!(logs.len(), 5, "every line is delivered: {logs:?}");
        assert_eq!(logs[0].line, "hook line 0");
        assert_eq!(logs[0].kind, "commit");
        assert_eq!(logs[4].line, "hook line 4");
    }

    #[test]
    fn rapid_progress_is_throttled_but_done_never_is() {
        let queue = queue();
        let sink = CollectorSink::shared();
        queue.set_sink(sink.clone());
        let (_id, rx) = queue.enqueue("fetch", move |ctx| {
            for i in 0..200 {
                ctx.emit_progress(&format!("burst {i}"), Some(1.0));
            }
            Ok(())
        });
        recv(rx).unwrap();
        let events = sink.snapshot();
        // First burst event + the queue's final done event; 199 throttled out.
        assert_eq!(events.len(), 2, "events: {events:?}");
        assert!(!events[0].done);
        assert!(events[1].done);
    }

    #[test]
    fn throttle_opens_again_after_interval() {
        let queue = queue();
        let sink = CollectorSink::shared();
        queue.set_sink(sink.clone());
        let (_id, rx) = queue.enqueue("fetch", move |ctx| {
            ctx.emit_progress("first", Some(10.0));
            std::thread::sleep(TEST_TICK);
            ctx.emit_progress("second", Some(80.0));
            Ok(())
        });
        recv(rx).unwrap();
        let events = sink.snapshot();
        assert_eq!(events.len(), 3, "two progress + one done: {events:?}");
        assert_eq!(events[0].message, "first");
        assert_eq!(events[1].message, "second");
    }

    #[test]
    fn enqueue_after_shutdown_rejects_immediately() {
        let queue = queue();
        queue.shutdown();
        let (_id, rx) = queue.enqueue("commit", move |_ctx| Ok(()));
        let err = recv(rx).unwrap_err();
        assert!(err.contains("closed"), "got {err}");
    }

    #[test]
    fn shutdown_drains_queued_backlog() {
        let queue = queue();
        let started = Arc::new(Barrier::new(1));
        let gate = {
            // Job 1 blocks until released; jobs 2-3 queue behind it.
            let (_id, rx) = {
                let started = started.clone();
                queue.enqueue("push", move |_ctx| {
                    started.wait(); // test releases after shutdown
                    Ok(1u32)
                })
            };
            let rx2 = {
                let (_id, rx) = queue.enqueue("push", move |_ctx| Ok(2u32));
                rx
            };
            queue.shutdown();
            started.wait();
            (rx, rx2)
        };
        assert_eq!(recv(gate.0).unwrap(), 1, "running job finishes");
        assert_eq!(recv(gate.1).unwrap(), 2, "queued backlog still drains");
    }

    #[test]
    fn works_without_sink() {
        let queue = OpQueue::new(RepoId("headless".to_string()));
        let (_id, rx) = queue.enqueue("stage", move |ctx| {
            ctx.emit_progress("dropped silently", None);
            Ok(())
        });
        recv(rx).expect("ops succeed with no sink installed");
        queue.shutdown();
    }

    // -------------------------------------------------------------------
    // M12: cooperative cancellation
    // -------------------------------------------------------------------

    #[test]
    fn cancel_queued_op_skips_execution_and_queue_continues() {
        let queue = queue();
        let gate_rx = Arc::new(Mutex::new(None::<std::sync::mpsc::Receiver<()>>));
        let (gate_tx, gate_r) = std::sync::mpsc::channel::<()>();
        *gate_rx.lock() = Some(gate_r);

        // Op 1 holds the single worker until the test releases the gate.
        let (_id1, rx1) = {
            let gate_rx = gate_rx.clone();
            queue.enqueue("push", move |_ctx| {
                if let Some(rx) = gate_rx.lock().take() {
                    let _ = rx.recv(); // blocks the worker
                }
                Ok(1u32)
            })
        };
        // Op 2 queues behind it; the test cancels it BEFORE it starts.
        let ran = Arc::new(AtomicBool::new(false));
        let (id2, rx2) = {
            let ran = ran.clone();
            queue.enqueue("push", move |_ctx| {
                ran.store(true, Ordering::SeqCst);
                Ok(2u32)
            })
        };
        assert!(queue.cancel_op("test-repo", &id2), "queued op cancellable");

        drop(gate_tx); // release op 1
        assert_eq!(recv(rx1).unwrap(), 1, "running op finishes normally");
        let skip_err = recv(rx2).unwrap_err();
        assert_eq!(
            parse_envelope(&skip_err).map(|e| e.code),
            Some(ErrCode::Cancelled),
            "queued op skipped with the cancelled envelope: {skip_err:?}"
        );
        assert!(!ran.load(Ordering::SeqCst), "skipped closure never ran");

        // The queue keeps serving: a later op runs fine.
        let (_id3, rx3) = queue.enqueue("stage", move |_ctx| Ok(3u32));
        assert_eq!(recv(rx3).unwrap(), 3);
        queue.shutdown();
    }

    #[test]
    fn cancel_running_op_is_observed_by_ctx() {
        let queue = queue();
        const MAX_ITERS: u32 = 1000;
        let entered = Arc::new(AtomicBool::new(false));
        let (id, rx) = {
            let entered = entered.clone();
            queue.enqueue("fetch", move |ctx| {
                entered.store(true, Ordering::SeqCst);
                let mut iters = 0u32;
                while iters < MAX_ITERS {
                    if ctx.cancelled() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                    iters += 1;
                }
                Ok(iters)
            })
        };
        // Cancelling before the closure runs resolves the op as "cancelled"
        // instead of handing it the token, so wait for actual entry first.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !entered.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "op never entered its closure"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(queue.cancel_op("test-repo", &id), "running op cancellable");
        let iters = recv(rx).unwrap();
        assert!(
            iters < MAX_ITERS,
            "cooperative stop must land before the loop ends (ran {iters})"
        );
        // Finished ops are no longer cancellable.
        assert!(!queue.cancel_op("test-repo", &id), "finished op unknown");
        queue.shutdown();
    }

    #[test]
    fn cancel_unknown_or_foreign_ops_return_false() {
        let queue = queue();
        assert!(!queue.cancel_op("test-repo", "test-repo-op-never-existed"));
        assert!(
            !queue.cancel_op("other-repo", "test-repo-op-1"),
            "foreign repo id never matches"
        );
        // A live op IS cancellable through the same path.
        let (id, rx) = queue.enqueue("stage", move |_ctx| Ok(()));
        recv(rx).unwrap();
        assert!(!queue.cancel_op("test-repo", &id), "finished op unknown");
        queue.shutdown();
    }

    // -------------------------------------------------------------------
    // Diagnostics telemetry: queue depth + caught panics
    // -------------------------------------------------------------------

    /// `depth` counts accepted-but-unfinished ops on both lanes and returns
    /// to zero once the backlog drains (diagnostics: `op_queue_depth`).
    #[test]
    fn depth_tracks_queued_and_running_ops() {
        let queue = queue();
        // Both ops gate on a channel so neither can complete before the
        // assertions run (a fast read op would drain the counter first).
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let gate_rx = Arc::new(Mutex::new(Some(gate_rx)));
        let (_id, rx) = {
            let gate_rx = gate_rx.clone();
            queue.enqueue("push", move |_ctx| {
                if let Some(gate) = gate_rx.lock().take() {
                    let _ = gate.recv(); // blocks the mutation lane
                }
                Ok(1u32)
            })
        };
        assert_eq!(queue.depth(), 1, "the gated op counts");
        let (gate_tx2, gate_rx2) = std::sync::mpsc::channel::<()>();
        let gate_rx2 = Arc::new(Mutex::new(Some(gate_rx2)));
        let (_rid, rx2) = {
            let gate_rx2 = gate_rx2.clone();
            queue.enqueue_read_lane("status", move |_ctx| {
                if let Some(gate) = gate_rx2.lock().take() {
                    let _ = gate.recv(); // blocks the read lane
                }
                Ok(2u32)
            })
        };
        assert_eq!(queue.depth(), 2, "both lanes count");
        drop(gate_tx);
        drop(gate_tx2); // release both gates
        recv(rx).unwrap();
        recv(rx2).unwrap();
        assert_eq!(queue.depth(), 0, "drained back to empty");
        queue.shutdown();
    }

    /// `panics_caught` increments when an op panics (the caller still
    /// resolves with the internal-error string). Process-global counter, so
    /// only the increase is asserted, never an exact value.
    #[test]
    fn panics_caught_counts_panicking_ops() {
        let queue = queue();
        let before = panics_caught();
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let (_id, rx) = queue.enqueue("commit", move |_ctx| -> Result<String, String> {
            panic!("diagnostics counter panic");
        });
        let err = recv(rx).unwrap_err();
        std::panic::set_hook(prev_hook);
        assert_eq!(err, "internal error: op worker panicked");
        assert!(panics_caught() > before, "caught panic counted");
        queue.shutdown();
    }

    // -------------------------------------------------------------------
    // Worker panic containment
    // -------------------------------------------------------------------

    /// A panicking op must not brick the repo's mutation queue: the caller
    /// resolves with a clear "internal error" (instead of hanging on the
    /// oneshot) and the queue keeps serving subsequent ops.
    #[test]
    fn panicking_op_resolves_the_caller_and_queue_keeps_serving() {
        let queue = queue();
        let sink = CollectorSink::shared();
        queue.set_sink(sink.clone());

        // The default panic hook is process-global; silence it for the
        // deliberate panic below and restore it right away.
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let (op_id, rx) = queue.enqueue("commit", move |_ctx| -> Result<String, String> {
            panic!("deliberate op panic");
        });
        let err = recv(rx).unwrap_err();
        std::panic::set_hook(prev_hook);

        assert_eq!(err, "internal error: op worker panicked", "got {err}");
        // The op registry entry is gone (finished, not wedged).
        assert!(!queue.cancel_op("test-repo", &op_id), "panicked op unknown");
        // The final done event still fired, classified as a real failure.
        let last = sink.snapshot().pop().expect("done event");
        assert!(last.done);
        assert_eq!(
            last.error.as_deref(),
            Some("internal error: op worker panicked")
        );
        assert_eq!(last.outcome, Some(OpOutcome::Failed));

        // The queue keeps serving: the next op runs fine.
        let (_id, rx2) = queue.enqueue("stage", move |_ctx| Ok(7u32));
        assert_eq!(recv(rx2).unwrap(), 7);
        queue.shutdown();
    }
}
