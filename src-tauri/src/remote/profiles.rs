//! Remote device profiles: persisted connection definitions + keyring-backed
//! secrets.
//!
//! A profile is everything needed to open an SSH session except the secrets:
//! host, port, username, auth method (password / key file + optional
//! passphrase), and the repo-change poll interval. Profiles live in
//! `<app-data>/remote/devices.json`; secrets (password, key passphrase) live
//! ONLY in the OS keyring under the same `"mygitui"` service the rest of the
//! app uses (`ssh-device:<id>:password` / `ssh-device:<id>:passphrase`), and
//! are never written to disk or logs.
//!
//! The store is plain-JSON on explicit paths so it round-trips in unit tests
//! without a Tauri app; the IPC layer supplies the real app-data dir.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Keyring service — same value as `keyring_store` / `auth` (duplicated
/// there for the same reason: entries are read via the keyring crate
/// directly, not through the `secrets_*` IPC commands).
const SERVICE: &str = "mygitui";

/// Default repo-change poll interval for remote repos (seconds). Used when
/// a device profile does not override `poll_secs` (the FE sends `None` /
/// omits the field; see `DeviceInput`).
pub const DEFAULT_POLL_SECS: u32 = 5;

/// Lower clamp for a device's `poll_secs`: below 2s the per-device poll
/// would hammer the SSH session with a status exec per tick.
pub const MIN_POLL_SECS: u32 = 2;

/// Upper clamp for a device's `poll_secs`: above 5 minutes a poller stops
/// being a change feed (the user almost certainly meant minutes, and a
/// typo'd day-scale value would leave the tab silently stale).
pub const MAX_POLL_SECS: u32 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    /// Username/password auth (password in the OS keyring).
    Password,
    /// Private key file auth (`key_path` on the local machine; passphrase in
    /// the keyring when `has_passphrase`).
    KeyFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeviceAuth {
    pub kind: AuthKind,
    /// Local path to the private key (KeyFile only).
    pub key_path: Option<String>,
    /// True when a passphrase is (or should be) stored in the keyring.
    pub has_passphrase: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeviceProfile {
    pub id: String,
    /// Display name (tab/home list).
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: DeviceAuth,
    /// Repo-change poll interval for repos opened on this device (seconds,
    /// clamped to 2-300 on read and write; the poller in `remote/polling.rs`
    /// consumes exactly this value). `serde(default)` keeps pre-field and
    /// hand-edited `devices.json` files parsing with the global default
    /// (5s) — the store's load is all-or-nothing, so a required field would
    /// silently wipe every saved device.
    #[serde(default = "default_poll_secs")]
    pub poll_secs: u32,
    /// Connect automatically when the app launches (opt-in). `serde(default)`
    /// keeps pre-field `devices.json` files parsing: the store's load is
    /// all-or-nothing, so a required field would silently wipe every saved
    /// device.
    #[serde(default)]
    pub auto_connect: bool,
}

impl DeviceProfile {
    pub fn host_port(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// `serde(default)` hook for [`DeviceProfile::poll_secs`].
fn default_poll_secs() -> u32 {
    DEFAULT_POLL_SECS
}

/// JSON file backing the device list.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DeviceFile {
    devices: Vec<DeviceProfile>,
}

/// Load/save + CRUD over one `devices.json`. All mutating methods write
/// through (the store is tiny; atomicity beats batching at this size).
pub struct DeviceStore {
    path: PathBuf,
}

impl DeviceStore {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Store rooted at `<dir>/remote/devices.json`.
    pub fn under_dir(dir: &Path) -> Self {
        Self::at(dir.join("remote").join("devices.json"))
    }

    fn load_file(&self) -> DeviceFile {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                // Clamp on read too: a hand-edited `devices.json` (or one
                // written by an older build with different clamps) must not
                // smuggle an out-of-range poll interval into the poller.
                let mut file: DeviceFile = serde_json::from_slice(&bytes).unwrap_or_default();
                for device in &mut file.devices {
                    device.poll_secs = clamp_poll_secs(device.poll_secs);
                }
                file
            }
            Err(_) => DeviceFile::default(),
        }
    }

    fn save_file(&self, file: &DeviceFile) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create device store dir: {e}"))?;
        }
        let bytes = serde_json::to_vec_pretty(file).map_err(|e| format!("serialize: {e}"))?;
        // Write-then-rename so a crash mid-write never truncates the store.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes).map_err(|e| format!("write devices.json: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("swap devices.json: {e}"))
    }

    pub fn list(&self) -> Vec<DeviceProfile> {
        self.load_file().devices
    }

    pub fn get(&self, id: &str) -> Option<DeviceProfile> {
        self.load_file().devices.into_iter().find(|d| d.id == id)
    }

    /// Insert or replace by id.
    pub fn upsert(&self, profile: DeviceProfile) -> Result<(), String> {
        let mut file = self.load_file();
        match file.devices.iter_mut().find(|d| d.id == profile.id) {
            Some(slot) => *slot = profile,
            None => file.devices.push(profile),
        }
        self.save_file(&file)
    }

    /// Remove by id; returns false when absent.
    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut file = self.load_file();
        let before = file.devices.len();
        file.devices.retain(|d| d.id != id);
        if file.devices.len() == before {
            return Ok(false);
        }
        self.save_file(&file)?;
        Ok(true)
    }
}

/// New profile id (`dev-<uuid v4>`).
pub fn new_device_id() -> String {
    format!("dev-{}", uuid::Uuid::new_v4())
}

/// Clamp a poll interval to the accepted per-device range
/// ([`MIN_POLL_SECS`]..=[`MAX_POLL_SECS`]). Applied on write
/// ([`sanitize_profile`]) and read ([`DeviceStore::load_file`]).
pub fn clamp_poll_secs(secs: u32) -> u32 {
    secs.clamp(MIN_POLL_SECS, MAX_POLL_SECS)
}

/// Normalize a user-supplied profile before persisting: trim whitespace,
/// default the port to 22, clamp the poll interval to 2-300s.
pub fn sanitize_profile(mut profile: DeviceProfile) -> DeviceProfile {
    profile.name = profile.name.trim().to_string();
    profile.host = profile.host.trim().to_string();
    profile.username = profile.username.trim().to_string();
    if profile.port == 0 {
        profile.port = 22;
    }
    if let Some(p) = profile.auth.key_path.as_mut() {
        *p = p.trim().to_string();
    }
    profile.poll_secs = clamp_poll_secs(profile.poll_secs);
    if profile.name.is_empty() {
        profile.name = format!("{}@{}", profile.username, profile.host);
    }
    profile
}

// ---------- keyring secrets ----------

fn keyring_entry(key: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, key).map_err(|e| format!("failed to open secret store: {e}"))
}

/// Keyring key for a device secret (`"password"` | `"passphrase"`).
pub fn secret_key(device_id: &str, kind: &str) -> String {
    format!("ssh-device:{device_id}:{kind}")
}

/// Read a device secret from the OS keyring (`None` = not stored).
pub fn get_secret(device_id: &str, kind: &str) -> Result<Option<String>, String> {
    match keyring_entry(&secret_key(device_id, kind))?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("failed to read secret: {e}")),
    }
}

/// Store (or overwrite) a device secret in the OS keyring.
pub fn set_secret(device_id: &str, kind: &str, value: &str) -> Result<(), String> {
    keyring_entry(&secret_key(device_id, kind))?
        .set_password(value)
        .map_err(|e| format!("failed to store secret: {e}"))
}

/// Delete both secret kinds for a device. Idempotent.
pub fn delete_secrets(device_id: &str) -> Result<(), String> {
    for kind in ["password", "passphrase"] {
        match keyring_entry(&secret_key(device_id, kind))?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(format!("failed to delete secret: {e}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DeviceStore {
        let dir =
            std::env::temp_dir().join(format!("mygitui-devices-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        DeviceStore::under_dir(&dir)
    }

    fn sample(id: &str) -> DeviceProfile {
        DeviceProfile {
            id: id.to_string(),
            name: "pi".to_string(),
            host: "192.168.1.42".to_string(),
            port: 22,
            username: "raul".to_string(),
            auth: DeviceAuth {
                kind: AuthKind::KeyFile,
                key_path: Some("C:\\keys\\id_ed25519".to_string()),
                has_passphrase: true,
            },
            poll_secs: DEFAULT_POLL_SECS,
            auto_connect: false,
        }
    }

    /// A profile JSON written before `auto_connect` existed must parse with
    /// the default (`false`) instead of failing the whole-file load (which
    /// would wipe every saved device).
    #[test]
    fn legacy_profile_json_defaults_auto_connect() {
        let store = temp_store("legacy-auto-connect");
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(
            &store.path,
            r#"{"devices":[{"id":"a","name":"pi","host":"192.168.1.42","port":22,"username":"raul","auth":{"kind":"key_file","key_path":"k","has_passphrase":true},"poll_secs":5}]}"#,
        )
        .unwrap();
        let devices = store.list();
        assert_eq!(devices.len(), 1);
        assert!(!devices[0].auto_connect);
    }

    #[test]
    fn upsert_get_remove_roundtrip() {
        let store = temp_store("crud");
        assert!(store.list().is_empty());
        assert!(store.get("nope").is_none());

        store.upsert(sample("a")).unwrap();
        assert_eq!(store.get("a").unwrap(), sample("a"));

        // Upsert replaces by id, keeps others.
        store.upsert(sample("b")).unwrap();
        let mut renamed = sample("a");
        renamed.name = "server".to_string();
        store.upsert(renamed).unwrap();
        assert_eq!(store.list().len(), 2);
        assert_eq!(store.get("a").unwrap().name, "server");

        assert!(store.remove("a").unwrap());
        assert!(!store.remove("a").unwrap(), "second remove is false");
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn missing_file_is_empty_store() {
        let store = temp_store("missing");
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        assert!(store.list().is_empty());
    }

    #[test]
    fn corrupt_file_degrades_to_empty() {
        let store = temp_store("corrupt");
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(&store.path, b"{ not json").unwrap();
        assert!(store.list().is_empty());
        // And the store recovers: a save overwrites the corruption.
        store.upsert(sample("a")).unwrap();
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn sanitize_trims_and_defaults() {
        let mut p = sample("a");
        p.name = "  ".to_string();
        p.host = " h ".to_string();
        p.username = " u ".to_string();
        p.port = 0;
        p.poll_secs = 1;
        p.auth.key_path = Some(" k ".to_string());
        let s = sanitize_profile(p);
        assert_eq!(s.port, 22);
        assert_eq!(s.poll_secs, MIN_POLL_SECS);
        assert_eq!(s.host, "h");
        assert_eq!(s.username, "u");
        assert_eq!(s.auth.key_path.as_deref(), Some("k"));
        // Empty name falls back to user@host.
        assert_eq!(s.name, "u@h");
    }

    /// The per-device poll interval is clamped to 2-300s on write
    /// (`sanitize_profile`), so both typo'd day-scale values and
    /// sub-2-second SSH hammering are rejected at the boundary.
    #[test]
    fn poll_secs_clamps_to_2_300_on_write() {
        assert_eq!(clamp_poll_secs(0), MIN_POLL_SECS);
        assert_eq!(clamp_poll_secs(1), MIN_POLL_SECS);
        assert_eq!(clamp_poll_secs(2), 2);
        assert_eq!(clamp_poll_secs(45), 45);
        assert_eq!(clamp_poll_secs(300), 300);
        assert_eq!(clamp_poll_secs(301), MAX_POLL_SECS);
        assert_eq!(clamp_poll_secs(u32::MAX), MAX_POLL_SECS);

        let mut p = sample("clamp");
        p.poll_secs = 3600;
        assert_eq!(sanitize_profile(p).poll_secs, MAX_POLL_SECS);
        let mut p = sample("clamp-low");
        p.poll_secs = 0;
        assert_eq!(sanitize_profile(p).poll_secs, MIN_POLL_SECS);
    }

    /// A `devices.json` written before `poll_secs` existed (or hand-edited
    /// without it) parses with the global default instead of failing the
    /// whole-file load (which would wipe every saved device).
    #[test]
    fn legacy_profile_json_defaults_poll_secs() {
        let store = temp_store("legacy-poll-secs");
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(
            &store.path,
            r#"{"devices":[{"id":"a","name":"pi","host":"192.168.1.42","port":22,"username":"raul","auth":{"kind":"key_file","key_path":"k","has_passphrase":true},"auto_connect":true}]}"#,
        )
        .unwrap();
        let devices = store.list();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].poll_secs, DEFAULT_POLL_SECS);
    }

    /// Clamps apply on READ too: a hand-edited out-of-range interval in the
    /// store cannot reach the poller.
    #[test]
    fn poll_secs_clamps_on_read() {
        let store = temp_store("read-clamp");
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(
            &store.path,
            r#"{"devices":[{"id":"a","name":"pi","host":"192.168.1.42","port":22,"username":"raul","auth":{"kind":"key_file","key_path":"k","has_passphrase":true},"poll_secs":9999}]}"#,
        )
        .unwrap();
        assert_eq!(store.get("a").unwrap().poll_secs, MAX_POLL_SECS);
    }

    /// Item 60: the per-device poll override round-trips through the store
    /// exactly as `device_add`/`device_update` write it
    /// (`DeviceInput.poll_secs: Option<u32>` → `unwrap_or(DEFAULT)` →
    /// sanitize → upsert), and `remote_repo_open` hands the persisted value
    /// to the poller via `profile.poll_secs`.
    #[test]
    fn per_device_poll_override_roundtrips_through_the_store() {
        let store = temp_store("poll-override");
        // The device_update path: a user override survives sanitize + persist.
        let mut input = sample("a");
        input.poll_secs = 42;
        let profile = sanitize_profile(input);
        store.upsert(profile.clone()).unwrap();
        assert_eq!(store.get("a").unwrap().poll_secs, 42);

        // No override (`None` from the FE → `unwrap_or(DEFAULT)`) lands on
        // the global default.
        let mut profile_b = sample("b");
        profile_b.poll_secs = DEFAULT_POLL_SECS;
        store.upsert(sanitize_profile(profile_b)).unwrap();
        assert_eq!(store.get("b").unwrap().poll_secs, DEFAULT_POLL_SECS);

        // Reload from disk (fresh store over the same file): the override
        // is exactly what `remote_repo_open` will pass to the poller.
        let reloaded = DeviceStore::at(store.path.clone());
        assert_eq!(reloaded.get("a").unwrap().poll_secs, 42);
        assert_eq!(reloaded.get("b").unwrap().poll_secs, DEFAULT_POLL_SECS);
    }

    #[test]
    fn host_port_formats() {
        let p = sample("a");
        assert_eq!(p.host_port(), "192.168.1.42:22");
    }

    #[test]
    fn secret_keys_namespaced() {
        assert_eq!(secret_key("dev-1", "password"), "ssh-device:dev-1:password");
        assert_eq!(
            secret_key("dev-1", "passphrase"),
            "ssh-device:dev-1:passphrase"
        );
    }
}
