//! Device service functions: profile CRUD, connect/test, host-key +
//! credential prompt answers, remote filesystem browsing, repo scanning, and
//! `remote_repo_open` (the remote twin of `repo_open`).
//!
//! Secrets never appear in parameters that get logged: the UI sends them
//! once, they go straight into the OS keyring, and are never stored in the
//! profile file or included in error strings.

use serde::{Deserialize, Serialize};

use super::profiles::{
    self, delete_secrets, sanitize_profile, AuthKind, DeviceAuth, DeviceProfile,
};
use super::prompts;
use super::transport::{test_connect, DeviceRegistry};
use crate::service::AppServices;

/// One device as the FE sees it: the profile plus live connection status.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub profile: DeviceProfile,
    /// "disconnected" | "connecting" | "connected".
    pub status: String,
}

/// One repository candidate found by `device_scan_repos`.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceRepoCandidate {
    /// Absolute POSIX path on the device.
    pub path: String,
    /// Directory name.
    pub name: String,
}

fn device_info(registry: &DeviceRegistry, profile: DeviceProfile) -> DeviceInfo {
    DeviceInfo {
        status: registry.status(&profile.id).as_str().to_string(),
        profile,
    }
}

// ---------- profile CRUD ----------

pub fn device_list(services: &AppServices) -> Result<Vec<DeviceInfo>, String> {
    Ok(services
        .device_store()
        .list()
        .into_iter()
        .map(|p| device_info(&services.devices, p))
        .collect())
}

/// Parameters for creating/updating a device. Secrets are optional and
/// write-once into the keyring (`None` = leave whatever is stored).
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceInput {
    pub id: Option<String>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub kind: AuthKind,
    pub key_path: Option<String>,
    pub has_passphrase: bool,
    /// Store/replace the password in the keyring (Password auth).
    pub password: Option<String>,
    /// Store/replace the key passphrase in the keyring.
    pub passphrase: Option<String>,
    pub poll_secs: Option<u32>,
}

fn profile_from_input(input: &DeviceInput, id: String) -> DeviceProfile {
    sanitize_profile(DeviceProfile {
        id,
        name: input.name.clone(),
        host: input.host.clone(),
        port: if input.port == 0 { 22 } else { input.port },
        username: input.username.clone(),
        auth: DeviceAuth {
            kind: input.kind,
            key_path: input.key_path.clone(),
            has_passphrase: input.has_passphrase,
        },
        poll_secs: input.poll_secs.unwrap_or(profiles::DEFAULT_POLL_SECS),
    })
}

fn store_secrets(id: &str, input: &DeviceInput) -> Result<(), String> {
    if let Some(password) = &input.password {
        if input.kind == AuthKind::Password && !password.is_empty() {
            profiles::set_secret(id, "password", password)?;
        }
    }
    if let Some(passphrase) = &input.passphrase {
        if input.kind == AuthKind::KeyFile && input.has_passphrase && !passphrase.is_empty() {
            profiles::set_secret(id, "passphrase", passphrase)?;
        }
    }
    Ok(())
}

pub async fn device_add(services: &AppServices, input: DeviceInput) -> Result<DeviceInfo, String> {
    let id = profiles::new_device_id();
    let profile = profile_from_input(&input, id.clone());
    store_secrets(&id, &input)?;
    services.device_store().upsert(profile.clone())?;
    Ok(device_info(&services.devices, profile))
}

pub async fn device_update(
    services: &AppServices,
    input: DeviceInput,
) -> Result<DeviceInfo, String> {
    let id = input
        .id
        .clone()
        .ok_or_else(|| "device id is required for updates".to_string())?;
    let store = services.device_store();
    if store.get(&id).is_none() {
        return Err(format!("unknown device: {id}"));
    }
    let profile = profile_from_input(&input, id.clone());
    store_secrets(&id, &input)?;
    store.upsert(profile.clone())?;
    Ok(device_info(&services.devices, profile))
}

pub async fn device_remove(services: &AppServices, device_id: String) -> Result<(), String> {
    // Drop the live session first (it keys on the device id).
    if let Some(session) = services.devices.remove(&device_id) {
        session.disconnect().await;
    }
    services.device_store().remove(&device_id)?;
    delete_secrets(&device_id)?;
    Ok(())
}

// ---------- connection lifecycle ----------

pub async fn device_connect(services: &AppServices, device_id: String) -> Result<String, String> {
    let profile = services
        .device_store()
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    let session = services
        .devices
        .session_for(&profile, &services.host_key_store())
        .await?;
    Ok(session.status().as_str().to_string())
}

pub async fn device_disconnect(services: &AppServices, device_id: String) -> Result<(), String> {
    if let Some(session) = services.devices.peek(&device_id) {
        session.disconnect().await;
    }
    Ok(())
}

pub fn device_status(services: &AppServices, device_id: String) -> String {
    services.devices.status(&device_id).as_str().to_string()
}

/// Probe-only connect (Test connection button). Nothing is persisted.
pub async fn device_test(services: &AppServices, input: DeviceInput) -> Result<String, String> {
    let id = input.id.clone().unwrap_or_else(profiles::new_device_id);
    let profile = profile_from_input(&input, id);
    // Secrets for an unsaved device: use what the FE just typed (same
    // store rules as add/update; probe failures ignore write errors).
    let _ = store_secrets(&profile.id, &input);
    let result = test_connect(&profile, &services.host_key_store()).await;
    if input.id.is_none() {
        // Probe-only device: the throwaway id's secrets must not linger in
        // the keyring — the real add stores them under its own id.
        let _ = profiles::delete_secrets(&profile.id);
    }
    result
}

// ---------- polling ----------

/// Pause/resume remote repo change polling. The FE flips this with the
/// window's `visibilitychange` (spec: polling is "paused when tab hidden").
pub fn remote_poll_set_paused(_services: &AppServices, paused: bool) -> Result<(), String> {
    super::polling::set_paused(paused);
    Ok(())
}

// ---------- prompt answers ----------

/// FE answer for a `hostkey-request` event.
pub fn device_hostkey_respond(
    _services: &AppServices,
    request_id: String,
    accept: bool,
) -> Result<(), String> {
    if prompts::respond_host_key(&request_id, accept) {
        Ok(())
    } else {
        Err(format!("no pending host key request: {request_id}"))
    }
}

/// FE answer for a `device-auth-request` event.
pub fn device_auth_respond(
    _services: &AppServices,
    request_id: String,
    password: Option<String>,
    store: bool,
) -> Result<(), String> {
    let answer = password.map(|password| prompts::SecretAnswer { password, store });
    if prompts::respond_secret(&request_id, answer) {
        Ok(())
    } else {
        Err(format!("no pending credential request: {request_id}"))
    }
}

/// Drop the pinned host key for a device (device settings).
pub fn device_forget_host_key(services: &AppServices, device_id: String) -> Result<bool, String> {
    let profile = services
        .device_store()
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    services.host_key_store().forget(&profile.host_port())
}

/// The pinned host key for a device (device detail view), if any.
pub fn device_host_key(
    services: &AppServices,
    device_id: String,
) -> Result<Option<super::hostkeys::KnownHostKey>, String> {
    let profile = services
        .device_store()
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    Ok(services.host_key_store().known(&profile.host_port()))
}

// ---------- remote filesystem ----------

/// List a directory anywhere on the device (absolute POSIX path, "" or "~"
/// for the home directory).
pub async fn device_browse_dir(
    services: &AppServices,
    device_id: String,
    path: String,
) -> Result<crate::workspace::DirListing, String> {
    let profile = services
        .device_store()
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    let session = services
        .devices
        .session_for(&profile, &services.host_key_store())
        .await?;
    let dir = resolve_browse_path(&session, &path).await?;
    // The workspace surface is synchronous and bridges into the async world
    // with `block_on`, so it must run on a blocking thread: calling it here
    // (inside a tokio task) panics with "Cannot start a runtime from within
    // a runtime" and takes the app down (panic = "abort" in release).
    let mut listing = {
        let workspace = crate::remote::workspace::RemoteWorkspace::new(session, dir.clone());
        crate::rt::spawn_blocking(move || workspace.list_dir("", false))
            .await
            .map_err(|e| format!("device browse failed: {e}"))??
    };
    // Paths in a device browse are ABSOLUTE (unlike repo browsing).
    for entry in listing.entries.iter_mut() {
        entry.path = if dir == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{}/{}", dir, entry.name)
        };
    }
    listing.path = dir;
    Ok(listing)
}

/// Resolve the device's home directory (`printf '%s' "$HOME"` — no
/// interpolation into the command, so nothing to quote).
async fn remote_home(session: &super::transport::DeviceSession) -> Result<String, String> {
    let out = session
        .exec(
            "printf '%s' \"$HOME\"",
            super::transport::DEFAULT_EXEC_TIMEOUT,
        )
        .await?;
    let home = out.stdout_str().trim().to_string();
    Ok(home)
}

async fn resolve_browse_path(
    session: &super::transport::DeviceSession,
    path: &str,
) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "~" || trimmed == "/" {
        // Resolve the device home directory once per call (cheap).
        let home = remote_home(session).await?;
        return Ok(if home.is_empty() {
            "/".to_string()
        } else {
            home
        });
    }
    // `~`-expansion passthrough.
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = remote_home(session).await?;
        return Ok(format!("{}/{}", home.trim_end_matches('/'), rest));
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

/// Find git repositories under `path` (default: home), depth-limited,
/// skipping dependency/build dirs. Returns up to 200 candidates.
pub async fn device_scan_repos(
    services: &AppServices,
    device_id: String,
    root: Option<String>,
) -> Result<Vec<DeviceRepoCandidate>, String> {
    let profile = services
        .device_store()
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    let session = services
        .devices
        .session_for(&profile, &services.host_key_store())
        .await?;
    let base = match root.as_deref() {
        Some(r) if !r.trim().is_empty() => r.trim().to_string(),
        _ => {
            let home = remote_home(&session).await?;
            if home.is_empty() {
                "/".to_string()
            } else {
                home
            }
        }
    };
    let cmd = format!(
        "find {} -xdev -maxdepth 4 -type d -name .git \
         -not -path '*/node_modules/*' -not -path '*/target/*' \
         -not -path '*/dist/*' -not -path '*/build/*' -not -path '*/.cache/*' \
         2>/dev/null | head -200 | while IFS= read -r g; do dirname \"$g\"; done",
        super::quoting::shell_quote(&base)
    );
    let out = session
        .exec(&cmd, super::transport::LONG_EXEC_TIMEOUT)
        .await?;
    let out = out.checked("scan")?;
    Ok(out
        .stdout_str()
        .lines()
        .filter_map(|line| {
            let path = line.trim();
            if path.is_empty() {
                return None;
            }
            let name = path.rsplit('/').next().unwrap_or(path).to_string();
            Some(DeviceRepoCandidate {
                path: path.to_string(),
                name,
            })
        })
        .collect())
}

// ---------- repo open ----------

/// Open a repository that lives on a device (the remote twin of
/// `repo_open`).
pub async fn remote_repo_open(
    services: &AppServices,
    device_id: String,
    path: String,
) -> Result<crate::engine::types::RepoInfo, String> {
    let profile = services
        .device_store()
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    let session = services
        .devices
        .session_for(&profile, &services.host_key_store())
        .await?;
    // `open_remote` runs the synchronous engine (block_on bridges inside
    // `discover`), so it must hop off the async worker: calling it here
    // (inside a tokio task) panics, which kills the app in release builds
    // and silently drops the reply otherwise (the FE "Add" no-op).
    //
    // `RepoManager` is neither `Clone` nor `'static`, so it cannot be moved
    // into `rt::spawn_blocking`; a scoped thread owned by this call gives
    // the same guarantee (discovery never runs on an async worker) while
    // borrowing the services bag.
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                services.repos.open_remote(
                    &profile.id,
                    &profile.name,
                    profile.poll_secs,
                    session,
                    &path,
                )
            })
            .join()
            .map_err(|_| "remote repo open failed: discovery thread panicked".to_string())?
    })
}
