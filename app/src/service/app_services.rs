//! Shared application services: the single value every `service::*` body
//! takes.
//!
//! Built once by the UI at startup (tests construct their own with explicit
//! directories), then cloned (Arc) into `Task::perform` closures and
//! borrowed by service functions running on the global runtime. The
//! registries are plain Arc/Mutex-based types; no global state is required
//! beyond the event bus.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::actions::ActionRegistry;
use crate::opencode::OpenCodeServerRegistry;
use crate::pty::PtyRegistry;
use crate::remote::actions::RemoteActionRegistry;
use crate::remote::hostkeys::HostKeyStore;
use crate::remote::profiles::DeviceStore;
use crate::remote::term::RemoteShellRegistry;
use crate::remote::transport::DeviceRegistry;
use crate::repo::RepoManager;

/// App-data directory suffix, unchanged since the pre-iced app so existing
/// `devices.json`, `known_hosts.json` and state files keep being found.
const APP_DIR: &str = "com.raulshma.mygitui";

pub struct AppServices {
    /// Open repo handles (one per tab), their queues and watchers.
    pub repos: RepoManager,
    /// Live custom-action runs (`action_run` / `action_cancel`).
    pub actions: ActionRegistry,
    /// Embedded terminal sessions.
    pub pty: PtyRegistry,
    /// Managed `opencode serve` child.
    pub opencode: OpenCodeServerRegistry,
    /// SSH device sessions.
    pub devices: DeviceRegistry,
    /// Remote shell sessions.
    pub remote_shells: RemoteShellRegistry,
    /// Remote action runs.
    pub remote_actions: RemoteActionRegistry,
    /// Persistent state (device profiles, host keys, window/UI state).
    pub data_dir: PathBuf,
    /// User-editable configuration (`config.json`).
    pub config_dir: PathBuf,
}

impl AppServices {
    /// Production services rooted at the platform dirs (`dirs` fallbacks:
    /// `.` when the environment provides none, e.g. some CI containers).
    pub fn new() -> Self {
        let root = |dir: Option<PathBuf>| dir.unwrap_or_else(|| PathBuf::from("."));
        let data_dir = root(dirs::data_dir()).join(APP_DIR);
        let config_dir = root(dirs::config_dir()).join(APP_DIR);
        Self::with_dirs(data_dir, config_dir)
    }

    /// Explicit-directory constructor (tests, portable mode).
    pub fn with_dirs(data_dir: impl Into<PathBuf>, config_dir: impl Into<PathBuf>) -> Self {
        Self {
            repos: RepoManager::new(),
            actions: ActionRegistry::default(),
            pty: PtyRegistry::default(),
            opencode: OpenCodeServerRegistry::default(),
            devices: DeviceRegistry::new(),
            remote_shells: RemoteShellRegistry::default(),
            remote_actions: RemoteActionRegistry::default(),
            data_dir: data_dir.into(),
            config_dir: config_dir.into(),
        }
    }

    /// `<data>/remote/devices.json` store.
    pub fn device_store(&self) -> DeviceStore {
        DeviceStore::under_dir(&self.data_dir)
    }

    /// `<data>/remote/known_hosts.json` store.
    pub fn host_key_store(&self) -> Arc<HostKeyStore> {
        Arc::new(HostKeyStore::under_dir(&self.data_dir))
    }

    /// JSON config path (`<config>/config.json`).
    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    /// Convenience for tests: services rooted in one temp dir.
    pub fn in_dir(dir: &Path) -> Self {
        Self::with_dirs(dir, dir)
    }
}

impl Default for AppServices {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The service bag is moved into `Task::perform` closures and shared
    /// across worker threads; pin the auto-trait bounds the UI relies on.
    #[test]
    fn app_services_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AppServices>();
    }

    #[test]
    fn explicit_dirs_drive_store_paths() {
        let dir = std::env::temp_dir().join(format!("mygitui-services-{}", std::process::id()));
        let services = AppServices::in_dir(&dir);
        assert_eq!(services.config_path(), dir.join("config.json"));
        let store = services.device_store();
        assert!(store.list().is_empty());
        let _ = services.host_key_store();
    }
}
