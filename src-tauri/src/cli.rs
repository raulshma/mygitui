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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::engine::types::CommitOptions;

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
    vars.into_iter()
        .filter(|(key, _)| key.starts_with("GIT_") || ENV_ALLOWLIST.contains(&key.as_str()))
        .collect()
}

/// Pure: is this env key on the pass-through list? (Shared with the
/// maintenance/rebase-exec runners.)
pub(crate) fn allowlisted(key: &str) -> bool {
    ENV_ALLOWLIST.contains(&key)
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

/// Run `git <args>` in `workdir` with the sanitized environment; success
/// returns trimmed stdout; failure returns stderr, falling back to stdout
/// (some failures, e.g. "nothing to commit", print there).
fn git(workdir: &Path, args: &[String]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(workdir).args(args);
    cmd.env_clear();
    for (key, value) in sanitize_env(std::env::vars()) {
        cmd.env(key, value);
    }
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

/// Commit via the git CLI (hooks + signing run for real). Returns the new
/// HEAD sha (`git rev-parse HEAD` after committing).
pub fn commit_via_cli(workdir: &Path, opts: &CommitOptions) -> Result<String, String> {
    let file = write_message_file(&opts.message)?;
    let result = (|| {
        let args = build_commit_args(&file.to_string_lossy(), opts);
        git(workdir, &args)?;
        git(workdir, &["rev-parse".to_string(), "HEAD".to_string()])
    })();
    let _ = std::fs::remove_file(&file);
    result
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

        let sha = commit_via_cli(&dir, &opts()).expect("cli commit succeeds");
        assert_eq!(sha.len(), 40, "sha from rev-parse HEAD: {sha}");
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
        let sha2 = commit_via_cli(&dir, &amended).expect("amend succeeds");
        assert_eq!(sha2.len(), 40);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
