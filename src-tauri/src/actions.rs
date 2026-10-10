//! Custom shell action runner (M4, contracts.md "Commands (M4)").
//!
//! `action_run` spawns a user-defined shell command with the repository
//! workdir as its cwd and streams its output as `action-output` events
//! `{run_id, repo_id, name, line, done, exit_code}`; `action_cancel` kills
//! a running action by run id. Output is streamed raw (user-triggered, no
//! throttle) but capped at [`MAX_OUTPUT_LINES`] emitted lines — the pipe is
//! still drained past the cap so the child never blocks on a full buffer,
//! only the emission stops (one truncation marker is emitted at the cap).
//!
//! Design mirrors `ops.rs`: the emitter is abstracted behind the
//! [`ActionSink`] trait ([`TauriActionSink`] in production, a collector in
//! tests), so the runner compiles and tests headless without an
//! `AppHandle`. A process-global [`ActionRegistry`] (tauri-managed state)
//! tracks live children for cancellation; run ids are `<counter in hex>`.
//!
//! Concurrency: at most [`MAX_CONCURRENT_PER_REPO`] actions per repository
//! (the command rejects with an error past that). Each action runs on its
//! own waiter thread plus one reader thread per piped stream; the waiter
//! polls `try_wait` (never holding the child lock across the wait, so a
//! cancel can always reach `kill`), joins the readers at EOF, then emits
//! the final `done: true` event with the exit code — output lines always
//! land before the done event.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;

/// `action-output` event name (contracts.md, Events M4).
pub const ACTION_OUTPUT_EVENT: &str = "action-output";

/// Max actions running at once per repository (further `action_run` calls
/// reject until one finishes or is cancelled).
pub const MAX_CONCURRENT_PER_REPO: usize = 3;

/// Cap on emitted output lines per run; past the cap the streams are still
/// drained (the child must not block) but nothing more is emitted.
pub const MAX_OUTPUT_LINES: usize = 10_000;

/// Registered runs kept before finished ones are pruned on the next
/// register (finished runs are only needed for the concurrency count).
const MAX_TRACKED_RUNS: usize = 200;

/// How often the waiter polls `try_wait` (short enough that cancel never
/// feels late, cheap enough not to matter).
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long the waiter waits for the stream readers to hit EOF before
/// emitting the done event anyway. Normally the readers close instantly
/// with the process; a killed shell's grandchild can hold the inherited
/// pipe open for its whole lifetime, and the done event must not hang on
/// that (late output is dropped via the cancelled flag).
const READER_GRACE: Duration = Duration::from_secs(2);

/// `action-output` payload — field names and shapes are fixed by
/// docs/contracts.md and mirrored in `src/lib/ipc/types.ts`
/// (`ActionOutputEvent`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ActionOutput {
    pub run_id: String,
    pub repo_id: String,
    /// The action definition's display name (from the FE config).
    pub name: String,
    /// One output line (stdout or stderr, order not guaranteed between
    /// streams); empty on the final event.
    pub line: String,
    /// True on the final event of a run.
    pub done: bool,
    /// `null` while running; the process exit code on the final event
    /// (`-1` when no code is available, e.g. killed by a signal).
    pub exit_code: Option<i32>,
}

/// Where `action-output` events go (see `ops.rs::EventSink` for the same
/// pattern on the op queue).
pub trait ActionSink: Send + Sync {
    fn emit(&self, event: &ActionOutput);
}

/// Production sink: emits [`ActionOutput`] on the app handle.
pub struct TauriActionSink {
    app: tauri::AppHandle,
}

impl TauriActionSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl ActionSink for TauriActionSink {
    fn emit(&self, event: &ActionOutput) {
        use tauri::Emitter;
        if let Err(err) = self.app.emit(ACTION_OUTPUT_EVENT, event) {
            tracing::warn!(
                run = %event.run_id,
                error = %err,
                "emit action-output failed"
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod test_sink {
    //! Test [`ActionSink`] shared with `actions_tests`.

    use super::{ActionOutput, ActionSink};
    use parking_lot::Mutex;
    use std::sync::Arc;

    #[derive(Default)]
    pub struct CollectorActionSink {
        pub events: Mutex<Vec<ActionOutput>>,
    }

    impl CollectorActionSink {
        pub fn shared() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub fn snapshot(&self) -> Vec<ActionOutput> {
            self.events.lock().clone()
        }
    }

    impl ActionSink for CollectorActionSink {
        fn emit(&self, event: &ActionOutput) {
            self.events.lock().push(event.clone());
        }
    }
}

/// Everything needed to launch one action (validated by the caller).
pub struct ActionSpec {
    pub repo_id: String,
    pub name: String,
    pub command: String,
    /// Working directory for the child (the repo workdir root).
    pub cwd: PathBuf,
}

/// One registered run (a live child plus its bookkeeping).
struct RunEntry {
    repo_id: String,
    /// Shared with the waiter thread; `cancel` locks it briefly to `kill`
    /// while the waiter only ever holds it for one `try_wait` poll.
    child: Arc<Mutex<std::process::Child>>,
    /// Shared reader/waiter state; `cancel` flips `cancelled` so output
    /// from a killed process tree (surviving grandchildren holding the
    /// pipes) is dropped instead of arriving after the done event.
    ctx: Arc<RunContext>,
    done: bool,
}

/// Process-global registry of action runs (tauri-managed state): run id
/// allocation, cancellation, per-repo concurrency accounting.
#[derive(Default)]
pub struct ActionRegistry {
    inner: Arc<RegistryInner>,
}

impl Clone for ActionRegistry {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

#[derive(Default)]
struct RegistryInner {
    runs: Mutex<HashMap<String, RunEntry>>,
    counter: AtomicU64,
}

impl ActionRegistry {
    /// Allocate the next run id (`act-<counter hex>`).
    pub fn next_run_id(&self) -> String {
        format!(
            "act-{:x}",
            self.inner.counter.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// How many runs of `repo_id` are still live (not yet finished).
    pub fn running_count_for(&self, repo_id: &str) -> usize {
        self.inner
            .runs
            .lock()
            .values()
            .filter(|entry| entry.repo_id == repo_id && !entry.done)
            .count()
    }

    /// Register a freshly spawned child under `run_id`. Prunes finished
    /// entries when the map outgrows [`MAX_TRACKED_RUNS`].
    pub fn register(
        &self,
        run_id: &str,
        repo_id: &str,
        child: Arc<Mutex<std::process::Child>>,
        ctx: Arc<RunContext>,
    ) {
        let mut runs = self.inner.runs.lock();
        runs.insert(
            run_id.to_owned(),
            RunEntry {
                repo_id: repo_id.to_owned(),
                child,
                ctx,
                done: false,
            },
        );
        if runs.len() > MAX_TRACKED_RUNS {
            runs.retain(|_, entry| !entry.done);
        }
    }

    /// Mark a run finished (concurrency budget freed, cancel becomes a
    /// no-op for it).
    pub fn set_done(&self, run_id: &str) {
        if let Some(entry) = self.inner.runs.lock().get_mut(run_id) {
            entry.done = true;
        }
    }

    /// Kill a running child. Returns `false` when the run is unknown or
    /// already finished (never an error — cancel is best-effort).
    pub fn cancel(&self, run_id: &str) -> bool {
        let runs = self.inner.runs.lock();
        let Some(entry) = runs.get(run_id) else {
            return false;
        };
        if entry.done {
            return false;
        }
        // Late output from a killed process tree is dropped (grandchildren
        // may outlive the shell and hold the pipes).
        entry.ctx.cancelled.store(true, Ordering::Relaxed);
        // Errors are fine: the child may have exited between the done check
        // and the kill.
        let _ = entry.child.lock().kill();
        true
    }
}

/// Handle to a launched action.
#[derive(Debug)]
pub struct ActionLaunch {
    pub run_id: String,
    /// Test affordance: resolves with the exit code when the run finishes
    /// (production never reads it — the FE watches the done event).
    #[cfg(test)]
    pub exit_rx: mpsc::Receiver<i32>,
}

/// Validate + spawn `spec` and wire up streaming (see module docs). Returns
/// immediately after the spawn; output flows through the sink.
pub fn run_action(
    registry: &ActionRegistry,
    spec: ActionSpec,
    sink: Arc<dyn ActionSink>,
) -> Result<ActionLaunch, String> {
    if spec.command.trim().is_empty() {
        return Err("action command must not be empty".to_string());
    }
    if registry.running_count_for(&spec.repo_id) >= MAX_CONCURRENT_PER_REPO {
        return Err(format!(
            "repo {} already has {MAX_CONCURRENT_PER_REPO} actions running; cancel one first",
            spec.repo_id
        ));
    }

    let run_id = registry.next_run_id();
    let mut cmd = build_shell_command(&spec.command);
    cmd.current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn action `{}`: {e}", spec.name))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child = Arc::new(Mutex::new(child));

    // Test builds keep the receiver so tests can await the exact exit code;
    // production drops it (the FE observes the done event instead).
    #[cfg(test)]
    let (exit_tx, exit_rx) = mpsc::channel::<i32>();
    #[cfg(not(test))]
    let (exit_tx, _exit_rx) = mpsc::channel::<i32>();
    let ctx = Arc::new(RunContext {
        run_id: run_id.clone(),
        repo_id: spec.repo_id.clone(),
        name: spec.name,
        sink,
        budget: LineBudget::new(MAX_OUTPUT_LINES),
        cancelled: AtomicBool::new(false),
    });
    registry.register(&run_id, &spec.repo_id, child.clone(), ctx.clone());

    // One reader per stream; each signals its channel at EOF. The waiter
    // waits for the signals (bounded by READER_GRACE) instead of joining,
    // so a killed shell's grandchild holding the inherited pipe can never
    // stall the done event.
    let mut closed = Vec::new();
    if let Some(stream) = stdout {
        closed.push(spawn_reader(stream, ctx.clone(), &run_id, "out")?);
    }
    if let Some(stream) = stderr {
        closed.push(spawn_reader(stream, ctx.clone(), &run_id, "err")?);
    }

    let wait_ctx = ctx.clone();
    let wait_run_id = run_id.clone();
    let wait_registry = registry.clone();
    std::thread::Builder::new()
        .name(format!("action-{run_id}"))
        .spawn(move || {
            let code = wait_for_exit(&child, closed).unwrap_or(-1);
            wait_registry.set_done(&wait_run_id);
            wait_ctx.emit(ActionOutput {
                run_id: wait_run_id,
                repo_id: wait_ctx.repo_id.clone(),
                name: wait_ctx.name.clone(),
                line: String::new(),
                done: true,
                exit_code: Some(code),
            });
            let _ = exit_tx.send(code);
        })
        .map_err(|e| format!("failed to spawn action waiter: {e}"))?;

    Ok(ActionLaunch {
        run_id,
        #[cfg(test)]
        exit_rx,
    })
}

/// The platform shell wrapping: `cmd /C` on Windows, `sh -c` elsewhere.
pub fn build_shell_command(command: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = crate::process::command("cmd");
        cmd.args(["/C", command]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    }
}

/// Spawn one stream reader thread: drains lines into the context until EOF,
/// then signals its channel (the waiter's EOF proof).
fn spawn_reader(
    stream: impl Read + Send + 'static,
    ctx: Arc<RunContext>,
    run_id: &str,
    which: &str,
) -> Result<mpsc::Receiver<()>, String> {
    let (closed_tx, closed_rx) = mpsc::channel::<()>();
    std::thread::Builder::new()
        .name(format!("action-{run_id}-{which}"))
        .spawn(move || {
            drain_stream(stream, &ctx);
            let _ = closed_tx.send(());
        })
        .map_err(|e| format!("failed to spawn action reader: {e}"))?;
    Ok(closed_rx)
}

/// Read `stream` line-by-line (lossy UTF-8, \n / \r\n stripped), emitting
/// each through the budget. Drains to EOF even past the emission cap.
fn drain_stream(stream: impl Read, ctx: &RunContext) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break, // EOF
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                ctx.emit_line(text.trim_end_matches(['\n', '\r']).to_owned());
            }
            Err(_) => break, // pipe error: the child is gone
        }
    }
}

/// Wait for the process to exit (polling so the child lock is never held
/// across the wait — cancel must always be able to reach `kill`), then give
/// the readers a bounded grace to hit EOF. Returns the exit status code
/// (-1 when the process has none, e.g. killed by a signal).
fn wait_for_exit(
    child: &Arc<Mutex<std::process::Child>>,
    closed: Vec<mpsc::Receiver<()>>,
) -> std::io::Result<i32> {
    let status = loop {
        let polled = child.lock().try_wait();
        match polled {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(WAIT_POLL_INTERVAL),
            Err(err) => return Err(err),
        }
    };
    // All lines the sink will ever get for this run are in before the last
    // EOF signal; past the grace (orphaned grandchild holding the pipe) the
    // done event proceeds anyway — late output is dropped via `cancelled`.
    for signal in closed {
        let _ = signal.recv_timeout(READER_GRACE);
    }
    Ok(status.code().unwrap_or(-1))
}

/// Shared per-run state for the reader/waiter threads.
pub(crate) struct RunContext {
    run_id: String,
    repo_id: String,
    name: String,
    sink: Arc<dyn ActionSink>,
    budget: LineBudget,
    /// Set by `cancel`: all further output lines (and the marker) are
    /// dropped, since a killed shell's grandchildren may keep feeding the
    /// inherited pipes after the done event.
    cancelled: AtomicBool,
}

impl RunContext {
    fn emit(&self, event: ActionOutput) {
        self.sink.emit(&event);
    }

    /// Emit one output line, honoring the line budget (see module docs).
    fn emit_line(&self, line: String) {
        if self.cancelled.load(Ordering::Relaxed) {
            return;
        }
        if !self.budget.take_line() {
            if self.budget.take_marker() {
                self.emit(ActionOutput {
                    run_id: self.run_id.clone(),
                    repo_id: self.repo_id.clone(),
                    name: self.name.clone(),
                    line: format!(
                        "— output truncated at {} lines; still running —",
                        self.budget.max
                    ),
                    done: false,
                    exit_code: None,
                });
            }
            return;
        }
        self.emit(ActionOutput {
            run_id: self.run_id.clone(),
            repo_id: self.repo_id.clone(),
            name: self.name.clone(),
            line,
            done: false,
            exit_code: None,
        });
    }
}

/// Emission budget for one run: the first [`LineBudget::max`] lines pass,
/// exactly one truncation marker fires once the cap is crossed, everything
/// after is dropped (the stream is still drained by the reader).
pub(crate) struct LineBudget {
    pub(crate) max: usize,
    emitted: AtomicUsize,
    marker_sent: AtomicBool,
}

impl LineBudget {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            max,
            emitted: AtomicUsize::new(0),
            marker_sent: AtomicBool::new(false),
        }
    }

    /// Reserve one emitted line; `false` past the cap.
    pub(crate) fn take_line(&self) -> bool {
        self.emitted.fetch_add(1, Ordering::Relaxed) < self.max
    }

    /// `true` exactly once, on the first over-cap line.
    pub(crate) fn take_marker(&self) -> bool {
        self.emitted.load(Ordering::Relaxed) > self.max
            && !self.marker_sent.swap(true, Ordering::Relaxed)
    }
}

#[cfg(test)]
impl RunContext {
    /// Registry-lifecycle tests need a context to register; the collector
    /// sink keeps it observable.
    pub(crate) fn for_test(repo_id: &str) -> Arc<RunContext> {
        Arc::new(Self {
            run_id: String::new(),
            repo_id: repo_id.to_owned(),
            name: String::new(),
            sink: test_sink::CollectorActionSink::shared(),
            budget: LineBudget::new(MAX_OUTPUT_LINES),
            cancelled: AtomicBool::new(false),
        })
    }
}
