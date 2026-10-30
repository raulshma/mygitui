//! SSH session pool over russh: one multiplexed [`DeviceSession`] per
//! device, shared by every repo/panel working against that device.
//!
//! Threading model: the backend engines are synchronous (they run on
//! blocking threads under the per-repo op queue, exactly like the git2
//! engine) and bridge into the async russh world with
//! `tauri::async_runtime::block_on`.
//!
//! Concurrency: a russh `Handle` is not `Clone`, so the session's handle
//! lives behind an async mutex — but the lock is held only for the
//! milliseconds it takes to OPEN a channel. Channels are independent once
//! opened, so a long `git log` never blocks the terminal or an SFTP read on
//! the same device.
//!
//! Reconnect: operations that fail to open a channel drop the cached handle
//! and re-dial once (full auth + host-key flow — a trusted host never
//! re-prompts). Auth secrets live in the OS keyring; a missing secret
//! prompts the FE through [`prompts`], with an optional "remember" that
//! writes back to the keyring.

use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::PrivateKeyWithHashAlg;
use russh::{Channel, ChannelMsg, Disconnect};
use serde::Serialize;
use tokio::sync::{watch, Mutex};
use ts_rs::TS;

use super::hostkeys::{HostKeyStatus, HostKeyStore};
use super::profiles::{get_secret, set_secret, AuthKind, DeviceAuth, DeviceProfile};
use super::prompts::{
    self, describe_host_key, DeviceAuthRequestEvent, HostKeyRequestEvent, KIND_DEVICE_PASSWORD,
    KIND_SSH_PASSPHRASE,
};

/// Hard cap on a single dial (TCP + KEX + auth).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default cap for one remote command (git status/diff/log scale).
pub const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(120);
/// Cap for network ops (fetch/pull/push) and big archives.
pub const LONG_EXEC_TIMEOUT: Duration = Duration::from_secs(900);
/// SSH-level keepalive probe interval.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
/// Missed keepalives before the session is considered dead.
const KEEPALIVE_MAX: usize = 4;
/// Hard cap on buffered exec output (archive-scale responses); a command
/// producing more fails instead of exhausting host memory.
pub const MAX_EXEC_BYTES: usize = 256 * 1024 * 1024;

/// Error string for a caller-aborted `exec_stream`: the caller returned
/// `false` from `on_line`, so the channel is dropped without draining.
const EXEC_CANCELLED: &str = "remote command cancelled";

/// Connection state of one device, mirrored to the FE as `device-status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    Disconnected,
    Connecting,
    Connected,
}

impl DeviceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceStatus::Disconnected => "disconnected",
            DeviceStatus::Connecting => "connecting",
            DeviceStatus::Connected => "connected",
        }
    }
}

/// `device-status` event payload.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DeviceStatusEvent {
    pub device_id: String,
    /// "disconnected" | "connecting" | "connected".
    #[ts(type = "\"disconnected\" | \"connecting\" | \"connected\"")]
    pub status: String,
}

/// Completed remote command execution.
#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub exit_code: u32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// Turn a non-zero exit into an `Err` carrying the remote stderr tail
    /// (git's actual error message lives there, not in stdout).
    pub fn checked(self, what: &str) -> Result<ExecOutput, String> {
        if self.exit_code == 0 {
            Ok(self)
        } else {
            let mut tail = self.stderr_str();
            tail = tail.trim().to_string();
            if tail.len() > 400 {
                tail = format!("…{}", &tail[tail.len() - 400..]);
            }
            Err(format!("{what} failed (exit {}): {tail}", self.exit_code))
        }
    }
}

/// Per-connection russh handler: host-key verification (TOFU) + nothing
/// else — every channel is opened client-side.
struct ClientHandler {
    device_id: String,
    host: String,
    port: u16,
    host_key_store: Arc<HostKeyStore>,
}

impl ClientHandler {
    fn host_port(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let (algo, fingerprint) = describe_host_key(server_public_key);
        let host_port = self.host_port();
        let status = self.host_key_store.status(&host_port, &algo, &fingerprint);
        match status {
            HostKeyStatus::Trusted => Ok(true),
            HostKeyStatus::Unknown | HostKeyStatus::Changed => {
                let pinned = self.host_key_store.known(&host_port);
                let status_str = match status {
                    HostKeyStatus::Unknown => "unknown",
                    _ => "changed",
                };
                let event = HostKeyRequestEvent {
                    request_id: format!("hk-{}", uuid::Uuid::new_v4()),
                    device_id: self.device_id.clone(),
                    host: self.host.clone(),
                    port: self.port,
                    algo: algo.clone(),
                    fingerprint: fingerprint.clone(),
                    status: status_str.to_string(),
                    pinned_fingerprint: pinned.as_ref().map(|k| k.fingerprint.clone()),
                };
                let accepted = prompts::ask_host_key(event).await.unwrap_or(false);
                if accepted {
                    // Persist on accept (first trust AND explicit re-pin).
                    let _ = self.host_key_store.trust(&host_port, &algo, &fingerprint);
                }
                Ok(accepted)
            }
        }
    }
}

/// One device's live SSH session (or its absence).
pub struct DeviceSession {
    pub device_id: String,
    pub device_name: String,
    host: String,
    port: u16,
    username: String,
    auth: DeviceAuth,
    host_key_store: Arc<HostKeyStore>,
    /// Not-Clone russh handle behind an async mutex; the lock guards only
    /// dial + channel open (see module docs).
    handle: Mutex<Option<Handle<ClientHandler>>>,
    status: watch::Sender<DeviceStatus>,
}

impl DeviceSession {
    fn new(profile: &DeviceProfile, host_key_store: Arc<HostKeyStore>) -> Arc<Self> {
        let (status, _rx) = watch::channel(DeviceStatus::Disconnected);
        Arc::new(Self {
            device_id: profile.id.clone(),
            device_name: profile.name.clone(),
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            auth: profile.auth.clone(),
            host_key_store,
            handle: Mutex::new(None),
            status,
        })
    }

    pub fn status(&self) -> DeviceStatus {
        *self.status.borrow()
    }

    fn set_status(&self, status: DeviceStatus) {
        if *self.status.borrow() != status {
            let _ = self.status.send_replace(status);
            prompts::emit_status(&DeviceStatusEvent {
                device_id: self.device_id.clone(),
                status: status.as_str().to_string(),
            });
        }
    }

    /// Dial + authenticate. Returns the fresh (unstored) handle.
    async fn establish(&self) -> Result<Handle<ClientHandler>, String> {
        self.set_status(DeviceStatus::Connecting);
        let result = self.establish_inner().await;
        match &result {
            Ok(_) => self.set_status(DeviceStatus::Connected),
            Err(_) => self.set_status(DeviceStatus::Disconnected),
        }
        result
    }

    async fn establish_inner(&self) -> Result<Handle<ClientHandler>, String> {
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(60 * 30)),
            keepalive_interval: Some(KEEPALIVE_INTERVAL),
            keepalive_max: KEEPALIVE_MAX,
            ..client::Config::default()
        });
        let addr = format!("{}:{}", self.host, self.port);
        let handler = ClientHandler {
            device_id: self.device_id.clone(),
            host: self.host.clone(),
            port: self.port,
            host_key_store: self.host_key_store.clone(),
        };
        let mut handle = tokio::time::timeout(
            CONNECT_TIMEOUT,
            client::connect(config, addr.as_str(), handler),
        )
        .await
        .map_err(|_| format!("connect to {addr} timed out"))?
        .map_err(|e| format!("connect to {addr} failed: {e}"))?;
        self.authenticate(&mut handle).await?;
        Ok(handle)
    }

    async fn authenticate(&self, handle: &mut Handle<ClientHandler>) -> Result<(), String> {
        match self.auth.kind {
            AuthKind::KeyFile => {
                let path = self
                    .auth
                    .key_path
                    .clone()
                    .filter(|p| !p.is_empty())
                    .ok_or_else(|| "key file path is not set".to_string())?;
                let mut passphrase = get_secret(&self.device_id, "passphrase")?;
                if passphrase.is_none() {
                    let answer = prompts::ask_secret(DeviceAuthRequestEvent {
                        request_id: format!("ak-{}", uuid::Uuid::new_v4()),
                        device_id: self.device_id.clone(),
                        kind: KIND_SSH_PASSPHRASE.to_string(),
                        prompt: format!("Passphrase for key {path}"),
                    })
                    .await?;
                    match answer {
                        Some(answer) => {
                            if answer.store {
                                let _ = set_secret(&self.device_id, "passphrase", &answer.password);
                            }
                            passphrase = Some(answer.password);
                        }
                        None => return Err("key passphrase prompt was cancelled".to_string()),
                    }
                }
                let key_path = path.clone();
                let key = {
                    let pass = passphrase.take();
                    tokio::task::spawn_blocking(move || {
                        russh::keys::load_secret_key(&key_path, pass.as_deref())
                    })
                    .await
                    .map_err(|e| format!("key load task failed: {e}"))?
                    .map_err(|e| format!("failed to load key {path}: {e}"))?
                };
                let result = handle
                    .authenticate_publickey(
                        &self.username,
                        PrivateKeyWithHashAlg::new(Arc::new(key), None),
                    )
                    .await
                    .map_err(|e| format!("public key auth failed: {e}"))?;
                if !result.success() {
                    return Err("public key authentication was rejected".to_string());
                }
            }
            AuthKind::Password => {
                let mut password = get_secret(&self.device_id, "password")?;
                if password.is_none() {
                    let answer = prompts::ask_secret(DeviceAuthRequestEvent {
                        request_id: format!("ak-{}", uuid::Uuid::new_v4()),
                        device_id: self.device_id.clone(),
                        kind: KIND_DEVICE_PASSWORD.to_string(),
                        prompt: format!("Password for {}@{}", self.username, self.host),
                    })
                    .await?;
                    match answer {
                        Some(answer) => {
                            if answer.store {
                                let _ = set_secret(&self.device_id, "password", &answer.password);
                            }
                            password = Some(answer.password);
                        }
                        None => return Err("password prompt was cancelled".to_string()),
                    }
                }
                let result = handle
                    .authenticate_password(&self.username, &password.unwrap_or_default())
                    .await
                    .map_err(|e| format!("password auth failed: {e}"))?;
                if !result.success() {
                    return Err("password authentication was rejected".to_string());
                }
            }
        }
        Ok(())
    }

    /// Connect when not already connected. No-op when live.
    pub async fn ensure_connected(&self) -> Result<(), String> {
        let mut guard = self.handle.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let handle = self.establish().await?;
        *guard = Some(handle);
        Ok(())
    }

    /// Open a fresh session channel, reconnecting transparently when the
    /// cached handle is dead. The session lock is held only while opening.
    async fn open_session_channel(&self) -> Result<Channel<russh::client::Msg>, String> {
        // Fast path: live handle, channel opens (locks for milliseconds).
        {
            let guard = self.handle.lock().await;
            if let Some(handle) = guard.as_ref() {
                match handle.channel_open_session().await {
                    Ok(channel) => return Ok(channel),
                    Err(err) => {
                        tracing::warn!(
                            device = %self.device_id,
                            error = %err,
                            "channel open failed; reconnecting"
                        );
                    }
                }
            }
        }
        // Reconnect path: exclusive. Double-check — another task may have
        // re-dialed while we waited for the lock.
        let mut guard = self.handle.lock().await;
        if let Some(handle) = guard.as_ref() {
            if let Ok(channel) = handle.channel_open_session().await {
                return Ok(channel);
            }
        }
        *guard = None;
        let handle = self.establish().await?;
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| format!("channel open failed after reconnect: {e}"))?;
        *guard = Some(handle);
        Ok(channel)
    }

    /// Close the session (idempotent).
    pub async fn disconnect(&self) {
        if let Some(handle) = self.handle.lock().await.take() {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "", "english")
                .await;
        }
        self.set_status(DeviceStatus::Disconnected);
    }

    /// Drive one exec channel to completion, optionally feeding stdin
    /// (`git commit -F -`, `git apply -`).
    async fn drive_exec(
        channel: &mut Channel<russh::client::Msg>,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<ExecOutput, String> {
        channel
            .exec(true, command.as_bytes().to_vec())
            .await
            .map_err(|e| format!("exec failed: {e}"))?;
        if let Some(input) = stdin {
            channel
                .data(&input[..])
                .await
                .map_err(|e| format!("stdin write failed: {e}"))?;
            channel
                .eof()
                .await
                .map_err(|e| format!("eof signal failed: {e}"))?;
        }
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code: Option<u32> = None;
        loop {
            match tokio::time::timeout(timeout, channel.wait()).await {
                Ok(None) => break, // channel closed by the server
                Ok(Some(ChannelMsg::Data { data })) => {
                    if stdout.len() + data.len() > MAX_EXEC_BYTES {
                        return Err("remote command output exceeded 256 MiB".to_string());
                    }
                    stdout.extend_from_slice(&data);
                }
                Ok(Some(ChannelMsg::ExtendedData { data, .. })) => {
                    if stderr.len() + data.len() > MAX_EXEC_BYTES {
                        return Err("remote command output exceeded 256 MiB".to_string());
                    }
                    stderr.extend_from_slice(&data);
                }
                Ok(Some(ChannelMsg::ExitStatus { exit_status })) => exit_code = Some(exit_status),
                Ok(Some(ChannelMsg::Eof)) => {}
                Ok(Some(ChannelMsg::Close)) => break,
                Ok(Some(_)) => {}
                Err(_) => return Err("remote command timed out".to_string()),
            }
        }
        Ok(ExecOutput {
            exit_code: exit_code.unwrap_or(0),
            stdout,
            stderr,
        })
    }

    /// Run one remote command. Transport-level failures while OPENING the
    /// channel reconnect transparently (a trusted host re-dials silently);
    /// a failure AFTER the command was sent is returned as-is.
    pub async fn exec(&self, command: &str, timeout: Duration) -> Result<ExecOutput, String> {
        self.exec_with_stdin(command, None, timeout).await
    }

    /// Run one remote command with optional stdin payload.
    ///
    /// No automatic re-run: once `exec` has been sent, a timeout or a
    /// mid-flight channel failure says nothing about whether the command
    /// completed on the device, and transparently re-running a
    /// non-idempotent one (`commit`, `stash drop`, `push`) would execute it
    /// twice. Only channel-OPEN failures retry (inside
    /// [`Self::open_session_channel`], which runs before any command is
    /// sent).
    pub async fn exec_with_stdin(
        &self,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<ExecOutput, String> {
        let mut channel = self.open_session_channel().await?;
        Self::drive_exec(&mut channel, command, stdin, timeout).await
    }

    /// Run one remote command, streaming complete stdout/stderr LINES to
    /// `on_line` as they arrive (progress bars, live op logs). `on_line`
    /// returns `false` to abort: the channel is dropped (the remote process
    /// gets the connection-closed SIGHUP). Like [`Self::exec`], failures
    /// after the command was sent are NOT retried — a partially-seen
    /// `git push` may have completed on the device.
    pub async fn exec_stream(
        &self,
        command: &str,
        timeout: Duration,
        mut on_line: impl FnMut(bool, &str) -> bool + Send,
    ) -> Result<ExecOutput, String> {
        let mut channel = self.open_session_channel().await?;
        Self::drive_stream(&mut channel, command, timeout, &mut on_line).await
    }

    async fn drive_stream(
        channel: &mut Channel<russh::client::Msg>,
        command: &str,
        timeout: Duration,
        on_line: &mut impl FnMut(bool, &str) -> bool,
    ) -> Result<ExecOutput, String> {
        channel
            .exec(true, command.as_bytes().to_vec())
            .await
            .map_err(|e| format!("exec failed: {e}"))?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut out_buf: Vec<u8> = Vec::new();
        let mut err_buf: Vec<u8> = Vec::new();
        let mut exit_code: Option<u32> = None;
        let mut feed =
            |buf: &mut Vec<u8>, data: &[u8], is_stderr: bool, total: &mut Vec<u8>| -> bool {
                if total.len() + data.len() > MAX_EXEC_BYTES {
                    return true; // capped: drop further bytes, keep draining
                }
                total.extend_from_slice(data);
                buf.extend_from_slice(data);
                while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&line[..line.len() - 1]);
                    if !on_line(is_stderr, line.trim_end_matches('\r')) {
                        return false;
                    }
                }
                true
            };
        loop {
            match tokio::time::timeout(timeout, channel.wait()).await {
                Ok(None) => break,
                Ok(Some(ChannelMsg::Data { data })) => {
                    if !feed(&mut out_buf, &data, false, &mut stdout) {
                        return Err(EXEC_CANCELLED.to_string());
                    }
                }
                Ok(Some(ChannelMsg::ExtendedData { data, .. })) => {
                    if !feed(&mut err_buf, &data, true, &mut stderr) {
                        return Err(EXEC_CANCELLED.to_string());
                    }
                }
                Ok(Some(ChannelMsg::ExitStatus { exit_status })) => exit_code = Some(exit_status),
                Ok(Some(ChannelMsg::Eof)) => {}
                Ok(Some(ChannelMsg::Close)) => break,
                Ok(Some(_)) => {}
                Err(_) => return Err("remote command timed out".to_string()),
            }
        }
        // Flush trailing partial lines.
        if !out_buf.is_empty() {
            let line = String::from_utf8_lossy(&out_buf).into_owned();
            if !on_line(false, &line) {
                return Err(EXEC_CANCELLED.to_string());
            }
        }
        if !err_buf.is_empty() {
            let line = String::from_utf8_lossy(&err_buf).into_owned();
            if !on_line(true, &line) {
                return Err(EXEC_CANCELLED.to_string());
            }
        }
        Ok(ExecOutput {
            exit_code: exit_code.unwrap_or(0),
            stdout,
            stderr,
        })
    }

    /// Open an SFTP session over the connection (reconnecting as needed).
    pub async fn sftp(&self) -> Result<russh_sftp::client::SftpSession, String> {
        let open = || async {
            let channel = self.open_session_channel().await?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(|e| format!("sftp subsystem request failed: {e}"))?;
            russh_sftp::client::SftpSession::new(channel.into_stream())
                .await
                .map_err(|e| format!("sftp init failed: {e}"))
        };
        match open().await {
            Ok(sftp) => Ok(sftp),
            Err(first) => {
                tracing::warn!(
                    device = %self.device_id,
                    error = %first,
                    "sftp open failed; retrying once"
                );
                open().await
            }
        }
    }

    /// Open an interactive shell channel (terminal panel). The channel is
    /// owned by the caller; dropping it closes the remote shell.
    pub async fn shell(&self, rows: u32, cols: u32) -> Result<Channel<russh::client::Msg>, String> {
        let channel = self.open_session_channel().await?;
        channel
            .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
            .await
            .map_err(|e| format!("pty request failed: {e}"))?;
        channel
            .request_shell(true)
            .await
            .map_err(|e| format!("shell request failed: {e}"))?;
        Ok(channel)
    }

    /// Change the pty size of an existing shell channel.
    pub async fn resize(
        channel: &mut Channel<russh::client::Msg>,
        rows: u32,
        cols: u32,
    ) -> Result<(), String> {
        channel
            .window_change(cols, rows, 0, 0)
            .await
            .map_err(|e| format!("window change failed: {e}"))
    }
}

/// One-shot connect test for the "Test connection" button: dials, runs a
/// trivial command, disconnects. Nothing is cached.
pub async fn test_connect(
    profile: &DeviceProfile,
    host_key_store: &Arc<HostKeyStore>,
) -> Result<String, String> {
    let session = DeviceSession::new(profile, host_key_store.clone());
    session.ensure_connected().await?;
    let out = session
        .exec("uname -s && git --version", DEFAULT_EXEC_TIMEOUT)
        .await
        .map_err(|e| format!("connected, but probe failed: {e}"))?;
    session.disconnect().await;
    Ok(out.stdout_str().trim().to_string())
}

/// Pool of live sessions keyed by device id.
#[derive(Default)]
pub struct DeviceRegistry {
    sessions: parking_lot::Mutex<std::collections::HashMap<String, Arc<DeviceSession>>>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The session for `profile`, connected. Reuses a live session.
    pub async fn session_for(
        &self,
        profile: &DeviceProfile,
        host_key_store: &Arc<HostKeyStore>,
    ) -> Result<Arc<DeviceSession>, String> {
        let existing = self.sessions.lock().get(&profile.id).cloned();
        if let Some(session) = existing {
            session.ensure_connected().await?;
            return Ok(session);
        }
        let session = DeviceSession::new(profile, host_key_store.clone());
        session.ensure_connected().await?;
        self.sessions
            .lock()
            .insert(profile.id.clone(), session.clone());
        Ok(session)
    }

    /// Live session without connecting (None when never connected).
    pub fn peek(&self, device_id: &str) -> Option<Arc<DeviceSession>> {
        self.sessions.lock().get(device_id).cloned()
    }

    /// Current status without connecting.
    pub fn status(&self, device_id: &str) -> DeviceStatus {
        self.peek(device_id)
            .map(|s| s.status())
            .unwrap_or(DeviceStatus::Disconnected)
    }

    /// Take the session out (e.g. before forgetting a device).
    pub fn remove(&self, device_id: &str) -> Option<Arc<DeviceSession>> {
        self.sessions.lock().remove(device_id)
    }
}
