//! Managed opencode server lifecycle (AI: detect → enable → it just works).
//!
//! Detection resolves `opencode` through PATH (Windows: PATH × PATHEXT scan
//! in PATHEXT priority order, because `Command::new("opencode")` never finds
//! `.cmd`/`.bat` npm shims) and runs `opencode --version` for the installed
//! version. Misses are never cached: an opencode installed while mygitui
//! runs is found on the next detect.
//!
//! Managed serve spawns `opencode serve --hostname=127.0.0.1 --port=<free>`
//! as a child owned by [`OpenCodeServerRegistry`]: readiness is confirmed by
//! a TCP connect, the announced listening URL is parsed from the child's
//! stdout when available, and the registry's `Drop` kills every remaining
//! child on app exit (same contract as the pty registry). The frontend talks
//! to the server over HTTP as before; this module only owns the process
//! lifecycle.
//!
//! Child environments go through [`crate::cli::apply_sanitized_env`] (PATH
//! stays — opencode needs it to locate runtimes/shims; its config/auth under
//! HOME/APPDATA/XDG_CONFIG_HOME passes through the same allowlist).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use regex::Regex;
use serde::Serialize;

use crate::cli::apply_sanitized_env;

/// How long `opencode --version` may take before we declare it broken.
const VERSION_TIMEOUT: Duration = Duration::from_secs(4);
/// How long `serve_start` waits for the child to accept connections.
/// 30s (t3's server timeout): a cold npm-shim start on a slow disk can
/// easily exceed 10s.
const SERVE_READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Poll interval for both waits.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const STARTUP_OUTPUT_LINES: usize = 4;
const STARTUP_OUTPUT_LINE_CHARS: usize = 180;

/// Result of probing the machine for a local opencode CLI.
#[derive(Serialize, Clone, Debug)]
pub struct OpencodeDetection {
    pub installed: bool,
    /// Resolved binary path (first PATH hit), when installed.
    pub path: Option<String>,
    /// `x.y.z` parsed from `--version` output, when parseable.
    pub version: Option<String>,
    /// Major version (`1` for 1.x, `2` for 2.x), when parseable.
    pub major: Option<u32>,
}

impl OpencodeDetection {
    fn not_installed() -> Self {
        Self {
            installed: false,
            path: None,
            version: None,
            major: None,
        }
    }
}

/// Live state of the managed `opencode serve` child.
#[derive(Serialize, Clone, Debug, Default)]
pub struct OpencodeServeState {
    pub running: bool,
    /// Base URL of the managed server (`http://127.0.0.1:<port>`).
    pub url: Option<String>,
    pub port: Option<u16>,
    /// Failure description when not running (spawn error, child exit, …).
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Pure: candidate paths for `name` across a PATH-style string, ordered by
/// PATH entry, then (Windows) PATHEXT entry in declared priority order —
/// matching CreateProcess lookup semantics, so a real `.exe` beats a
/// `.cmd` npm shim in the same directory. Only existing files are returned,
/// so the first hit is spawnable.
pub(crate) fn scan_candidates(path_var: &str, pathext_var: &str, name: &str) -> Vec<PathBuf> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let extensions: Vec<String> = if cfg!(windows) {
        pathext_var
            .split(';')
            .map(|ext| ext.trim().trim_start_matches('.').to_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in path_var.split(separator) {
        let dir = Path::new(dir.trim());
        if dir.as_os_str().is_empty() {
            continue;
        }
        let mut candidates = Vec::new();
        // Windows: extensionless files are not executable; elsewhere the
        // bare name is the normal layout.
        if !cfg!(windows) {
            candidates.push(dir.join(name));
        }
        for ext in &extensions {
            candidates.push(dir.join(format!("{name}.{ext}")));
        }
        for candidate in candidates {
            if candidate.is_file() && !out.contains(&candidate) {
                out.push(candidate);
            }
        }
    }
    out
}

/// Extracts `x.y.z` + major from CLI output (`1.18.35`, `opencode v2.0.18`).
fn parse_version(text: &str) -> Option<(String, u32)> {
    let re = Regex::new(r"v?(\d+)\.(\d+)\.(\d+)").ok()?;
    let caps = re.captures(text)?;
    let major: u32 = caps.get(1)?.as_str().parse().ok()?;
    let matched = caps.get(0)?.as_str();
    Some((matched.trim_start_matches('v').to_string(), major))
}

/// Builds the spawn `Command` for a resolved binary. `.bat`/`.cmd` shims
/// need `cmd /C` on Windows (CreateProcess cannot execute them directly).
fn build_command(binary: &Path, args: &[&str]) -> Command {
    let needs_shell = cfg!(windows)
        && binary
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"));
    let mut command = if needs_shell {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(binary);
        cmd
    } else {
        Command::new(binary)
    };
    command.args(args);
    apply_sanitized_env(&mut command);
    command
}

/// Terminates a spawned child. Guarded by a liveness check first: a child
/// that already exited must NOT be killed again — its PID is freed and may
/// have been reused by Windows (e.g. by the very next spawn), and the
/// tree-kill would murder that unrelated process tree. When alive, the
/// child's PID is pinned by our open handle, so the Windows tree-kill is
/// safe — and necessary, because installed launchers (`~/.bun/bin`,
/// npm `.cmd` shims) spawn the real server as a grandchild that would
/// otherwise outlive the kill.
fn kill_child(child: &mut Child) {
    let alive = matches!(child.try_wait(), Ok(None));
    if !alive {
        let _ = child.wait();
        return;
    }
    #[cfg(windows)]
    {
        let pid = child.id();
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs `<binary> --version` (bounded); resolves `x.y.z` + major.
fn probe_version(binary: &Path) -> Option<(String, u32)> {
    let mut command = build_command(binary, &["--version"]);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    // Drain both streams independently or a full pipe can block the other.
    let (stdout_tx, stdout_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        let _ = stdout_tx.send(text);
    });
    let (stderr_tx, stderr_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        let _ = stderr_tx.send(text);
    });
    let deadline = Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) if Instant::now() >= deadline => break,
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
        }
    }
    // Whether it exited or we timed out: kill + reap so the reader hits EOF.
    kill_child(&mut child);
    let stdout = stdout_rx.recv_timeout(VERSION_TIMEOUT).unwrap_or_default();
    let stderr = stderr_rx.recv_timeout(VERSION_TIMEOUT).unwrap_or_default();
    let text = format!("{stdout}{stderr}");
    parse_version(&text)
}

/// Probes the machine for opencode: PATH × PATHEXT scan + `--version`.
pub(crate) fn detect_binary() -> OpencodeDetection {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let pathext_var = std::env::var("PATHEXT").unwrap_or_default();
    let Some(binary) = scan_candidates(&path_var, &pathext_var, "opencode")
        .into_iter()
        .next()
    else {
        return OpencodeDetection::not_installed();
    };
    let Some((version, major)) = probe_version(&binary) else {
        // Found on disk but broken (permissions, quarantine, …): report the
        // path so the UI can show something actionable.
        return OpencodeDetection {
            installed: true,
            path: Some(binary.to_string_lossy().into_owned()),
            version: None,
            major: None,
        };
    };
    OpencodeDetection {
        installed: true,
        path: Some(binary.to_string_lossy().into_owned()),
        version: Some(version),
        major: Some(major),
    }
}

// ---------------------------------------------------------------------------
// Managed serve lifecycle
// ---------------------------------------------------------------------------

/// One managed `opencode serve` child. The stdout reader thread fills `url`
/// once the server announces its listening address.
struct ServeEntry {
    child: Child,
    port: u16,
    url: Arc<Mutex<String>>,
}

/// Owns the managed server child; `Drop` kills it (app-exit backstop, same
/// contract as the pty registry).
pub struct OpenCodeServerRegistry {
    entry: Mutex<Option<ServeEntry>>,
    /// Serializes full startup attempts while status/stop remain responsive.
    starting: Mutex<()>,
}

impl Default for OpenCodeServerRegistry {
    fn default() -> Self {
        Self {
            entry: Mutex::new(None),
            starting: Mutex::new(()),
        }
    }
}

impl Drop for OpenCodeServerRegistry {
    fn drop(&mut self) {
        if let Some(mut entry) = self.entry.lock().take() {
            kill_child(&mut entry.child);
        }
    }
}

/// The `… listening on http://…` announcement in opencode's serve output.
fn parse_listening_url(line: &str) -> Option<String> {
    let re = Regex::new(r"(?i)listening on\s+(https?://\S+)").ok()?;
    Some(re.captures(line)?.get(1)?.as_str().to_string())
}

/// Extracts the port from OpenCode's announced URL.
fn base_url_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn record_startup_output(output: &Mutex<VecDeque<String>>, stream: &str, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let lower = line.to_ascii_lowercase();
    let detail = if [
        "password",
        "authorization",
        "api_key",
        "api-key",
        "token",
        "secret",
    ]
    .iter()
    .any(|term| lower.contains(term))
    {
        "[sensitive startup output redacted]".to_string()
    } else {
        line.chars().take(STARTUP_OUTPUT_LINE_CHARS).collect()
    };
    let mut output = output.lock();
    output.push_back(format!("{stream}: {detail}"));
    while output.len() > STARTUP_OUTPUT_LINES {
        output.pop_front();
    }
}

fn startup_error(message: String, output: &Mutex<VecDeque<String>>) -> String {
    let details = output
        .lock()
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");
    if details.is_empty() {
        message
    } else {
        format!("{message} — {details}")
    }
}

/// Strips the `\\?\` verbatim prefix Windows `canonicalize()` adds (dunce
/// style). OpenCode resolves its workspace with forward-slash-friendly
/// logic; a verbatim UNC cwd confuses its project detection.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{stripped}"));
    }
    if let Some(stripped) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(stripped.to_string());
    }
    path
}

/// Uses the requested project as OpenCode's workspace, or a private empty
/// directory when startup is triggered before any repository is active.
fn server_working_directory(cwd: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(cwd) = cwd {
        let resolved = cwd
            .canonicalize()
            .map_err(|err| format!("could not resolve OpenCode workspace: {err}"))?;
        if !resolved.is_dir() {
            return Err("OpenCode workspace is not a directory".into());
        }
        return Ok(strip_verbatim(resolved));
    }

    let fallback = std::env::temp_dir().join("mygitui-opencode");
    std::fs::create_dir_all(&fallback)
        .map_err(|err| format!("could not prepare OpenCode workspace: {err}"))?;
    fallback
        .canonicalize()
        .map(|resolved| strip_verbatim(resolved))
        .map_err(|err| format!("could not resolve OpenCode workspace: {err}"))
}

impl OpenCodeServerRegistry {
    /// Reaps the entry when the child already exited.
    fn reap_dead(live: &mut Option<ServeEntry>) {
        let dead = match live.as_mut() {
            Some(entry) => matches!(entry.child.try_wait(), Ok(Some(_)) | Err(_)),
            None => false,
        };
        if dead {
            if let Some(mut finished) = live.take() {
                let _ = finished.child.wait();
            }
        }
    }

    fn state_of(entry: &ServeEntry) -> OpencodeServeState {
        let announced = entry.url.lock().clone();
        OpencodeServeState {
            running: true,
            url: Some(if announced.is_empty() {
                base_url_for(entry.port)
            } else {
                announced
            }),
            port: Some(entry.port),
            error: None,
        }
    }

    /// Current state (reaping a child that exited since the last call).
    pub fn status(&self) -> OpencodeServeState {
        let mut guard = self.entry.lock();
        Self::reap_dead(&mut guard);
        match guard.as_ref() {
            Some(entry) => Self::state_of(entry),
            None => OpencodeServeState::default(),
        }
    }

    /// Starts the managed server (idempotent: returns the live one). Lets
    /// OpenCode choose a loopback port and waits until it accepts connects.
    pub fn start(&self, cwd: Option<&Path>) -> OpencodeServeState {
        // A settings refresh and a feature request can arrive together.
        // Coalesce those starts here too, instead of briefly running two
        // OpenCode servers and killing whichever loses the registry race.
        let _starting = self.starting.lock();
        {
            let mut guard = self.entry.lock();
            Self::reap_dead(&mut guard);
            if let Some(entry) = guard.as_ref() {
                return Self::state_of(entry);
            }
        }
        let detection = detect_binary();
        let Some(binary) = detection.path else {
            return OpencodeServeState {
                error: Some(
                    "opencode not found on PATH — install opencode or pick another backend in AI settings"
                        .into(),
                ),
                ..OpencodeServeState::default()
            };
        };

        let working_dir = match server_working_directory(cwd) {
            Ok(path) => path,
            Err(error) => {
                return OpencodeServeState {
                    error: Some(error),
                    ..OpencodeServeState::default()
                };
            }
        };

        let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) else {
            return OpencodeServeState {
                error: Some("no free local port available".into()),
                ..OpencodeServeState::default()
            };
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        drop(listener);
        let port_arg = format!("--port={port}");

        let mut command = build_command(
            Path::new(&binary),
            &["serve", "--hostname=127.0.0.1", &port_arg],
        );
        command.current_dir(working_dir);
        let mut child = match command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                return OpencodeServeState {
                    error: Some(format!("failed to start opencode: {err}")),
                    ..OpencodeServeState::default()
                };
            }
        };

        // Reader threads: parse the listening URL from stdout, drain stderr
        // (a full pipe would block the child).
        let announced: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let startup_output = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stdout) = child.stdout.take() {
            let sink = Arc::clone(&announced);
            let output = Arc::clone(&startup_output);
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Some(url) = parse_listening_url(&line) {
                        *sink.lock() = url;
                    }
                    record_startup_output(&output, "stdout", &line);
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let output = Arc::clone(&startup_output);
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    record_startup_output(&output, "stderr", &line);
                }
            });
        }

        // Keep the port in sync with OpenCode 1.x, which treats `--port=0`
        // as its default port rather than asking the OS for an ephemeral one.
        let deadline = Instant::now() + SERVE_READY_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    std::thread::sleep(POLL_INTERVAL);
                    return OpencodeServeState {
                        error: Some(startup_error(
                            format!("opencode serve exited immediately ({status})"),
                            &startup_output,
                        )),
                        ..OpencodeServeState::default()
                    };
                }
                Err(err) => {
                    kill_child(&mut child);
                    std::thread::sleep(POLL_INTERVAL);
                    return OpencodeServeState {
                        error: Some(startup_error(
                            format!("opencode serve wait failed: {err}"),
                            &startup_output,
                        )),
                        ..OpencodeServeState::default()
                    };
                }
                Ok(None) => {}
            }
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                let mut guard = self.entry.lock();
                // A racing start won the slot? Then tear our duplicate down.
                if guard.is_some() {
                    drop(guard);
                    kill_child(&mut child);
                } else {
                    *guard = Some(ServeEntry {
                        child,
                        port,
                        url: announced,
                    });
                    drop(guard);
                    return self.status();
                }
                return self.status();
            }
            if Instant::now() >= deadline {
                kill_child(&mut child);
                std::thread::sleep(POLL_INTERVAL);
                return OpencodeServeState {
                    error: Some(startup_error(
                        format!(
                            "opencode server did not become ready within {}s",
                            SERVE_READY_TIMEOUT.as_secs()
                        ),
                        &startup_output,
                    )),
                    ..OpencodeServeState::default()
                };
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Stops the managed server (no-op when none is running).
    pub fn stop(&self) -> OpencodeServeState {
        if let Some(mut entry) = self.entry.lock().take() {
            kill_child(&mut entry.child);
        }
        OpencodeServeState::default()
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Probes for a local opencode CLI (PATH scan + `--version`).
#[tauri::command(rename_all = "snake_case")]
pub fn opencode_detect() -> OpencodeDetection {
    detect_binary()
}

/// State of the managed opencode server (starts nothing).
#[tauri::command(rename_all = "snake_case")]
pub fn opencode_serve_status(
    registry: tauri::State<'_, OpenCodeServerRegistry>,
) -> OpencodeServeState {
    registry.status()
}

/// Starts (or returns the running) managed opencode server.
#[tauri::command(rename_all = "snake_case")]
pub fn opencode_serve_start(
    cwd: Option<String>,
    registry: tauri::State<'_, OpenCodeServerRegistry>,
) -> OpencodeServeState {
    registry.start(cwd.as_deref().map(Path::new))
}

/// Stops the managed opencode server.
#[tauri::command(rename_all = "snake_case")]
pub fn opencode_serve_stop(
    registry: tauri::State<'_, OpenCodeServerRegistry>,
) -> OpencodeServeState {
    registry.stop()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-opencode-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_version_handles_plain_prefixed_and_garbage() {
        assert_eq!(parse_version("1.18.35"), Some(("1.18.35".into(), 1)));
        assert_eq!(
            parse_version("opencode v2.0.18"),
            Some(("2.0.18".into(), 2))
        );
        assert_eq!(
            parse_version("version: 10.0.1 (build 7)"),
            Some(("10.0.1".into(), 10))
        );
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version("1.2 is not triple"), None);
    }

    #[test]
    fn parse_listening_url_matches_serve_announcement() {
        assert_eq!(
            parse_listening_url("opencode server listening on http://127.0.0.1:45617"),
            Some("http://127.0.0.1:45617".into())
        );
        assert_eq!(
            parse_listening_url("Server Listening on  https://0.0.0.0:4096/"),
            Some("https://0.0.0.0:4096/".into())
        );
        assert_eq!(parse_listening_url("nothing to see"), None);
    }

    #[cfg(windows)]
    #[test]
    fn strip_verbatim_removes_unc_prefixes() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\Code\repo")),
            PathBuf::from(r"C:\Code\repo")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\server\share\repo")),
            PathBuf::from(r"\\server\share\repo")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"C:\plain\path")),
            PathBuf::from(r"C:\plain\path")
        );
    }

    #[cfg(windows)]
    #[test]
    fn scan_candidates_orders_path_then_pathext() {
        let dir = unique_dir("scan");
        std::fs::write(dir.join("opencode.exe"), b"").unwrap();
        std::fs::write(dir.join("opencode.cmd"), b"").unwrap();
        let other = unique_dir("scan-other");
        std::fs::write(other.join("opencode.bat"), b"").unwrap();

        let path_var = format!("{};{}", dir.display(), other.display());
        let found = scan_candidates(&path_var, ".COM;.EXE;.BAT;.CMD", "opencode");
        assert_eq!(found.len(), 3, "all three shim flavors exist");
        assert_eq!(
            found[0],
            dir.join("opencode.exe"),
            "EXE precedes CMD within a PATH entry (PATHEXT priority order)"
        );
        assert_eq!(
            found[2],
            other.join("opencode.bat"),
            "second PATH entry last"
        );

        // A missing binary yields nothing (misses are never cached anywhere).
        assert!(scan_candidates(&other.display().to_string(), ".EXE", "opencode").is_empty());
    }

    #[cfg(not(windows))]
    #[test]
    fn scan_candidates_finds_bare_name_on_path() {
        let dir = unique_dir("scan-unix");
        std::fs::write(dir.join("opencode"), b"").unwrap();
        let found = scan_candidates(&dir.display().to_string(), "", "opencode");
        assert_eq!(found, vec![dir.join("opencode")]);
    }

    /// Real-CLI integration: resolves the installed opencode and reads its
    /// version. Requires opencode on PATH — run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "shells out to real opencode"]
    fn detect_finds_installed_cli() {
        let detection = detect_binary();
        assert!(detection.installed, "opencode expected on PATH");
        assert!(detection.version.is_some());
    }

    /// Real-CLI integration: managed serve lifecycle. Requires opencode on
    /// PATH — run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "shells out to real opencode"]
    fn serve_start_status_stop_roundtrip() {
        let registry = OpenCodeServerRegistry::default();
        let started = registry.start(None);
        assert!(started.running, "start failed: {:?}", started.error);
        let url = started.url.expect("running server has a URL");
        assert!(url.starts_with("http://127.0.0.1:"));

        let status = registry.status();
        assert!(status.running);
        assert_eq!(status.port, started.port);

        // The server actually speaks HTTP (health endpoint answers).
        let mut stream = TcpStream::connect(("127.0.0.1", status.port.unwrap())).unwrap();
        stream
            .write_all(
                b"GET /global/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        assert!(String::from_utf8_lossy(&response).contains("200"));

        let stopped = registry.stop();
        assert!(!stopped.running);
        assert!(registry.status().url.is_none());

        // Tree-kill regression: installed launchers (`~/.bun/bin` shims,
        // npm `.cmd`) spawn the real server as a grandchild — stop() must
        // take the whole tree down, leaving nothing on the port.
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            TcpStream::connect(("127.0.0.1", status.port.unwrap())).is_err(),
            "nothing may answer on {} after stop",
            status.port.unwrap()
        );
    }
}
