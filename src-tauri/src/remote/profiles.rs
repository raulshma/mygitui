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

/// Default repo-change poll interval for remote repos (seconds).
pub const DEFAULT_POLL_SECS: u32 = 5;

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
    /// minimum 1).
    pub poll_secs: u32,
}

impl DeviceProfile {
    pub fn host_port(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
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
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
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

/// Normalize a user-supplied profile before persisting: trim whitespace,
/// default the port to 22, clamp the poll interval to >= 1s.
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
    profile.poll_secs = profile.poll_secs.max(1);
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
        }
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
        p.poll_secs = 0;
        p.auth.key_path = Some(" k ".to_string());
        let s = sanitize_profile(p);
        assert_eq!(s.port, 22);
        assert_eq!(s.poll_secs, 1);
        assert_eq!(s.host, "h");
        assert_eq!(s.username, "u");
        assert_eq!(s.auth.key_path.as_deref(), Some("k"));
        // Empty name falls back to user@host.
        assert_eq!(s.name, "u@h");
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
