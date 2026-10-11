//! Bridge to git's own credential helpers (`git credential fill/approve/reject`).
//!
//! libgit2 never invokes `credential.helper`, so without this module the app
//! cannot see credentials the user's git already has stored (Git Credential
//! Manager on Windows/macOS, libsecret, cache, …) and prompts for every
//! HTTPS remote that cmd git handles silently. The three operations mirror
//! what git itself does around a transport:
//!
//! * [`fill`] — look a credential up before an authentication attempt.
//!   Interactive helper prompts are suppressed (`-c
//!   credential.interactive=false`, `GCM_INTERACTIVE=never`,
//!   `GIT_TERMINAL_PROMPT=0`): when no helper holds a stored credential this
//!   returns `None` and the app's own AuthDialog stays the only interactive
//!   step.
//! * [`approve`] — store a credential the user just entered with "remember
//!   me", so cmd git and other consumers share it.
//! * [`reject`] — erase a stored credential the remote just refused, so a
//!   stale token cannot be served forever.
//!
//! Every call spawns the user's `git` through [`crate::process::command`]
//! (no console flash) with the sanitized environment
//! ([`crate::cli::apply_sanitized_env`]); `GCM_INTERACTIVE` is re-added
//! explicitly because it sits outside the `GIT_*`/allowlist set. Failures
//! (git missing, helper errors) degrade to "no credential": never fatal,
//! never logged with secret material.

use std::io::Write;
use std::process::Stdio;

use crate::cli;
use crate::process;

/// A credential described the way `git credential` wants it.
pub(crate) struct Query<'a> {
    pub protocol: &'a str,
    pub host: &'a str,
    pub path: Option<&'a str>,
    pub username: Option<&'a str>,
}

/// Ask the configured credential helpers for a stored credential.
/// `None` = nothing stored (or git/helper failed): the caller falls through
/// to the next link in its chain.
pub(crate) fn fill(query: &Query<'_>) -> Option<(String, String)> {
    let stdout = run_credential("fill", &attrs(query, None)).ok()?;
    match parse_fill(&stdout) {
        Some(pair) => Some(pair),
        None => {
            tracing::debug!(
                host = query.host,
                "credential helper returned no credential"
            );
            None
        }
    }
}

/// Store `password` for `query` in whatever helper the user configured.
/// Fire-and-forget: storage failures are logged, never surfaced — the
/// credential already works for this session.
pub(crate) fn approve(query: &Query<'_>, password: &str) {
    if let Err(err) = run_credential("approve", &attrs(query, Some(password))) {
        tracing::warn!(host = query.host, error = %err, "git credential approve failed");
    }
}

/// Erase the stored credential for `query` after the remote refused it (what
/// `git` itself does on a rejected credential). Fire-and-forget.
pub(crate) fn reject(query: &Query<'_>) {
    if let Err(err) = run_credential("reject", &attrs(query, None)) {
        tracing::warn!(host = query.host, error = %err, "git credential reject failed");
    }
}

/// Spawn `git credential <action>`, feed `attrs` on stdin (blank-line
/// terminated), return stdout. `Err` on spawn/wait failure.
fn run_credential(action: &str, attrs: &[String]) -> Result<String, String> {
    let mut cmd = process::command("git");
    // Non-interactive: `credential.interactive` is honored by core git
    // (2.42+) and GCM alike; the env vars cover older GCM releases and the
    // terminal/askpass fallback. Unknown `-c` keys are accepted silently.
    cmd.arg("-c").arg("credential.interactive=false");
    cmd.args(["credential", action]);
    cli::apply_sanitized_env(&mut cmd);
    cmd.env("GCM_INTERACTIVE", "never");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("spawn git credential {action}: {err}"))?;
    // The input is a few short lines, far below the pipe buffer, so writing
    // before reading cannot deadlock. Dropping stdin signals EOF.
    if let Some(mut stdin) = child.stdin.take() {
        for attr in attrs {
            let _ = writeln!(stdin, "{attr}");
        }
        let _ = writeln!(stdin);
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("wait git credential {action}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "git credential {action} exited with {}",
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// stdin attribute lines for `query`; `password` only for `approve`.
fn attrs(query: &Query<'_>, password: Option<&str>) -> Vec<String> {
    let mut out = vec![
        format!("protocol={}", query.protocol),
        format!("host={}", query.host),
    ];
    if let Some(path) = query.path.filter(|p| !p.is_empty()) {
        out.push(format!("path={path}"));
    }
    if let Some(user) = query.username.filter(|u| !u.is_empty()) {
        out.push(format!("username={user}"));
    }
    if let Some(pass) = password {
        out.push(format!("password={pass}"));
    }
    out
}

/// Parse `git credential fill` output into `(username, password)`; `None`
/// unless both attributes are present and non-empty (a helper with nothing
/// stored prints the echoed attrs only). Continuation lines (leading space)
/// and other attributes are ignored.
fn parse_fill(stdout: &str) -> Option<(String, String)> {
    let mut user = None;
    let mut pass = None;
    for line in stdout.lines() {
        if line.starts_with(' ') {
            continue; // secret continuation
        }
        if let Some(v) = line.strip_prefix("username=") {
            user = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("password=") {
            pass = Some(v.to_string());
        }
    }
    match (user, pass) {
        (Some(u), Some(p)) if !u.is_empty() && !p.is_empty() => Some((u, p)),
        _ => None,
    }
}

/// Split a remote URL into `(protocol, host, path)` the way `git credential`
/// wants them. Handles scheme URLs (userinfo/ports stripped, path without
/// the leading slash, query/fragment dropped) and scp-like `git@host:path`.
pub(crate) fn split_url(url: &str) -> (String, String, Option<String>) {
    if let Some((scheme, rest)) = url.split_once("://") {
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let after = &rest[authority_end..];
        let hostport = authority.rsplit('@').next().unwrap_or(authority);
        let host = strip_port(hostport);
        let path = after.split(['?', '#']).next().unwrap_or("");
        let path = path.trim_start_matches('/');
        let path = (!path.is_empty()).then(|| path.to_string());
        (scheme.to_ascii_lowercase(), host, path)
    } else {
        let userhost = url.split([':', '/']).next().unwrap_or(url);
        let host = userhost.rsplit('@').next().unwrap_or(userhost);
        (String::from("ssh"), host.to_string(), None)
    }
}

fn strip_port(hostport: &str) -> String {
    match hostport.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            host.to_string()
        }
        _ => hostport.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fill_reads_username_and_password() {
        let out = concat!(
            "protocol=https\n",
            "host=github.com\n",
            "username=ada\n",
            "password=t0ken\n",
        );
        assert_eq!(
            parse_fill(out),
            Some(("ada".to_string(), "t0ken".to_string()))
        );
    }

    #[test]
    fn parse_fill_missing_either_attribute_is_a_miss() {
        assert_eq!(parse_fill("protocol=https\nhost=x\n"), None);
        assert_eq!(
            parse_fill("protocol=https\nhost=x\nusername=ada\n"),
            None,
            "password missing"
        );
        assert_eq!(
            parse_fill("protocol=https\nhost=x\npassword=t0ken\n"),
            None,
            "username missing"
        );
        assert_eq!(
            parse_fill("username=\npassword=\n"),
            None,
            "empty values are not a credential"
        );
    }

    #[test]
    fn parse_fill_ignores_continuations_and_extra_attrs() {
        let out = concat!(
            "protocol=https\n",
            "wwwauth[]=Basic realm=\"x\"\n",
            "username=ada\n",
            "password=\n",
            " t0ken-continued\n",
        );
        // The continuation line is skipped, so the password looks empty.
        assert_eq!(parse_fill(out), None);
    }

    #[test]
    fn attrs_includes_only_present_fields() {
        let q = Query {
            protocol: "https",
            host: "github.com",
            path: Some("o/r.git"),
            username: Some("ada"),
        };
        assert_eq!(
            attrs(&q, None),
            vec![
                "protocol=https".to_string(),
                "host=github.com".to_string(),
                "path=o/r.git".to_string(),
                "username=ada".to_string(),
            ]
        );
        let minimal = Query {
            protocol: "https",
            host: "example.com",
            path: None,
            username: None,
        };
        assert_eq!(
            attrs(&minimal, Some("pw")),
            vec![
                "protocol=https".to_string(),
                "host=example.com".to_string(),
                "password=pw".to_string(),
            ]
        );
    }

    #[test]
    fn split_url_handles_scheme_urls() {
        assert_eq!(
            split_url("https://github.com/o/r.git"),
            (
                "https".to_string(),
                "github.com".to_string(),
                Some("o/r.git".to_string())
            )
        );
        assert_eq!(
            split_url("https://user:token@example.com:8443/r.git"),
            (
                "https".to_string(),
                "example.com".to_string(),
                Some("r.git".to_string())
            )
        );
        assert_eq!(
            split_url("https://example.com"),
            ("https".to_string(), "example.com".to_string(), None)
        );
        assert_eq!(
            split_url("https://example.com/a?x=1#f"),
            (
                "https".to_string(),
                "example.com".to_string(),
                Some("a".to_string())
            )
        );
    }

    #[test]
    fn split_url_handles_scp_like() {
        assert_eq!(
            split_url("git@github.com:owner/repo.git"),
            ("ssh".to_string(), "github.com".to_string(), None)
        );
    }

    /// The scp-like branch and the empty input: the host half doubles as
    /// the auth-broker cache key (`auth::host_of` used to own this logic).
    #[test]
    fn split_url_host_edge_cases() {
        assert_eq!(split_url("").2, None);
        assert_eq!(split_url("").1, "");
        assert_eq!(
            split_url("ssh://git@ssh.dev.azure.com/v3/x/y").1,
            "ssh.dev.azure.com"
        );
    }
}
