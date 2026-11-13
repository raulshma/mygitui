//! One-time capture of the user's login-shell environment.
//!
//! A GUI-launched app does not source shell profiles, so its environment
//! misses what an interactive terminal would have — nvm's PATH,
//! SDKMAN_DIR, cargo's bin, … . User-facing spawns that are meant to
//! behave "like in a terminal" — custom shell actions
//! ([`crate::actions::run_action`]) and editor launches
//! ([`crate::editors`]) — overlay [`resolved()`] onto their environment.
//!
//! Resolution runs once: `$SHELL` as a login+interactive shell prints
//! `/usr/bin/env -0` between two markers (the markers slice the clean
//! region out of whatever else rc files print — prompts, motd, escape
//! codes). It is bounded by [`SHELL_ENV_TIMEOUT`], and any failure
//! (missing `$SHELL`, shells that reject `-i`, garbage in the region)
//! degrades to `None` — spawns then keep their inherited environment
//! unchanged, exactly as before this module existed. Windows resolves to
//! `None`: GUI processes already receive the user/machine environment from
//! the registry there, so there is nothing to fix.

use std::process::Command;
use std::sync::OnceLock;

/// A captured login-shell environment: `NAME=VALUE` pairs in shell order.
pub(crate) struct ShellEnv {
    vars: Vec<(String, String)>,
}

static RESOLVED: OnceLock<Option<ShellEnv>> = OnceLock::new();

/// The user's login-shell environment, resolved once on first use. `None`
/// when resolution failed (or is unsupported on this platform) — callers
/// keep their inherited environment. A warm-up thread started in
/// `lib.rs::run` pays the probe latency during app boot; the first spawn
/// after that reads the cache for free.
pub(crate) fn resolved() -> Option<&'static ShellEnv> {
    RESOLVED.get_or_init(resolve_once).as_ref()
}

/// Kick off resolution in the background. Called from a warm-up thread at
/// app startup so the first action/editor spawn does not pay the probe
/// latency.
pub(crate) fn warm() {
    let _ = resolved();
}

/// Overlay the resolved login-shell environment (see [`resolved`]) onto
/// `cmd`'s current environment; no-op when resolution failed.
///
/// Overlay, not replace: the probe shell is spawned from this process, so
/// the captured env is the inherited env plus profile mutations — keeping
/// the inherited set underneath makes anything the profiles did not touch
/// fall through unchanged. Spawns that sanitize first
/// ([`crate::cli::apply_sanitized_env`] `env_clear()`s) must call this
/// *after* the sanitize call.
pub(crate) fn apply_resolved(cmd: &mut Command) {
    apply_env(cmd, resolved());
}

fn apply_env(cmd: &mut Command, env: Option<&ShellEnv>) {
    if let Some(env) = env {
        for (key, value) in &env.vars {
            cmd.env(key, value);
        }
    }
}

#[cfg(unix)]
fn resolve_once() -> Option<ShellEnv> {
    let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) else {
        tracing::debug!("shell env: SHELL unset; keeping inherited env");
        return None;
    };
    let env = resolve_with_shell(&shell);
    match &env {
        Some(env) => tracing::debug!(
            count = env.vars.len(),
            "shell env: resolved login-shell environment"
        ),
        None => tracing::warn!(
            shell = %shell.to_string_lossy(),
            "shell env: login-shell probe failed; keeping inherited env"
        ),
    }
    env
}

#[cfg(not(unix))]
fn resolve_once() -> Option<ShellEnv> {
    None
}

/// Probe `shell` as a login+interactive shell and parse its environment.
/// Split from [`resolve_once`] so tests can drive it without touching the
/// process environment.
#[cfg(unix)]
fn resolve_with_shell(shell: &std::ffi::OsStr) -> Option<ShellEnv> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Duration;

    /// Heavy rc files (nvm, conda, p10k) can take a second or two; past
    /// this the shell is treated as hung and the run falls back.
    const SHELL_ENV_TIMEOUT: Duration = Duration::from_secs(5);

    // `printf %s` (not `echo`) so the markers carry no trailing newline and
    // the region between them is exactly the `env -0` NUL stream. `-l -i`
    // makes bash run the interactive-gated parts of the rc files — where
    // nvm & friends are typically initialized.
    const START_MARKER: &str = "MYGITUI_SHELL_ENV_START";
    const END_MARKER: &str = "MYGITUI_SHELL_ENV_END";
    let script = format!("printf %s {START_MARKER}; /usr/bin/env -0; printf %s {END_MARKER}");

    let mut cmd = crate::process::command(shell);
    cmd.args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            tracing::debug!(error = %err, "shell env: probe spawn failed");
            return None;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };

    // Reader thread → channel, the wait bounded by [`SHELL_ENV_TIMEOUT`]
    // (mirrors `opencode.rs::probe_version`): a hung shell must not stall
    // the caller.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let reader = std::thread::Builder::new()
        .name("shell-env-probe".to_owned())
        .spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    if reader.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }

    // Bounded by the timeout, never unbounded: either the shell finished
    // (EOF) or it is treated as hung. Either way: kill + reap — the shell
    // may have already exited, `kill` is a harmless no-op then.
    let output = rx.recv_timeout(SHELL_ENV_TIMEOUT).ok();
    let _ = child.kill();
    let _ = child.wait();
    let Some(output) = output else {
        tracing::debug!(
            timeout_secs = SHELL_ENV_TIMEOUT.as_secs(),
            "shell env: probe timed out"
        );
        return None;
    };
    parse_env_output(&output)
}

/// Pure: slice the marker region out of raw probe output and split the
/// NUL-separated `env -0` pairs. Rejects anything that does not read as a
/// complete environment (missing markers, junk inside the region, no PATH)
/// so rc-file noise can never yield a half env.
#[cfg(unix)]
fn parse_env_output(output: &[u8]) -> Option<ShellEnv> {
    const START_MARKER: &str = "MYGITUI_SHELL_ENV_START";
    const END_MARKER: &str = "MYGITUI_SHELL_ENV_END";

    let start = find_marker(output, START_MARKER.as_bytes())?;
    let after_start = start + START_MARKER.len();
    let end = find_marker(&output[after_start..], END_MARKER.as_bytes())? + after_start;

    let mut vars = Vec::new();
    for chunk in output[after_start..end].split(|byte| *byte == 0) {
        if chunk.is_empty() {
            continue; // `env -0` terminates the last pair with NUL
        }
        let Some(eq) = chunk.iter().position(|byte| *byte == b'=') else {
            tracing::debug!(
                junk = %String::from_utf8_lossy(chunk),
                "shell env: non k=v chunk inside probe output"
            );
            return None;
        };
        let key = String::from_utf8_lossy(&chunk[..eq]).into_owned();
        if key.is_empty() {
            return None;
        }
        // Values are expected UTF-8; lossy for the exotic-but-possible
        // non-UTF8 one.
        let value = String::from_utf8_lossy(&chunk[eq + 1..]).into_owned();
        vars.push((key, value));
    }
    if vars.is_empty() || !vars.iter().any(|(key, _)| key == "PATH") {
        tracing::debug!("shell env: probe output missing PATH");
        return None;
    }
    Some(ShellEnv { vars })
}

/// First occurrence of `marker` as a byte substring.
#[cfg(unix)]
fn find_marker(haystack: &[u8], marker: &[u8]) -> Option<usize> {
    haystack
        .windows(marker.len())
        .position(|window| window == marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(pairs: &[(&str, &str)]) -> ShellEnv {
        ShellEnv {
            vars: pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    /// Marker-delimited probe output as a well-behaved shell would print
    /// it: `noise <START> k=v\0… <END> noise`.
    #[cfg(unix)] // parse_env_output + its helpers only exist on unix
    fn probe_bytes(noise: bool, pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        if noise {
            out.extend_from_slice(b"p10k instant prompt\n\x1b[38;5;99m");
        }
        out.extend_from_slice(b"MYGITUI_SHELL_ENV_START");
        for (key, value) in pairs {
            out.extend_from_slice(format!("{key}={value}").as_bytes());
            out.push(0);
        }
        out.extend_from_slice(b"MYGITUI_SHELL_ENV_END");
        if noise {
            out.extend_from_slice(b"\nlogout message");
        }
        out
    }

    #[cfg(unix)] // parse_env_output only exists on unix
    #[test]
    fn parse_slices_marker_region_and_splits_nul_pairs() {
        let env = parse_env_output(&probe_bytes(
            true,
            &[("PATH", "/nvm/bin:/usr/bin"), ("FOO", "bar=baz")],
        ))
        .expect("well-formed probe output parses");
        assert_eq!(
            env.vars,
            vec![
                ("PATH".to_owned(), "/nvm/bin:/usr/bin".to_owned()),
                ("FOO".to_owned(), "bar=baz".to_owned()), // '=' inside a value survives
            ]
        );
    }

    #[cfg(unix)] // parse_env_output only exists on unix
    #[test]
    fn parse_rejects_missing_markers() {
        assert!(parse_env_output(b"PATH=/usr/bin").is_none());
        assert!(parse_env_output(b"MYGITUI_SHELL_ENV_STARTPATH=/usr/bin").is_none());
    }

    #[cfg(unix)] // parse_env_output only exists on unix
    #[test]
    fn parse_rejects_junk_inside_region() {
        // rc noise landing *between* the markers must not yield a half env.
        let mut bytes = probe_bytes(false, &[("PATH", "/usr/bin")]);
        // Splice a NUL-terminated junk chunk in front of the first pair.
        let mut spliced = b"MYGITUI_SHELL_ENV_START".to_vec();
        spliced.extend_from_slice(b"garbage without equals\0");
        spliced.extend_from_slice(&bytes[b"MYGITUI_SHELL_ENV_START".len()..]);
        bytes = spliced;
        assert!(parse_env_output(&bytes).is_none());
    }

    #[cfg(unix)] // parse_env_output only exists on unix
    #[test]
    fn parse_rejects_region_without_path() {
        let bytes = probe_bytes(false, &[("HOME", "/home/u")]);
        assert!(parse_env_output(&bytes).is_none());
    }

    #[test]
    fn apply_env_overlays_pairs_and_keeps_existing() {
        let mut cmd = Command::new("unused-binary");
        cmd.env("KEEP", "me");
        let env = env_with(&[("PATH", "/nvm/bin:/usr/bin"), ("NVM_DIR", "/n")]);
        apply_env(&mut cmd, Some(&env));
        let got: Vec<(String, String)> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        assert!(got.contains(&("KEEP".to_owned(), "me".to_owned())));
        assert!(got.contains(&("PATH".to_owned(), "/nvm/bin:/usr/bin".to_owned())));
        assert!(got.contains(&("NVM_DIR".to_owned(), "/n".to_owned())));
    }

    #[test]
    fn apply_env_none_is_noop() {
        let mut cmd = Command::new("unused-binary");
        cmd.env("KEEP", "me");
        apply_env(&mut cmd, None);
        let got: Vec<(String, String)> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        assert_eq!(got, vec![("KEEP".to_owned(), "me".to_owned())]);
    }

    #[cfg(unix)]
    #[test]
    fn unresolvable_shell_falls_back_fast() {
        // Deterministic on every CI machine: a missing binary must fail
        // immediately with None, not hang until the timeout.
        let started = std::time::Instant::now();
        let result = resolve_with_shell(std::ffi::OsStr::new("/nonexistent/mygitui-missing-shell"));
        assert!(result.is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "needs the real user's login shell + rc files; run manually: cargo test -- --ignored"]
    fn real_login_shell_resolves_with_path() {
        let shell = std::env::var_os("SHELL").expect("SHELL set for the manual probe");
        let env = resolve_with_shell(&shell).expect("login shell resolves to Some");
        let path = env
            .vars
            .iter()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.clone())
            .expect("PATH present");
        eprintln!("resolved login-shell PATH: {path}");
        assert!(!path.is_empty());
    }
}
