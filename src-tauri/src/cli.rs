//! CLI fallback for engine capabilities libgit2 cannot reach.
//!
//! M2 use: `commit`. The libgit2 engine routes commits behind active hooks
//! or signing here (`EngineError::Unsupported` — hooks/signing shells are
//! exactly what libgit2 does not run); real `git commit` executes them.
//! Later milestones reuse the same pattern for partial clone and
//! sparse-checkout edges (capability table).
//!
//! Rules for every CLI invocation (mirrors the engine CONTRACT in
//! git_engine.rs): run system `git` with `cwd` set to the repo workdir,
//! inherit **nothing** from the environment implicitly — only `GIT_*`
//! variables (signing, hooks, proxies) plus the minimal system vars git
//! needs to run pass through ([`sanitize_env`]). The commit message always
//! travels via `-F <temp file>` so no shell quoting is ever involved.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::engine::types::{CommitOptions, NetStats, PushOptions};

/// Poll tick for the cancel-aware child wait (see [`git_stream`]): small
/// enough that `op_cancel` feels instant, coarse enough to never matter
/// next to a real git operation.
const CANCEL_POLL: Duration = Duration::from_millis(150);

/// Grace window for draining the reader threads after a cancel-kill. The
/// killed git's own pipes close (its readers hit EOF), but hook
/// grandchildren can inherit the pipe ends and keep the readers from EOF
/// forever — the drain is bounded so a leaked grandchild cannot wedge the
/// op lane (same shape as the forge runner's reader grace).
const KILL_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Non-`GIT_*` environment keys passed through to the child git: enough for
/// it to locate its config, crypto, and helpers on Windows and Unix.
const ENV_ALLOWLIST: [&str; 21] = [
    // Windows: crypto/config resolution breaks without these.
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "COMSPEC",
    "PATHEXT",
    "PROGRAMFILES",
    "PROGRAMDATA",
    "APPDATA",
    "LOCALAPPDATA",
    "ALLUSERSPROFILE",
    "USERNAME",
    "USERPROFILE",
    // Cross-platform: git binary lookup, config + tmp.
    "PATH",
    "HOME",
    "HOMEDRIVE",
    "HOMEPATH",
    "TMP",
    "TEMP",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "XDG_CONFIG_HOME",
];

/// Pure: filter environment pairs down to `GIT_*` + [`ENV_ALLOWLIST`].
pub(crate) fn sanitize_env<I>(vars: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    let windows = cfg!(windows);
    vars.into_iter()
        .filter(|(key, _)| {
            // Windows environment names are case-insensitive, while
            // std::env preserves the original spelling (for example
            // `SystemRoot` and `ProgramFiles`). Keep those required entries.
            let is_git = if windows {
                key.get(..4)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("GIT_"))
            } else {
                key.starts_with("GIT_")
            };
            let is_allowed = ENV_ALLOWLIST.iter().any(|allowed| {
                if windows {
                    allowed.eq_ignore_ascii_case(key)
                } else {
                    *allowed == key
                }
            });
            is_git || is_allowed
        })
        .collect()
}

/// Pure: is this env key on the pass-through list? (Shared with the
/// maintenance/rebase-exec runners.)
/// Clears `cmd`'s environment, then re-applies the sanitized set
/// ([`sanitize_env`] over the parent env). Every spawned git must go
/// through this — the allowlist has exactly one home.
pub(crate) fn apply_sanitized_env(cmd: &mut Command) {
    cmd.env_clear();
    for (key, value) in sanitize_env(std::env::vars()) {
        cmd.env(key, value);
    }
}

/// Pure: argv for `git commit`. The message always goes through `-F` with a
/// file argument (quoting-proof); flags map 1:1 from [`CommitOptions`].
pub(crate) fn build_commit_args(message_file: &str, opts: &CommitOptions) -> Vec<String> {
    let mut args = vec![
        "commit".to_string(),
        "-F".to_string(),
        message_file.to_string(),
    ];
    if opts.amend {
        args.push("--amend".to_string());
    }
    if opts.no_verify {
        args.push("--no-verify".to_string());
    }
    if opts.allow_empty {
        args.push("--allow-empty".to_string());
    }
    if let Some(author) = &opts.author {
        // Author date is not carried (CommitOptions has no date field).
        args.push("--author".to_string());
        args.push(format!("{} <{}>", author.name, author.email));
    }
    args
}

/// Pure: argv for `git push`. Mirrors [`PushOptions`] 1:1; `--force-with-lease`
/// is the real CLI flag (the libgit2 path approximates it).
pub(crate) fn build_push_args(opts: &PushOptions) -> Vec<String> {
    let mut args = vec!["push".to_string()];
    if opts.delete {
        args.push("--delete".to_string());
    }
    if opts.tags {
        args.push("--tags".to_string());
    }
    if opts.force {
        args.push("--force".to_string());
    }
    if opts.force_with_lease {
        args.push("--force-with-lease".to_string());
    }
    if opts.set_upstream {
        args.push("--set-upstream".to_string());
    }
    args.push(opts.remote.clone());
    if !opts.refs.is_empty() {
        args.extend(opts.refs.iter().cloned());
    } else if !opts.branch.is_empty() {
        args.push(opts.branch.clone());
    }
    args
}

/// Run `git <args>` in `workdir` with the sanitized environment; success
/// returns trimmed stdout; failure returns stderr, falling back to stdout
/// (some failures, e.g. "nothing to commit", print there). `cancel` is the
/// op's cancellation token — polled while git runs (see [`git_stream`]).
fn git(workdir: &Path, args: &[String], cancel: Option<&AtomicBool>) -> Result<String, String> {
    // Same polled child as the streaming variants (one cancel-aware wait
    // loop for every CLI invocation — a plain `output()` wait would hang
    // forever on a wedged git); lines are folded into the captured
    // streams instead of being forwarded.
    let output = git_stream(workdir, args, &mut |_| {}, cancel)?;
    Ok(output.stdout)
}

/// Captured output of a streamed child, split by stream.
#[derive(Debug, Default)]
struct StreamOutput {
    stdout: String,
    stderr: String,
}

/// Run `git <args>` while streaming stdout/stderr lines to `on_line`.
/// Capturing both streams keeps the failure diagnostics identical to
/// [`git`]; the callback is invoked on the calling thread so it may borrow
/// the op context (it is not `Send`).
///
/// Cancellation: `cancel` is the op's token (`OpCtx::cancel_token`). The
/// child is polled with `try_wait` on a fixed tick and the token checked
/// between ticks — a blocking `wait()` would consult neither, so a hung
/// hook used to wedge the mutation lane forever. On cancel the child is
/// killed and the shared [`crate::ops::CANCELLED_ERR`] is returned (the
/// same error shape `op_cancel` produces for the libgit2 net ops, so the
/// queue classifies the op as `cancelled`). Deliberately NO deadline: a
/// slow hook (a release build inside `pre-commit`) is legitimate work and
/// only the user's op-cancel may stop it — an aggressive timeout would
/// kill honest commits.
fn git_stream(
    workdir: &Path,
    args: &[String],
    on_line: &mut dyn FnMut(&str),
    cancel: Option<&AtomicBool>,
) -> Result<StreamOutput, String> {
    let mut cmd = crate::process::command("git");
    cmd.current_dir(workdir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_sanitized_env(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed to spawn git: {err}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture git stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture git stderr".to_string())?;

    let (tx, rx) = mpsc::channel::<(bool, String)>();
    let out_tx = tx.clone();
    let out_reader = std::thread::spawn(move || read_stream_lines(stdout, true, out_tx));
    let err_reader = std::thread::spawn(move || read_stream_lines(stderr, false, tx));

    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    // Cancellation-aware wait: fold lines as they arrive; between ticks
    // poll the child and check the token. The token is consulted on every
    // wake-up (lines flowing or not), so even a chatty child cannot delay
    // the abort past one tick.
    let mut status: Option<ExitStatus> = None;
    let cancelled = loop {
        match rx.recv_timeout(CANCEL_POLL) {
            Ok((is_stdout, line)) => {
                if is_stdout {
                    stdout_text.push_str(&line);
                    stdout_text.push('\n');
                } else {
                    stderr_text.push_str(&line);
                    stderr_text.push('\n');
                }
                if !line.trim().is_empty() {
                    on_line(&line);
                }
            }
            // Both readers hit EOF: the child's streams are closed.
            Err(mpsc::RecvTimeoutError::Disconnected) => break false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
            break true;
        }
        match child.try_wait() {
            Ok(Some(exit)) => status = Some(exit),
            Ok(None) => {}
            Err(err) => return Err(format!("failed to wait for git: {err}")),
        }
    };

    if cancelled {
        // Kill so the child's pipes close and the readers hit EOF; drain
        // them bounded (hook grandchildren may inherit the pipe ends and
        // never EOF) and detach the threads — they exit whenever the
        // pipes finally close, and their captured output no longer
        // matters.
        let _ = child.kill();
        let _ = child.wait();
        let deadline = Instant::now() + KILL_DRAIN_GRACE;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(_) => {} // trailing output, discarded
                Err(_) => break,
            }
        }
        drop(out_reader);
        drop(err_reader);
        return Err(crate::ops::CANCELLED_ERR.to_string());
    }

    // Child exited (status observed above) or its streams closed: drain
    // the trailing lines so failure diagnostics and the push report keep
    // every byte, then reap.
    for (is_stdout, line) in rx {
        if is_stdout {
            stdout_text.push_str(&line);
            stdout_text.push('\n');
        } else {
            stderr_text.push_str(&line);
            stderr_text.push('\n');
        }
        if !line.trim().is_empty() {
            on_line(&line);
        }
    }
    let _ = out_reader.join();
    let _ = err_reader.join();
    let status = match status {
        Some(status) => status,
        None => child
            .wait()
            .map_err(|err| format!("failed to wait for git: {err}"))?,
    };

    if status.success() {
        Ok(StreamOutput {
            stdout: stdout_text.trim().to_string(),
            stderr: stderr_text.trim().to_string(),
        })
    } else {
        let detail = if stderr_text.trim().is_empty() {
            stdout_text.trim().to_string()
        } else {
            stderr_text.trim().to_string()
        };
        let head = args.first().map(String::as_str).unwrap_or_default();
        Err(format!("git {head} failed: {detail}"))
    }
}

/// Reads one child stream line by line (lossy UTF-8) into the channel.
fn read_stream_lines<R: Read>(reader: R, is_stdout: bool, tx: mpsc::Sender<(bool, String)>) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim_end_matches(['\r', '\n']);
                let _ = tx.send((is_stdout, line.to_string()));
            }
            Err(_) => break,
        }
    }
}

/// Commit via the git CLI (hooks + signing run for real). Returns the new
/// HEAD sha (`git rev-parse HEAD` after committing). Every stdout/stderr line
/// is handed to `on_line` while the command runs. `cancel` is the op's
/// cancellation token (`OpCtx::cancel_token`): a hung hook is killed and the
/// shared "cancelled" error returned instead of wedging the op lane — see
/// [`git_stream`].
pub fn commit_via_cli(
    workdir: &Path,
    opts: &CommitOptions,
    on_line: &mut dyn FnMut(&str),
    cancel: Option<&AtomicBool>,
) -> Result<String, String> {
    let file = write_message_file("commit", &opts.message)?;
    let result = (|| {
        // A cancel that raced the engine phase must not spawn git at all.
        if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
            return Err(crate::ops::CANCELLED_ERR.to_string());
        }
        let args = build_commit_args(&file.to_string_lossy(), opts);
        git_stream(workdir, &args, on_line, cancel)?;
        git(
            workdir,
            &["rev-parse".to_string(), "HEAD".to_string()],
            cancel,
        )
    })();
    let _ = std::fs::remove_file(&file);
    result
}

/// Push via the git CLI so a `pre-push` hook really runs (and streams its
/// output). Returns the refs reported by `git push`; the report is written to
/// stderr alongside progress, so parse both streams. `cancel` is the op's
/// cancellation token (see [`commit_via_cli`] / [`git_stream`]).
pub fn push_via_cli(
    workdir: &Path,
    opts: &PushOptions,
    on_line: &mut dyn FnMut(&str),
    cancel: Option<&AtomicBool>,
) -> Result<NetStats, String> {
    // A cancel that raced the queue-skip check must not spawn git.
    if cancel.is_some_and(|token| token.load(Ordering::Acquire)) {
        return Err(crate::ops::CANCELLED_ERR.to_string());
    }
    let args = build_push_args(opts);
    let output = git_stream(workdir, &args, on_line, cancel)?;
    let report = format!("{}\n{}", output.stdout, output.stderr);
    let updated_refs = crate::engine::netops::parse_push_report(&report);
    Ok(NetStats {
        received_bytes: 0,
        objects: updated_refs.len() as u32,
        updated_refs,
    })
}

/// Unique temp file for a commit/tag message handed to git via `-F`
/// (removed by the caller). Shared by the commit path and the tag-sign
/// path (`prefix` distinguishes the file names). Created owner-only:
/// messages sit in the shared temp dir, so 0600 keeps other local users
/// from reading them while git holds the file. (Windows needs no extra
/// step: default ACLs on files created in %TEMP% are per-user.)
pub(crate) fn write_message_file(prefix: &str, message: &str) -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "mygitui-{prefix}-{}-{nanos}.msg",
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&path)
        .and_then(|mut file| file.write_all(message.as_bytes()))
        .map_err(|err| format!("failed to write {prefix} message file: {err}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::GitSignature;
    use std::sync::Arc;

    fn opts() -> CommitOptions {
        CommitOptions {
            message: "hello\n\nbody".to_string(),
            amend: false,
            no_verify: false,
            allow_empty: false,
            author: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn message_file_is_owner_only_and_round_trips() {
        let path = write_message_file("commit", "secret body\n").expect("write message file");
        let meta = std::fs::metadata(&path).expect("message file exists");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let content = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(content, "secret body\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn commit_args_message_only() {
        let args = build_commit_args("C:\\tmp\\m.msg", &opts());
        assert_eq!(args, vec!["commit", "-F", "C:\\tmp\\m.msg"]);
    }

    #[test]
    fn commit_args_all_flags() {
        let mut options = opts();
        options.amend = true;
        options.no_verify = true;
        options.allow_empty = true;
        let args = build_commit_args("/tmp/m.msg", &options);
        assert_eq!(
            args,
            vec![
                "commit",
                "-F",
                "/tmp/m.msg",
                "--amend",
                "--no-verify",
                "--allow-empty",
            ]
        );
    }

    #[test]
    fn commit_args_author_override() {
        let mut options = opts();
        options.author = Some(GitSignature {
            name: "Ada".to_string(),
            email: "ada@example.com".to_string(),
            time: 0,
            offset_minutes: 0,
        });
        let args = build_commit_args("/tmp/m.msg", &options);
        assert_eq!(
            args.last().map(String::as_str),
            Some("Ada <ada@example.com>")
        );
        assert!(args.contains(&"--author".to_string()));
    }

    #[test]
    fn push_args_default_branch_and_flags() {
        let options = PushOptions {
            remote: "origin".to_string(),
            branch: "main".to_string(),
            force: true,
            force_with_lease: false,
            set_upstream: true,
            ..PushOptions::default()
        };
        assert_eq!(
            build_push_args(&options),
            vec!["push", "--force", "--set-upstream", "origin", "main"]
        );
    }

    #[test]
    fn push_args_refs_tags_and_delete() {
        let mut options = PushOptions {
            remote: "origin".to_string(),
            refs: vec!["feature".to_string(), "v1".to_string()],
            force_with_lease: true,
            ..PushOptions::default()
        };
        assert_eq!(
            build_push_args(&options),
            vec!["push", "--force-with-lease", "origin", "feature", "v1"]
        );

        options.refs.clear();
        options.delete = true;
        options.tags = true;
        assert_eq!(
            build_push_args(&options),
            vec!["push", "--delete", "--tags", "--force-with-lease", "origin"]
        );
    }

    #[test]
    fn sanitize_env_keeps_git_and_allowlist_drops_rest() {
        let vars = vec![
            ("GIT_CONFIG_GLOBAL".to_string(), "/x/gitconfig".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("USERPROFILE".to_string(), r"C:\Users\u".to_string()),
            ("HOME".to_string(), "/home/u".to_string()),
            ("SECRET_LEAK".to_string(), "nope".to_string()),
            ("SYSTEMROOT".to_string(), r"C:\Windows".to_string()),
        ];
        let mut kept = sanitize_env(vars);
        kept.sort();
        assert_eq!(
            kept,
            vec![
                ("GIT_CONFIG_GLOBAL".to_string(), "/x/gitconfig".to_string()),
                ("HOME".to_string(), "/home/u".to_string()),
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("SYSTEMROOT".to_string(), r"C:\Windows".to_string()),
                ("USERPROFILE".to_string(), r"C:\Users\u".to_string()),
            ]
        );
    }

    /// Real-git integration: temp repo, commit via the fallback, verify the
    /// returned sha. Requires `git` on PATH — run with
    /// `cargo test -- --ignored`.
    #[test]
    #[ignore = "shells out to real git in a temp repo"]
    fn cli_commit_creates_commit_and_returns_sha() {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-cli-commit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git_init = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("spawn git");
            assert!(
                out.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git_init(&["init", "-q"]);
        git_init(&["config", "user.email", "test@example.com"]);
        git_init(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("file.txt"), b"one").unwrap();
        // The app flow stages via the engine before committing; the CLI
        // fallback commits the index as-is, so stage here too.
        git_init(&["add", "file.txt"]);

        let mut seen: Vec<String> = Vec::new();
        let sha = commit_via_cli(&dir, &opts(), &mut |line| seen.push(line.to_string()), None)
            .expect("cli commit succeeds");
        assert_eq!(sha.len(), 40, "sha from rev-parse HEAD: {sha}");
        assert!(
            seen.iter().any(|line| line.contains("[")),
            "git commit output was streamed: {seen:?}"
        );
        let show = Command::new("git")
            .args(["log", "-1", "--pretty=%B"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&show.stdout).starts_with("hello"));

        // Amend path.
        let mut amended = opts();
        amended.amend = true;
        amended.message = "amended".to_string();
        let sha2 = commit_via_cli(&dir, &amended, &mut |_line| {}, None).expect("amend succeeds");
        assert_eq!(sha2.len(), 40);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real-git integration: push a new branch through the CLI fallback. The
    /// ref report is written to stderr, which must still feed `updated_refs`
    /// and stream to the user. Requires `git` on PATH — run with
    /// `cargo test -- --ignored`.
    #[test]
    #[ignore = "shells out to real git in a temp repo"]
    fn cli_push_streams_output_and_reports_new_refs() {
        let root = std::env::temp_dir().join(format!(
            "mygitui-cli-push-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let remote = root.join("remote.git");
        let work = root.join("work");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&remote).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("spawn git");
            assert!(
                out.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&remote, &["init", "--bare", "-q"]);
        run(&work, &["init", "-q"]);
        run(&work, &["config", "user.email", "test@example.com"]);
        run(&work, &["config", "user.name", "Test"]);
        std::fs::write(work.join("file.txt"), b"one").unwrap();
        run(&work, &["add", "file.txt"]);
        run(&work, &["commit", "-q", "-m", "init"]);
        run(&work, &["branch", "-M", "main"]);
        let remote_url = remote.to_string_lossy().to_string();
        run(&work, &["remote", "add", "origin", &remote_url]);

        let mut seen: Vec<String> = Vec::new();
        let stats = push_via_cli(
            &work,
            &PushOptions {
                remote: "origin".to_string(),
                branch: "main".to_string(),
                set_upstream: true,
                ..PushOptions::default()
            },
            &mut |line| seen.push(line.to_string()),
            None,
        )
        .expect("cli push succeeds");
        assert_eq!(
            stats.updated_refs,
            vec![("main".to_string(), "(new)".to_string())],
            "stderr ref report was parsed"
        );
        assert!(
            seen.iter().any(|line| line.contains("[new branch]")),
            "git push output was streamed: {seen:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Real-git integration: a hung pre-commit hook is killed by op-cancel
    /// and the commit resolves with the shared "cancelled" error quickly
    /// instead of wedging the mutation lane for the hook's full (30 s)
    /// runtime. Requires `git` on PATH — run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "shells out to real git in a temp repo"]
    fn cli_commit_cancel_kills_a_hung_hook() {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-cli-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("spawn git");
            assert!(
                out.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("file.txt"), b"one").unwrap();
        run(&["add", "file.txt"]);
        // The hook the test cancels mid-run (git runs hooks through its
        // bundled sh on every platform, so a shebang script works even on
        // Windows; no exec bit is needed there, and git ignores mode bits
        // set via std::fs on Windows).
        std::fs::write(dir.join(".git/hooks/pre-commit"), "#!/bin/sh\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                dir.join(".git/hooks/pre-commit"),
                std::fs::Permissions::from_mode(0o755),
            );
        }

        // Simulate op_cancel arriving 300 ms into the (30 s) hook.
        let token = Arc::new(AtomicBool::new(false));
        let cancel_token = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            cancel_token.store(true, Ordering::Release);
        });

        let started = Instant::now();
        let err = commit_via_cli(&dir, &opts(), &mut |_| {}, Some(&token))
            .expect_err("cancelled commit must fail");
        assert_eq!(err, crate::ops::CANCELLED_ERR, "op_cancel error shape");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "cancel must not wait out the 30 s hook (took {:?})",
            started.elapsed()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
