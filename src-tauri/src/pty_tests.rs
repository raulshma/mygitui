//! Tests for the PTY session manager (`pty.rs`): spawn + output capture on
//! a short-lived shell, frontend write reaching an interactive shell,
//! resize, explicit kill cleanup, the concurrent-session cap and the
//! exit-event/unregister lifecycle. Cross-platform: `cmd` vs `sh`/`cat`
//! behind [`spawn_session`] + [`CommandBuilder`]. Real ptys are used
//! throughout (ConPTY is available on Windows CI runners, termios on unix).

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use portable_pty::CommandBuilder;

use crate::pty::{
    coalesce_ready, kill_session, resize_session, spawn_session, strip_cursor_probes,
    write_to_session, PtyExit, PtyOutput, PtyRegistry, PtySink, SessionSpec, COALESCE_WINDOW,
    MAX_EVENT_BYTES, MAX_SESSIONS,
};

/// How long output/exit assertions wait before failing (ConPTY/termios
/// spawns are fast; this is CI-runner slack).
const TIMEOUT: Duration = Duration::from_secs(5);

/// Budget for a kill to actually terminate a long child (spec: 2 s from
/// `pty_kill` to the session's `pty-exit` event).
const KILL_BUDGET: Duration = Duration::from_secs(2);

/// Slack for kills whose exit event is observed without the budget claim
/// (kill propagation through the exit waiter on a loaded CI runner).
const KILL_TIMEOUT: Duration = Duration::from_secs(5);

/// Unique marker fragment so no other output can satisfy a contains-check.
fn marker(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{tag}-{}-{nanos}", std::process::id())
}

/// Poll `pred` until it holds or the deadline passes.
fn wait_for(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if pred() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Test [`PtySink`]: collects output and exit events for assertions.
#[derive(Default)]
struct CollectorPtySink {
    outputs: Mutex<Vec<PtyOutput>>,
    exits: Mutex<Vec<PtyExit>>,
}

impl CollectorPtySink {
    fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// All output data joined (output may arrive in any chunking).
    fn joined_output(&self) -> String {
        self.outputs
            .lock()
            .iter()
            .map(|e| e.data.as_str())
            .collect()
    }

    fn outputs_with(&self, needle: &str) -> Vec<PtyOutput> {
        self.outputs
            .lock()
            .iter()
            .filter(|e| e.data.contains(needle))
            .cloned()
            .collect()
    }

    fn exits(&self) -> Vec<PtyExit> {
        self.exits.lock().clone()
    }
}

impl PtySink for CollectorPtySink {
    fn emit_output(&self, event: &PtyOutput) {
        self.outputs.lock().push(event.clone());
    }

    fn emit_exit(&self, event: &PtyExit) {
        self.exits.lock().push(event.clone());
    }
}

/// A scratch cwd (never watched, just a spawn directory).
fn spec(command: Option<CommandBuilder>) -> SessionSpec {
    SessionSpec {
        cwd: std::env::temp_dir(),
        rows: 24,
        cols: 80,
        command,
    }
}

/// Short-lived session: echoes `marker` and exits (exit code 0).
fn echo_command(marker: &str) -> CommandBuilder {
    #[cfg(windows)]
    {
        let mut cmd = CommandBuilder::new("cmd");
        cmd.args(["/C", &format!("echo {marker}")]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", &format!("echo {marker}")]);
        cmd
    }
}

/// Long-lived session that outlives the assertions until killed.
fn blocking_command() -> CommandBuilder {
    #[cfg(windows)]
    {
        let mut cmd = CommandBuilder::new("cmd");
        cmd.args(["/C", "ping -n 30 127.0.0.1"]);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = CommandBuilder::new("sleep");
        cmd.arg("30");
        cmd
    }
}

/// Interactive stdin-echoing session for the write test (`cmd` echoes its
/// typed input through ConPTY, `cat` through the tty line discipline).
fn interactive_command() -> CommandBuilder {
    #[cfg(windows)]
    {
        CommandBuilder::new("cmd")
    }
    #[cfg(not(windows))]
    {
        CommandBuilder::new("cat")
    }
}

/// Pure coalescing decision (no live pty): bursts flush on the 16 ms
/// window or the event-size cap, never while empty.
#[test]
fn coalesce_ready_pure_decision() {
    // An empty buffer never flushes, no matter how long it sat.
    assert!(!coalesce_ready(0, Duration::ZERO));
    assert!(!coalesce_ready(0, COALESCE_WINDOW * 10));

    // Fresh burst under the cap: keep accumulating until the window closes.
    assert!(!coalesce_ready(1, Duration::ZERO));
    assert!(!coalesce_ready(4096, COALESCE_WINDOW / 2));

    // Window elapsed: flush whatever accumulated.
    assert!(coalesce_ready(1, COALESCE_WINDOW));
    assert!(coalesce_ready(
        4096,
        COALESCE_WINDOW + Duration::from_millis(1)
    ));

    // Size cap: flush immediately regardless of age (burst split).
    assert!(coalesce_ready(MAX_EVENT_BYTES, Duration::ZERO));
    assert!(coalesce_ready(MAX_EVENT_BYTES + 1, COALESCE_WINDOW / 2));
}

/// Pure probe-stripping: conhost's startup cursor probe (`ESC[6n`) is
/// answered and removed backend-side (see pty.rs); split-across-chunks and
/// lookalike sequences are covered here without a live pty.
#[test]
fn cursor_probe_stripped_and_counted() {
    // Probe fully inside one chunk: stripped, counted, rest forwarded.
    let mut carry = Vec::new();
    let (out, answers) = strip_cursor_probes(b"\x1b[6nhello", &mut carry);
    assert_eq!(answers, 1);
    assert_eq!(out, b"hello");
    assert!(carry.is_empty());

    // Probe split across two chunks is still caught (partial carry).
    let mut carry = Vec::new();
    let (out, answers) = strip_cursor_probes(b"\x1b[", &mut carry);
    assert_eq!(answers, 0);
    assert!(out.is_empty());
    assert_eq!(carry, b"\x1b[");
    let (out, answers) = strip_cursor_probes(b"6n\x1b[m", &mut carry);
    assert_eq!(answers, 1);
    assert_eq!(out, b"\x1b[m");
    assert!(carry.is_empty());

    // Probe split byte-by-byte across three chunks.
    let mut carry = Vec::new();
    let (_, a1) = strip_cursor_probes(b"\x1b", &mut carry);
    let (_, a2) = strip_cursor_probes(b"[6", &mut carry);
    let (out, a3) = strip_cursor_probes(b"n", &mut carry);
    assert_eq!(a1 + a2 + a3, 1);
    assert!(out.is_empty());

    // Lookalikes pass through untouched (`ESC[6x` is not a probe) and
    // multiple probes in one chunk each count.
    let mut carry = Vec::new();
    let (out, answers) = strip_cursor_probes(b"\x1b[6x\x1b[6n\x1b[6nend", &mut carry);
    assert_eq!(answers, 2);
    assert_eq!(out, b"\x1b[6xend");

    // No probes at all: byte-identical passthrough.
    let mut carry = Vec::new();
    let (out, answers) = strip_cursor_probes(b"\x1b[?25h\x1b]0;title\x07plain", &mut carry);
    assert_eq!(answers, 0);
    assert_eq!(out, b"\x1b[?25h\x1b]0;title\x07plain");
}

#[test]
fn output_capture_exit_event_and_unregister() {
    let registry = PtyRegistry::default();
    let sink = CollectorPtySink::shared();
    let tag = marker("ptytest");

    let session_id =
        spawn_session(&registry, spec(Some(echo_command(&tag))), sink.clone()).expect("spawn");

    assert!(
        registry.get(&session_id).is_some(),
        "session must be registered right after spawn"
    );

    // Output arrives coalesced through the sink until it contains the tag.
    assert!(
        wait_for(TIMEOUT, || !sink.outputs_with(&tag).is_empty()),
        "no pty-output containing {tag}; got: {}",
        sink.joined_output()
    );
    let hit = &sink.outputs_with(&tag)[0];
    // Payload keys, exactly (contracts.md M5). serde_json's Value returns
    // object keys sorted, so compare the sorted key set.
    let json = serde_json::to_value(hit).unwrap();
    assert_eq!(
        json.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["data", "session_id"],
        "pty-output payload keys: {json}"
    );
    assert_eq!(json["session_id"], session_id);

    // The shell exits on its own; the waiter unregisters and emits the
    // final pty-exit with the real exit code.
    assert!(
        wait_for(TIMEOUT, || !sink.exits().is_empty()),
        "no pty-exit event; got: {}",
        sink.joined_output()
    );
    let exit = &sink.exits()[0];
    assert_eq!(exit.session_id, session_id);
    assert_eq!(exit.code, 0, "echo exits cleanly");
    let json = serde_json::to_value(exit).unwrap();
    assert_eq!(
        json.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["code", "session_id"],
        "pty-exit payload keys: {json}"
    );
    assert_eq!(
        registry.live_count(),
        0,
        "exited session must be unregistered"
    );
}

#[test]
fn write_reaches_shell_then_resize_and_kill_cleanup() {
    let registry = PtyRegistry::default();
    let sink = CollectorPtySink::shared();
    let tag = marker("ptywrite");

    let session_id =
        spawn_session(&registry, spec(Some(interactive_command())), sink.clone()).expect("spawn");

    // Frontend keystrokes flow to the shell; the echo (ConPTY for `cmd`,
    // the tty line discipline for `cat`) brings the marker back as output.
    let written = if cfg!(windows) {
        format!("echo {tag}\r\n")
    } else {
        format!("{tag}\n")
    };
    write_to_session(&registry, &session_id, &written).expect("write");

    assert!(
        wait_for(TIMEOUT, || !sink.outputs_with(&tag).is_empty()),
        "written data never echoed back; got: {}",
        sink.joined_output()
    );

    // Resize a live session succeeds; an unknown session is an error.
    resize_session(&registry, &session_id, 30, 120).expect("resize live session");
    assert!(
        resize_session(&registry, "pty-does-not-exist", 30, 120).is_err(),
        "resize of unknown session must fail"
    );

    // Kill cleans up: entry gone, second kill is an error, and the kill
    // still surfaces as the pty-exit event from the exit waiter.
    kill_session(&registry, &session_id).expect("kill live session");
    assert_eq!(
        registry.live_count(),
        0,
        "killed session must be unregistered"
    );
    assert!(
        kill_session(&registry, &session_id).is_err(),
        "double kill must fail"
    );
    assert!(
        wait_for(KILL_TIMEOUT, || !sink.exits().is_empty()),
        "kill must produce a pty-exit event"
    );
    assert_eq!(sink.exits()[0].session_id, session_id);
}

/// A killed long child must die promptly: from `pty_kill` to the session's
/// `pty-exit` event no more than [`KILL_BUDGET`] may pass (spec budget).
#[test]
fn kill_terminates_long_child_within_budget() {
    let registry = PtyRegistry::default();
    let sink = CollectorPtySink::shared();

    let session_id = spawn_session(&registry, spec(Some(blocking_command())), sink.clone())
        .expect("spawn blocking session");
    let started = Instant::now();
    kill_session(&registry, &session_id).expect("kill long child");

    assert!(
        wait_for(KILL_BUDGET, || !sink.exits().is_empty()),
        "killed long child's pty-exit must arrive within {KILL_BUDGET:?}; got: {}",
        sink.joined_output()
    );
    assert!(
        started.elapsed() <= KILL_BUDGET + Duration::from_millis(100),
        "kill-to-exit must stay within the 2 s budget, took {:?}",
        started.elapsed()
    );
    assert_eq!(sink.exits()[0].session_id, session_id);
    assert_eq!(
        registry.live_count(),
        0,
        "killed session must be unregistered"
    );
}

#[test]
fn session_cap_rejects_ninth_and_kills_release_slots() {
    let registry = PtyRegistry::default();
    let sink = CollectorPtySink::shared();

    let mut ids = Vec::new();
    for _ in 0..MAX_SESSIONS {
        ids.push(
            spawn_session(&registry, spec(Some(blocking_command())), sink.clone())
                .expect("spawn blocking session"),
        );
    }
    let err = spawn_session(&registry, spec(Some(blocking_command())), sink.clone())
        .expect_err("9th session must be rejected");
    assert!(err.contains("already"), "cap error mentions the cap: {err}");

    // Kill every session: slots free up immediately.
    for id in &ids {
        kill_session(&registry, id).expect("kill");
    }
    assert_eq!(registry.live_count(), 0);

    let fresh = spawn_session(&registry, spec(Some(blocking_command())), sink.clone())
        .expect("slot freed after kills");
    kill_session(&registry, &fresh).expect("kill fresh session");
}

#[test]
fn registry_drop_kills_remaining_children() {
    let registry = PtyRegistry::default();
    let sink = CollectorPtySink::shared();
    spawn_session(&registry, spec(Some(blocking_command())), sink.clone())
        .expect("spawn blocking session");
    spawn_session(&registry, spec(Some(blocking_command())), sink.clone())
        .expect("spawn blocking session");
    assert_eq!(registry.live_count(), 2);

    // App-exit path: dropping the registry kills (and forgets) everything.
    // Observable: each orphaned session still fires its pty-exit, proving
    // the children actually died rather than leaking.
    drop(registry);
    assert!(
        wait_for(KILL_TIMEOUT, || sink.exits().len() == 2),
        "registry Drop must kill children (one pty-exit each); got {} exit(s)",
        sink.exits().len()
    );
    let fresh = PtyRegistry::default();
    assert_eq!(fresh.live_count(), 0);
}
