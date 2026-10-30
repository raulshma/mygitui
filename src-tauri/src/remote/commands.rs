//! Device IPC commands: profile CRUD, connect/test, host-key + credential
//! prompt answers, remote filesystem browsing, repo scanning, and
//! `remote_repo_open` (the remote twin of `repo_open`).
//!
//! Secrets never appear in command parameters that get logged: the FE sends
//! them once over IPC, they go straight into the OS keyring, and are never
//! stored in the profile file or included in error strings.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};
use ts_rs::TS;

use super::hostkeys::HostKeyStore;
use super::profiles::{
    self, delete_secrets, sanitize_profile, AuthKind, DeviceAuth, DeviceProfile, DeviceStore,
};
use super::prompts;
use super::transport::{test_connect, DeviceRegistry};
use crate::repo::RepoManager;

/// `<app-data>/remote/…` stores rooted at the app data dir.
pub fn device_store(app: &tauri::AppHandle) -> DeviceStore {
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    DeviceStore::under_dir(&dir)
}

pub fn host_key_store(app: &tauri::AppHandle) -> Arc<HostKeyStore> {
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    Arc::new(HostKeyStore::under_dir(&dir))
}

/// One device as the FE sees it: the profile plus live connection status.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DeviceInfo {
    pub profile: DeviceProfile,
    /// "disconnected" | "connecting" | "connected".
    #[ts(type = "\"disconnected\" | \"connecting\" | \"connected\"")]
    pub status: String,
}

/// One repository candidate found by `device_scan_repos`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
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

#[tauri::command(rename_all = "snake_case")]
pub fn device_list(
    app: tauri::AppHandle,
    registry: State<'_, DeviceRegistry>,
) -> Result<Vec<DeviceInfo>, String> {
    Ok(device_store(&app)
        .list()
        .into_iter()
        .map(|p| device_info(&registry, p))
        .collect())
}

/// Parameters for creating/updating a device. Secrets are optional and
/// write-once into the keyring (`None` = leave whatever is stored).
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
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

#[tauri::command(rename_all = "snake_case")]
pub async fn device_add(
    input: DeviceInput,
    app: tauri::AppHandle,
    registry: State<'_, DeviceRegistry>,
) -> Result<DeviceInfo, String> {
    let id = profiles::new_device_id();
    let profile = profile_from_input(&input, id.clone());
    store_secrets(&id, &input)?;
    device_store(&app).upsert(profile.clone())?;
    Ok(device_info(&registry, profile))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn device_update(
    input: DeviceInput,
    app: tauri::AppHandle,
    registry: State<'_, DeviceRegistry>,
) -> Result<DeviceInfo, String> {
    let id = input
        .id
        .clone()
        .ok_or_else(|| "device id is required for updates".to_string())?;
    let store = device_store(&app);
    if store.get(&id).is_none() {
        return Err(format!("unknown device: {id}"));
    }
    let profile = profile_from_input(&input, id.clone());
    store_secrets(&id, &input)?;
    store.upsert(profile.clone())?;
    Ok(device_info(&registry, profile))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn device_remove(
    device_id: String,
    app: tauri::AppHandle,
    registry: State<'_, DeviceRegistry>,
) -> Result<(), String> {
    // Drop the live session first (it keys on the device id).
    if let Some(session) = registry.remove(&device_id) {
        session.disconnect().await;
    }
    device_store(&app).remove(&device_id)?;
    delete_secrets(&device_id)?;
    Ok(())
}

// ---------- connection lifecycle ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn device_connect(
    device_id: String,
    app: tauri::AppHandle,
    registry: State<'_, DeviceRegistry>,
) -> Result<String, String> {
    let profile = device_store(&app)
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    let session = registry
        .session_for(&profile, &host_key_store(&app))
        .await?;
    Ok(session.status().as_str().to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn device_disconnect(
    device_id: String,
    registry: State<'_, DeviceRegistry>,
) -> Result<(), String> {
    if let Some(session) = registry.peek(&device_id) {
        session.disconnect().await;
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn device_status(device_id: String, registry: State<'_, DeviceRegistry>) -> String {
    registry.status(&device_id).as_str().to_string()
}

/// Probe-only connect (Test connection button). Nothing is persisted.
#[tauri::command(rename_all = "snake_case")]
pub async fn device_test(input: DeviceInput, app: tauri::AppHandle) -> Result<String, String> {
    let id = input.id.clone().unwrap_or_else(profiles::new_device_id);
    let profile = profile_from_input(&input, id);
    // Secrets for an unsaved device: use what the FE just typed (same
    // store rules as add/update; probe failures ignore write errors).
    let _ = store_secrets(&profile.id, &input);
    let result = test_connect(&profile, &host_key_store(&app)).await;
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
#[tauri::command(rename_all = "snake_case")]
pub fn remote_poll_set_paused(paused: bool) -> Result<(), String> {
    super::polling::set_paused(paused);
    Ok(())
}

// ---------- prompt answers ----------

/// FE answer for a `hostkey-request` event.
#[tauri::command(rename_all = "snake_case")]
pub fn device_hostkey_respond(request_id: String, accept: bool) -> Result<(), String> {
    if prompts::respond_host_key(&request_id, accept) {
        Ok(())
    } else {
        Err(format!("no pending host key request: {request_id}"))
    }
}

/// FE answer for a `device-auth-request` event.
#[tauri::command(rename_all = "snake_case")]
pub fn device_auth_respond(
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
#[tauri::command(rename_all = "snake_case")]
pub fn device_forget_host_key(device_id: String, app: tauri::AppHandle) -> Result<bool, String> {
    let profile = device_store(&app)
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    host_key_store(&app).forget(&profile.host_port())
}

/// The pinned host key for a device (device detail view), if any.
#[tauri::command(rename_all = "snake_case")]
pub fn device_host_key(
    device_id: String,
    app: tauri::AppHandle,
) -> Result<Option<super::hostkeys::KnownHostKey>, String> {
    let profile = device_store(&app)
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    Ok(host_key_store(&app).known(&profile.host_port()))
}

// ---------- remote filesystem ----------

/// List a directory anywhere on the device (absolute POSIX path, "" or "~"
/// for the home directory).
#[tauri::command(rename_all = "snake_case")]
pub async fn device_browse_dir(
    device_id: String,
    path: String,
    app: tauri::AppHandle,
    registry: State<'_, DeviceRegistry>,
) -> Result<crate::workspace::DirListing, String> {
    let profile = device_store(&app)
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    let session = registry
        .session_for(&profile, &host_key_store(&app))
        .await?;
    let dir = resolve_browse_path(&session, &path).await?;
    // The workspace surface is synchronous and bridges into the async world
    // with `block_on`, so it must run on a blocking thread: calling it here
    // (inside a tokio task) panics with "Cannot start a runtime from within
    // a runtime" and takes the app down (panic = "abort" in release).
    let mut listing = {
        let workspace = crate::remote::workspace::RemoteWorkspace::new(session, dir.clone());
        tauri::async_runtime::spawn_blocking(move || workspace.list_dir("", false))
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
#[tauri::command(rename_all = "snake_case")]
pub async fn device_scan_repos(
    device_id: String,
    root: Option<String>,
    app: tauri::AppHandle,
    registry: State<'_, DeviceRegistry>,
) -> Result<Vec<DeviceRepoCandidate>, String> {
    let profile = device_store(&app)
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    let session = registry
        .session_for(&profile, &host_key_store(&app))
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
#[tauri::command(rename_all = "snake_case")]
pub async fn remote_repo_open(
    device_id: String,
    path: String,
    app: tauri::AppHandle,
    registry: State<'_, DeviceRegistry>,
) -> Result<crate::engine::types::RepoInfo, String> {
    let profile = device_store(&app)
        .get(&device_id)
        .ok_or_else(|| format!("unknown device: {device_id}"))?;
    let session = registry
        .session_for(&profile, &host_key_store(&app))
        .await?;
    // `open_remote` runs the synchronous engine (block_on bridges inside
    // `discover`), so it must hop to a blocking thread: calling it here
    // (inside a tokio task) panics, which kills the app in release builds
    // and silently drops the IPC reply otherwise (the FE "Add" no-op).
    tauri::async_runtime::spawn_blocking(move || {
        let repos = app.state::<RepoManager>();
        repos.open_remote(
            &app,
            &profile.id,
            &profile.name,
            profile.poll_secs,
            session,
            &path,
        )
    })
    .await
    .map_err(|e| format!("remote repo open failed: {e}"))?
}
