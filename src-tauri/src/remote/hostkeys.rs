//! Trust-on-first-use host-key store.
//!
//! OpenSSH-style `known_hosts` semantics, scoped to this app: the first
//! connection to a host pins the host key fingerprint; later connections
//! must match it. Unknown keys go through a FE confirm dialog (fingerprint
//! shown); a CHANGED key is a hard warning that blocks the connection until
//! the user explicitly re-confirms. The store file lives at
//! `<app-data>/remote/known_hosts.json`, keyed by `host:port`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Result of checking a presented host key against the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// Key matches the pinned fingerprint — connect without asking.
    Trusted,
    /// Host is known but presents a DIFFERENT key — possible MITM; hard
    /// warning, connect only after explicit re-confirm.
    Changed,
    /// First contact with this host — show the fingerprint, ask once.
    Unknown,
}

/// One pinned host key (JSON file entry).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnownHostKey {
    /// Key algorithm name as reported by the SSH handshake
    /// ("ssh-ed25519", "rsa-sha2-512", …).
    pub algo: String,
    /// `SHA256:<base64>` fingerprint (OpenSSH display format).
    pub fingerprint: String,
    /// Unix seconds when first trusted.
    pub added_at: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct HostKeyFile {
    /// `host:port` → pinned key.
    hosts: HashMap<String, KnownHostKey>,
}

/// Load/save + TOFU decisions over one `known_hosts.json`.
pub struct HostKeyStore {
    path: PathBuf,
}

impl HostKeyStore {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Store rooted at `<dir>/remote/known_hosts.json`.
    pub fn under_dir(dir: &Path) -> Self {
        Self::at(dir.join("remote").join("known_hosts.json"))
    }

    fn load_file(&self) -> HostKeyFile {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save_file(&self, file: &HostKeyFile) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create known_hosts dir: {e}"))?;
        }
        let bytes = serde_json::to_vec_pretty(file).map_err(|e| format!("serialize: {e}"))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes).map_err(|e| format!("write known_hosts: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("swap known_hosts: {e}"))
    }

    /// Classify the presented key (`host_port` = `"host:port"` normalized by
    /// the caller, lowercase host).
    pub fn status(&self, host_port: &str, algo: &str, fingerprint: &str) -> HostKeyStatus {
        match self.load_file().hosts.get(host_port) {
            None => HostKeyStatus::Unknown,
            Some(pinned) => {
                if pinned.algo == algo && pinned.fingerprint == fingerprint {
                    HostKeyStatus::Trusted
                } else {
                    HostKeyStatus::Changed
                }
            }
        }
    }

    /// The currently pinned key for a host, if any (shown in change warnings
    /// and the device detail view).
    pub fn known(&self, host_port: &str) -> Option<KnownHostKey> {
        self.load_file().hosts.get(host_port).cloned()
    }

    /// Pin (or re-pin after an explicit user confirmation) a host key.
    pub fn trust(&self, host_port: &str, algo: &str, fingerprint: &str) -> Result<(), String> {
        let mut file = self.load_file();
        let added_at = file
            .hosts
            .get(host_port)
            .map(|k| k.added_at)
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            });
        file.hosts.insert(
            host_port.to_string(),
            KnownHostKey {
                algo: algo.to_string(),
                fingerprint: fingerprint.to_string(),
                added_at,
            },
        );
        self.save_file(&file)
    }

    /// Drop the pin for a host ("forget host key" in device settings).
    pub fn forget(&self, host_port: &str) -> Result<bool, String> {
        let mut file = self.load_file();
        let removed = file.hosts.remove(host_port).is_some();
        if removed {
            self.save_file(&file)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> HostKeyStore {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-hostkeys-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        HostKeyStore::under_dir(&dir)
    }

    const ALGO: &str = "ssh-ed25519";
    const FP: &str = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn tofu_lifecycle() {
        let store = temp_store("tofu");
        assert_eq!(store.status("pi:22", ALGO, FP), HostKeyStatus::Unknown);
        assert!(store.known("pi:22").is_none());

        store.trust("pi:22", ALGO, FP).unwrap();
        assert_eq!(store.status("pi:22", ALGO, FP), HostKeyStatus::Trusted);
        let pinned = store.known("pi:22").unwrap();
        assert_eq!(pinned.algo, ALGO);
        assert_eq!(pinned.fingerprint, FP);

        store.forget("pi:22").unwrap();
        assert_eq!(store.status("pi:22", ALGO, FP), HostKeyStatus::Unknown);
    }

    #[test]
    fn changed_key_is_flagged() {
        let store = temp_store("changed");
        store.trust("pi:22", ALGO, FP).unwrap();
        const FP2: &str = "SHA256:BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
        assert_eq!(store.status("pi:22", ALGO, FP2), HostKeyStatus::Changed);
        // Same fingerprint under a different algo name is also a change.
        assert_eq!(
            store.status("pi:22", "rsa-sha2-512", FP),
            HostKeyStatus::Changed
        );
        // Re-pinning after explicit confirmation clears it.
        store.trust("pi:22", ALGO, FP2).unwrap();
        assert_eq!(store.status("pi:22", ALGO, FP2), HostKeyStatus::Trusted);
    }

    #[test]
    fn hosts_are_independent() {
        let store = temp_store("indep");
        store.trust("pi:22", ALGO, FP).unwrap();
        assert_eq!(store.status("nas:22", ALGO, FP), HostKeyStatus::Unknown);
    }

    #[test]
    fn forget_absent_is_false() {
        let store = temp_store("forget");
        assert!(!store.forget("nobody:22").unwrap());
    }

    #[test]
    fn trust_keeps_original_added_at_on_rep_pin() {
        let store = temp_store("addedat");
        store.trust("pi:22", ALGO, FP).unwrap();
        let first = store.known("pi:22").unwrap().added_at;
        const FP2: &str = "SHA256:CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
        store.trust("pi:22", ALGO, FP2).unwrap();
        assert_eq!(store.known("pi:22").unwrap().added_at, first);
    }
}
