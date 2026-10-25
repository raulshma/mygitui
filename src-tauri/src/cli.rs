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

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::engine::types::{CommitOptions, NetStats, PushOptions};

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
/// (some failures, e.g. "nothing to commit", print there).
fn git(workdir: &Path, args: &[String]) -> Result<String, String> {
    let mut cmd = crate::process::command("git");
    cmd.current_dir(workdir).args(args);
    apply_sanitized_env(&mut cmd);
    let output = cmd
        .output()
        .map_err(|err| format!("failed to spawn git: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        let head = args.first().map(String::as_str).unwrap_or_default();
        Err(format!("git {head} failed: {detail}"))
    }
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
fn git_stream(
    workdir: &Path,
    args: &[String],
    on_line: &mut dyn FnMut(&str),
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

    let status = child
        .wait()
        .map_err(|err| format!("failed to wait for git: {err}"))?;
    let _ = out_reader.join();
    let _ = err_reader.join();

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
/// is handed to `on_line` while the command runs.
pub fn commit_via_cli(
    workdir: &Path,
    opts: &CommitOptions,
    on_line: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let file = write_message_file(&opts.message)?;
    let result = (|| {
        let args = build_commit_args(&file.to_string_lossy(), opts);
        git_stream(workdir, &args, on_line)?;
        git(workdir, &["rev-parse".to_string(), "HEAD".to_string()])
    })();
    let _ = std::fs::remove_file(&file);
    result
}

/// Push via the git CLI so a `pre-push` hook really runs (and streams its
/// output). Returns the refs reported by `git push`; the report is written to
/// stderr alongside progress, so parse both streams.
pub fn push_via_cli(
    workdir: &Path,
    opts: &PushOptions,
    on_line: &mut dyn FnMut(&str),
) -> Result<NetStats, String> {
    let args = build_push_args(opts);
    let output = git_stream(workdir, &args, on_line)?;
    let report = format!("{}\n{}", output.stdout, output.stderr);
    let updated_refs = crate::engine::netops::parse_push_report(&report);
    Ok(NetStats {
        received_bytes: 0,
        objects: updated_refs.len() as u32,
        updated_refs,
    })
}

/// Unique temp file for the commit message (removed by the caller).
fn write_message_file(message: &str) -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path =
        std::env::temp_dir().join(format!("mygitui-commit-{}-{nanos}.msg", std::process::id()));
    std::fs::write(&path, message)
        .map_err(|err| format!("failed to write commit message file: {err}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::GitSignature;

    fn opts() -> CommitOptions {
        CommitOptions {
            message: "hello\n\nbody".to_string(),
            amend: false,
            no_verify: false,
            allow_empty: false,
            author: None,
        }
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
        let sha = commit_via_cli(&dir, &opts(), &mut |line| seen.push(line.to_string()))
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
        let sha2 = commit_via_cli(&dir, &amended, &mut |_line| {}).expect("amend succeeds");
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
}
