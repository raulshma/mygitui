//! Tests for the custom action runner (`actions.rs`): registry lifecycle,
//! spawn + streamed output capture (through the collector sink, no tauri),
//! cancellation of long-running children, per-repo concurrency cap and the
//! line budget. Cross-platform: `cmd /C` vs `sh -c` behind
//! [`ActionRegistry`] + `build_shell_command`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::actions::test_sink::CollectorActionSink;
use crate::actions::{
    run_action, ActionOutput, ActionRegistry, ActionSpec, LineBudget, MAX_CONCURRENT_PER_REPO,
    MAX_OUTPUT_LINES,
};

/// Unique scratch directory for a test (never watched, just a cwd).
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mygitui-actions-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A command that runs long enough to cancel reliably (~30s).
fn long_command() -> &'static str {
    if cfg!(windows) {
        "ping -n 30 127.0.0.1"
    } else {
        "sleep 30"
    }
}

/// Two lines on stdout and one on stderr.
fn mixed_output_command() -> &'static str {
    if cfg!(windows) {
        "echo one & echo two & echo err 1>&2"
    } else {
        "echo one; echo two; echo err 1>&2"
    }
}

fn spec(repo_id: &str, command: &str) -> ActionSpec {
    ActionSpec {
        repo_id: repo_id.to_owned(),
        name: "test action".to_owned(),
        command: command.to_owned(),
        cwd: temp_dir("cwd"),
    }
}

/// Poll the sink until the run's final `done` event lands (or timeout).
fn wait_for_done(
    sink: &Arc<CollectorActionSink>,
    run_id: &str,
    timeout: Duration,
) -> Option<ActionOutput> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(event) = sink
            .snapshot()
            .into_iter()
            .find(|event| event.run_id == run_id && event.done)
        {
            return Some(event);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

#[test]
fn registry_ids_are_unique_and_lifecycle_tracks_running() {
    let registry = ActionRegistry::default();
    let first = registry.next_run_id();
    let second = registry.next_run_id();
    assert_ne!(first, second);
    assert!(first.starts_with("act-"), "got {first}");

    let child = crate::actions::build_shell_command("echo hi")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let child = Arc::new(parking_lot::Mutex::new(child));
    registry.register(
        &first,
        "repo-1",
        child,
        crate::actions::RunContext::for_test("repo-1"),
    );
    assert_eq!(registry.running_count_for("repo-1"), 1);
    assert_eq!(registry.running_count_for("repo-2"), 0);

    assert!(registry.cancel(&first), "live run cancels");
    registry.set_done(&first);
    assert!(!registry.cancel(&first), "done run refuses cancel");
    assert_eq!(registry.running_count_for("repo-1"), 0);
}

#[test]
fn run_action_streams_lines_then_done_with_exit_code() {
    let registry = ActionRegistry::default();
    let sink = CollectorActionSink::shared();
    let launch = run_action(&registry, spec("repo-1", "echo hi"), sink.clone()).unwrap();

    let done = wait_for_done(&sink, &launch.run_id, Duration::from_secs(10))
        .expect("done event within timeout");
    assert_eq!(done.exit_code, Some(0));
    assert_eq!(done.repo_id, "repo-1");
    assert_eq!(done.name, "test action");
    assert!(done.line.is_empty(), "final event carries no line");

    let events = sink.snapshot();
    let lines: Vec<&str> = events
        .iter()
        .filter(|event| !event.done)
        .map(|event| event.line.as_str())
        .collect();
    assert_eq!(lines, vec!["hi"], "stdout captured: {events:?}");
    // Contracts payload keys, exactly.
    let json = serde_json::to_value(&done).unwrap();
    for key in ["run_id", "repo_id", "name", "line", "done", "exit_code"] {
        assert!(json.get(key).is_some(), "missing field {key} in {json}");
    }
}

#[test]
fn run_action_captures_both_streams() {
    let registry = ActionRegistry::default();
    let sink = CollectorActionSink::shared();
    let launch = run_action(
        &registry,
        spec("repo-1", mixed_output_command()),
        sink.clone(),
    )
    .unwrap();
    let done = wait_for_done(&sink, &launch.run_id, Duration::from_secs(10))
        .expect("done event within timeout");
    assert_eq!(done.exit_code, Some(0));

    let mut lines: Vec<String> = sink
        .snapshot()
        .into_iter()
        .filter(|event| !event.done)
        .map(|event| event.line.trim_end().to_owned())
        .collect();
    lines.sort();
    // cmd's `echo x & echo y` keeps the separator space in the argument,
    // hence the trim.
    assert_eq!(lines, vec!["err", "one", "two"], "stdout+stderr merged");
}

#[test]
fn run_action_reports_failure_exit_code() {
    let registry = ActionRegistry::default();
    let sink = CollectorActionSink::shared();
    let command = if cfg!(windows) {
        "cmd /C exit 3"
    } else {
        "exit 3"
    };
    let launch = run_action(&registry, spec("repo-1", command), sink.clone()).unwrap();
    let code = launch
        .exit_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("waiter resolves with the exit code");
    assert_eq!(code, 3);
    let done = wait_for_done(&sink, &launch.run_id, Duration::from_secs(1)).unwrap();
    assert_eq!(done.exit_code, Some(3));
}

#[test]
fn run_action_rejects_empty_command() {
    let registry = ActionRegistry::default();
    let sink = CollectorActionSink::shared();
    for command in ["", "   "] {
        let err = run_action(&registry, spec("repo-1", command), sink.clone()).unwrap_err();
        assert!(err.contains("must not be empty"), "got: {err}");
    }
    assert_eq!(sink.snapshot().len(), 0, "nothing emitted for rejects");
}

#[test]
fn cancel_kills_a_long_running_child() {
    let registry = ActionRegistry::default();
    let sink = CollectorActionSink::shared();
    let launch = run_action(&registry, spec("repo-1", long_command()), sink.clone()).unwrap();

    // Let it actually start, then cancel.
    std::thread::sleep(Duration::from_millis(300));
    assert!(registry.cancel(&launch.run_id), "live run cancels");

    let started = Instant::now();
    let code = launch
        .exit_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("killed run still resolves with a code");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "kill must end the ~30s command early (took {:?}, code {code})",
        started.elapsed()
    );
    let done = wait_for_done(&sink, &launch.run_id, Duration::from_secs(1)).unwrap();
    assert!(done.done);
    assert_eq!(done.exit_code, Some(code));
}

#[test]
fn cancel_unknown_run_returns_false() {
    let registry = ActionRegistry::default();
    assert!(!registry.cancel("act-deadbeef"));
}

#[test]
fn per_repo_concurrency_is_capped_at_three() {
    let registry = ActionRegistry::default();
    let sink = CollectorActionSink::shared();
    let mut launches = Vec::new();
    for _ in 0..MAX_CONCURRENT_PER_REPO {
        launches.push(run_action(&registry, spec("repo-1", long_command()), sink.clone()).unwrap());
    }
    let err = run_action(&registry, spec("repo-1", "echo hi"), sink.clone()).unwrap_err();
    assert!(err.contains("3 actions running"), "got: {err}");
    // Other repos are unaffected.
    assert!(run_action(&registry, spec("repo-2", "echo hi"), sink.clone()).is_ok());

    for launch in &launches {
        assert!(registry.cancel(&launch.run_id));
    }
    for launch in &launches {
        let _ = launch.exit_rx.recv_timeout(Duration::from_secs(10));
    }
    // Budget freed: the same repo accepts new runs again.
    assert!(run_action(&registry, spec("repo-1", "echo hi"), sink.clone()).is_ok());
}

#[test]
fn line_budget_admits_exactly_max_then_one_marker() {
    let budget = LineBudget::new(3);
    assert!(budget.take_line());
    assert!(budget.take_line());
    assert!(budget.take_line());
    // 4th line: over cap, marker fires once.
    assert!(!budget.take_line());
    assert!(budget.take_marker());
    assert!(!budget.take_marker(), "marker never repeats");
    assert!(!budget.take_line());
    assert!(!budget.take_marker());
}

#[test]
fn oversized_output_is_capped_with_marker_but_process_completes() {
    let registry = ActionRegistry::default();
    let sink = CollectorActionSink::shared();
    // MAX_OUTPUT_LINES + 5 lines; keep it cheap by echoing a long single
    // command only on unix sh (cmd quoting for this many tokens is fragile
    // — the budget unit test above covers the cap logic portably).
    if cfg!(windows) {
        return;
    }
    let command = format!(
        "i=0; while [ $i -lt {} ]; do echo line$i; i=$((i+1)); done",
        MAX_OUTPUT_LINES + 5
    );
    let launch = run_action(&registry, spec("repo-1", &command), sink.clone()).unwrap();
    let code = launch
        .exit_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("run completes despite the cap");
    assert_eq!(code, 0);

    let events = sink.snapshot();
    let emitted_lines = events.iter().filter(|event| !event.done).count();
    assert_eq!(emitted_lines, MAX_OUTPUT_LINES + 1, "cap + one marker");
    assert!(events[emitted_lines - 1].line.contains("truncated"));
    let done = events.last().unwrap();
    assert!(done.done && done.exit_code == Some(0));
    assert_eq!(registry.running_count_for("repo-1"), 0, "budget freed");
}
