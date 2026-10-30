//! Frontend prompt round-trips for the remote layer: host-key confirmation
//! and SSH credential (passphrase/password) prompts.
//!
//! Same shape as `auth.rs`: the production emitter is installed once at
//! startup via [`init`] (keeps `Emitter for AppHandle` monomorphization out
//! of test binaries), requests are parked on a tokio oneshot under a
//! request id, and the FE answers through `device_hostkey_respond` /
//! `device_auth_respond`. Everything times out — a prompt never wedges a
//! connection attempt forever.

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use russh::keys::PublicKeyOrCertificate;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use ts_rs::TS;

/// `hostkey-request` event name.
pub const HOSTKEY_REQUEST_EVENT: &str = "hostkey-request";
/// `device-auth-request` event name.
pub const DEVICE_AUTH_REQUEST_EVENT: &str = "device-auth-request";

/// How long a pending prompt waits for the FE before the connection attempt
/// fails with a timeout.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

/// Prompt kind for [`DeviceAuthRequestEvent`] (string union in TS).
pub const KIND_SSH_PASSPHRASE: &str = "ssh-passphrase";
pub const KIND_DEVICE_PASSWORD: &str = "device-password";

/// `hostkey-request` payload. `status` is "unknown" (first contact) or
/// "changed" (key mismatch — hard warning).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct HostKeyRequestEvent {
    pub request_id: String,
    pub device_id: String,
    pub host: String,
    pub port: u16,
    pub algo: String,
    /// `SHA256:<base64>` fingerprint of the presented key.
    pub fingerprint: String,
    /// "unknown" | "changed".
    #[ts(type = "\"unknown\" | \"changed\"")]
    pub status: String,
    /// Previously pinned fingerprint (present when `status == "changed"`).
    pub pinned_fingerprint: Option<String>,
}

/// `device-auth-request` payload (SSH credentials for a device connection).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DeviceAuthRequestEvent {
    pub request_id: String,
    pub device_id: String,
    /// "ssh-passphrase" | "device-password".
    #[ts(type = "\"ssh-passphrase\" | \"device-password\"")]
    pub kind: String,
    pub prompt: String,
}

/// The FE's answer to a secret prompt.
#[derive(Debug, Clone, Deserialize)]
pub struct SecretAnswer {
    pub password: String,
    /// Persist back to the OS keyring.
    pub store: bool,
}

enum Pending {
    HostKey(oneshot::Sender<bool>),
    Secret(oneshot::Sender<Option<SecretAnswer>>),
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Emitter seam: production installs a closure that emits a serde payload
/// as a Tauri event on the main window.
type EmitFn = Box<dyn Fn(&'static str, &serde_json::Value) + Send + Sync>;
static EMIT: OnceLock<EmitFn> = OnceLock::new();

/// Install the production event emitter. Called from `lib.rs` setup.
pub fn init<F>(emit: F)
where
    F: Fn(&'static str, &serde_json::Value) + Send + Sync + 'static,
{
    let _ = EMIT.set(Box::new(emit));
}

fn emit_event(event: &'static str, payload: &impl Serialize) {
    if let Some(emit) = EMIT.get() {
        let value = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        emit(event, &value);
    }
}

/// Broadcast a device status transition (`device-status` event).
pub(crate) fn emit_status(payload: &super::transport::DeviceStatusEvent) {
    emit_event("device-status", payload);
}

/// Ask the user to confirm a host key during the SSH handshake. Resolves
/// `true` when accepted (the caller pins it), `false` on reject.
pub async fn ask_host_key(event: HostKeyRequestEvent) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    let request_id = event.request_id.clone();
    PENDING
        .lock()
        .insert(request_id.clone(), Pending::HostKey(tx));
    emit_event(HOSTKEY_REQUEST_EVENT, &event);
    match tokio::time::timeout(PROMPT_TIMEOUT, rx).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(_)) => Err("host key prompt was cancelled".to_string()),
        Err(_) => {
            PENDING.lock().remove(&request_id);
            Err("host key prompt timed out".to_string())
        }
    }
}

/// Ask the user for an SSH secret (key passphrase / device password).
/// `None` = user cancelled.
pub async fn ask_secret(event: DeviceAuthRequestEvent) -> Result<Option<SecretAnswer>, String> {
    let (tx, rx) = oneshot::channel();
    let request_id = event.request_id.clone();
    PENDING
        .lock()
        .insert(request_id.clone(), Pending::Secret(tx));
    emit_event(DEVICE_AUTH_REQUEST_EVENT, &event);
    match tokio::time::timeout(PROMPT_TIMEOUT, rx).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(_)) => Err("credential prompt was cancelled".to_string()),
        Err(_) => {
            PENDING.lock().remove(&request_id);
            Err("credential prompt timed out".to_string())
        }
    }
}

/// FE answer for a `hostkey-request`: accept (true) or reject.
pub fn respond_host_key(request_id: &str, accept: bool) -> bool {
    if let Some(Pending::HostKey(tx)) = PENDING.lock().remove(request_id) {
        tx.send(accept).is_ok()
    } else {
        false
    }
}

/// FE answer for a `device-auth-request`.
pub fn respond_secret(request_id: &str, answer: Option<SecretAnswer>) -> bool {
    if let Some(Pending::Secret(tx)) = PENDING.lock().remove(request_id) {
        tx.send(answer).is_ok()
    } else {
        false
    }
}

/// Fingerprint + algorithm of a presented host key, in OpenSSH display
/// format (`SHA256:<base64>`).
pub fn describe_host_key(key: &PublicKeyOrCertificate) -> (String, String) {
    match key {
        PublicKeyOrCertificate::PublicKey { key, .. } => (
            key.algorithm().as_str().to_string(),
            key.fingerprint(russh::keys::HashAlg::Sha256).to_string(),
        ),
        PublicKeyOrCertificate::Certificate(cert) => (
            cert.algorithm().as_str().to_string(),
            cert.public_key()
                .fingerprint(russh::keys::HashAlg::Sha256)
                .to_string(),
        ),
    }
}

/// Test helper: assert nothing is left parked.
#[cfg(test)]
pub fn pending_count() -> usize {
    PENDING.lock().len()
}

/// Test-only: resolve every pending prompt (prevents cross-test bleed).
#[cfg(test)]
pub fn clear_pending() {
    PENDING.lock().clear();
}
