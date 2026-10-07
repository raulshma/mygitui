//! Per-repo serial op queue with `op-progress` events (contracts.md M2).
//!
//! M1 read commands call the engine directly under the repo mutex —
//! concurrent reads are safe because `git2::Repository` access is fully
//! serialized by that lock. Every M2 mutation and network op instead goes
//! through [`OpQueue`] (one per [`crate::repo::RepoHandle`]): a single
//! worker thread per repo executes enqueued jobs in FIFO order, so a push
//! never races a stage and one repo's slow fetch never blocks another's
//! commit.
//!
//! Progress flows through [`OpCtx::emit_progress`], throttled to at most
//! one event per [`THROTTLE_MS`] per op; the queue itself always emits a
//! final `done: true` event when a job finishes (success or error). Events
//! reach the frontend via the [`EventSink`] abstraction — [`TauriEventSink`]
//! in production, a collector in tests — so the queue itself needs no
//! `AppHandle` to compile or run headless.
//!
//! Lifecycle: the worker thread is spawned lazily on the first `enqueue`
//! and exits once [`OpQueue::shutdown`] runs (repo close) and the backlog
//! has drained. Jobs still queued at close are executed, not dropped; their
//! oneshot receivers still resolve.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Instant;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use crate::engine::types::RepoId;

/// `op-progress` event name (contracts.md, Events M2).
pub const OP_PROGRESS_EVENT: &str = "op-progress";

/// Minimum interval between two throttled progress events for one op
/// (mirrors the clone-progress throttle in ipc_commands).
pub(crate) const THROTTLE_MS: u64 = 80;

/// `op-progress` payload — field names and shapes are fixed by
/// docs/contracts.md and mirrored in `src/lib/ipc/types.ts`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OpProgress {
    pub repo_id: String,
    pub op_id: String,
    pub kind: String,
    pub message: String,
    /// `null` until a percentage is known.
    pub pct: Option<f64>,
    /// True on the final event of an op.
    pub done: bool,
    /// `Some(msg)` when the op failed.
    pub error: Option<String>,
}

/// Where progress events go. Abstracted so the queue has no hard dependency
/// on tauri: production installs a [`TauriEventSink`] via
/// [`OpQueue::set_sink`] (see `RepoManager::open`), tests install a collector.
/// Without a sink, events are silently dropped.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &OpProgress);
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
    /// Lazily spawned worker's channel; `None` after [`OpQueue::shutdown`].
    sender: Mutex<Option<mpsc::Sender<Job>>>,
    /// Monotonic op counter (op ids are `<repo>-op-<n hex>`).
    counter: AtomicU64,
    sink: RwLock<Option<Arc<dyn EventSink>>>,
    closed: AtomicBool,
}

impl OpQueue {
    pub fn new(repo_id: RepoId) -> Self {
        Self {
            inner: Arc::new(QueueInner {
                repo_id,
                sender: Mutex::new(None),
                counter: AtomicU64::new(0),
                sink: RwLock::new(None),
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// Install (or replace) the progress sink. Called by `RepoManager::open`
    /// once an `AppHandle` exists; headless callers (tests) skip it.
    pub fn set_sink(&self, sink: Arc<dyn EventSink>) {
        *self.inner.sink.write() = Some(sink);
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

        let sink = self.inner.sink.read().clone();
        let repo_id = self.inner.repo_id.0.clone();
        let kind_owned = kind.to_string();
        let sender = self.ensure_worker();
        // The result sender is consumed by whoever runs first: the job's
        // closure (normal path) or the enqueue call (worker gone) — hence
        // the shared slot.
        let tx = Arc::new(Mutex::new(Some(tx)));
        let job_tx = tx.clone();
        let returned_op_id = op_id.clone();
        let job = Job {
            run: Box::new(move || {
                // Final-event copies: `ctx` is consumed by `run`.
                let final_repo = repo_id.clone();
                let final_op = op_id.clone();
                let final_kind = kind_owned.clone();
                let ctx = OpCtx {
                    repo_id,
                    op_id,
                    kind: kind_owned,
                    sink: sink.clone(),
                    last_emit: Mutex::new(None),
                };
                let result = run(ctx);
                let error = result.as_ref().err().cloned();
                let event = OpProgress {
                    repo_id: final_repo,
                    op_id: final_op,
                    kind: final_kind,
                    message: error.clone().unwrap_or_else(|| "done".to_string()),
                    pct: if error.is_some() { None } else { Some(100.0) },
                    done: true,
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
            // closed underneath us).
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
    /// the worker exits once the backlog is empty. Called from
    /// `RepoHandle::shutdown` (repo close / handle drop).
    pub fn shutdown(&self) {
        self.inner.closed.store(true, Ordering::Release);
        self.inner.sender.lock().take();
    }

    /// Get the worker sender, spawning the worker thread on first use.
    fn ensure_worker(&self) -> mpsc::Sender<Job> {
        let mut guard = self.inner.sender.lock();
        if let Some(sender) = guard.as_ref() {
            return sender.clone();
        }
        let (tx, rx) = mpsc::channel::<Job>();
        let name = format!("op-queue-{}", self.inner.repo_id.0);
        std::thread::Builder::new()
            .name(name)
            .spawn(move || {
                // Drains until every sender (incl. the queue's own) is gone.
                for job in rx {
                    (job.run)();
                }
            })
            .expect("spawn op queue worker thread");
        *guard = Some(tx.clone());
        tx
    }
}

/// Handout to one running op: identity fields plus the throttled emitter.
pub struct OpCtx {
    repo_id: String,
    op_id: String,
    kind: String,
    sink: Option<Arc<dyn EventSink>>,
    /// Last emitted (throttled) progress timestamp; `None` = never.
    last_emit: Mutex<Option<Instant>>,
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
            });
        }
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

    use super::{EventSink, OpProgress};
    use parking_lot::Mutex;
    use std::sync::Arc;

    #[derive(Default)]
    pub struct CollectorSink {
        pub events: Mutex<Vec<OpProgress>>,
    }

    impl CollectorSink {
        pub fn shared() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub fn snapshot(&self) -> Vec<OpProgress> {
            self.events.lock().clone()
        }
    }

    impl EventSink for CollectorSink {
        fn emit(&self, event: &OpProgress) {
            self.events.lock().push(event.clone());
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
}
