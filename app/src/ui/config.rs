//! Persistent user configuration (`<config>/com.raulshma.mygitui/config.json`).
//!
//! Replaces the old webview's `localStorage` entries. Loading never fails:
//! a missing or corrupt file yields defaults, and unknown keys are ignored so
//! older/newer builds can share the file. Saves are atomic (temp + rename)
//! and surface I/O errors to the UI.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ui::theme;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: theme::Mode,
    pub diff_wrap: bool,
    /// Minutes between background fetches (0 = off).
    pub auto_fetch_minutes: u32,
    pub recent_repos: Vec<RecentRepo>,
}

/// Auto-fetch cadence choices cycled by the header button (minutes; 0 = off).
pub const AUTO_FETCH_CHOICES: [u32; 5] = [0, 1, 5, 10, 30];

impl Config {
    /// Next auto-fetch cadence after `current` in [`AUTO_FETCH_CHOICES`].
    pub fn next_auto_fetch(current: u32) -> u32 {
        let position = AUTO_FETCH_CHOICES
            .iter()
            .position(|choice| *choice == current)
            .map_or(0, |position| (position + 1) % AUTO_FETCH_CHOICES.len());
        AUTO_FETCH_CHOICES[position]
    }

    /// Header label for the auto-fetch cadence.
    pub fn auto_fetch_label(minutes: u32) -> String {
        if minutes == 0 {
            "Auto-fetch: Off".to_string()
        } else if minutes == 1 {
            "Auto-fetch: 1m".to_string()
        } else {
            format!("Auto-fetch: {minutes}m")
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: theme::Mode::System,
            diff_wrap: false,
            auto_fetch_minutes: 0,
            recent_repos: Vec::new(),
        }
    }
}

/// One entry of the repository switcher (old key `mygitui.recent-repos`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentRepo {
    pub path: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub last_opened: u64,
}

impl Config {
    /// Reads `config.json` under `config_dir`, defaulting on any failure.
    pub fn load(config_dir: &Path) -> Self {
        let Ok(raw) = fs::read_to_string(config_dir.join("config.json")) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    /// Writes the config atomically, creating the directory when needed.
    pub fn save(&self, config_dir: &Path) -> Result<(), String> {
        fs::create_dir_all(config_dir).map_err(|err| err.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|err| err.to_string())?;
        let final_path = config_dir.join("config.json");
        let tmp_path = config_dir.join("config.json.tmp");
        fs::write(&tmp_path, json).map_err(|err| err.to_string())?;
        fs::rename(&tmp_path, &final_path).map_err(|err| err.to_string())
    }

    /// Adds or re-touches a repository, deduping separator/case variants.
    pub fn note_recent(&mut self, path: &str, now_ms: u64) {
        if path.is_empty() {
            return;
        }
        let key = canonical_key(path);
        if let Some(existing) = self
            .recent_repos
            .iter_mut()
            .find(|entry| canonical_key(&entry.path) == key)
        {
            existing.last_opened = now_ms;
            existing.path = path.to_string();
            return;
        }
        self.recent_repos.push(RecentRepo {
            path: path.to_string(),
            pinned: false,
            last_opened: now_ms,
        });
    }

    pub fn remove_recent(&mut self, path: &str) {
        let key = canonical_key(path);
        self.recent_repos
            .retain(|entry| canonical_key(&entry.path) != key);
    }

    pub fn toggle_pin(&mut self, path: &str) {
        let key = canonical_key(path);
        if let Some(entry) = self
            .recent_repos
            .iter_mut()
            .find(|entry| canonical_key(&entry.path) == key)
        {
            entry.pinned = !entry.pinned;
        }
    }

    /// Pinned first, then most recently opened (the old store's ordering).
    pub fn recent_sorted(&self) -> Vec<RecentRepo> {
        let mut entries = self.recent_repos.clone();
        entries.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.last_opened.cmp(&a.last_opened))
        });
        entries
    }
}

/// Canonical comparison key: folds `\\?\` prefixes, slash styles and
/// trailing separators; lowercases Windows-shaped paths (case-insensitive
/// filesystems) while POSIX paths keep their case.
pub fn canonical_key(path: &str) -> String {
    let windowsish = path.get(..2).is_some_and(|prefix| {
        prefix.as_bytes()[1] == b':' && prefix.as_bytes()[0].is_ascii_alphabetic()
    }) || path.starts_with("\\\\");
    let stripped = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let stripped = stripped.trim_end_matches(['\\', '/']).replace('\\', "/");
    if windowsish {
        stripped.to_lowercase()
    } else {
        stripped
    }
}

/// Milliseconds since the Unix epoch (persisted `last_opened`).
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-config-{tag}-{}-{}",
            std::process::id(),
            now_millis()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn missing_and_corrupt_config_load_defaults() {
        let dir = temp_dir("defaults");
        assert_eq!(Config::load(&dir).theme, theme::Mode::System);
        std::fs::write(dir.join("config.json"), "{not json").unwrap();
        let loaded = Config::load(&dir);
        assert_eq!(loaded.theme, theme::Mode::System);
        assert!(loaded.recent_repos.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn config_round_trips_through_disk() {
        let dir = temp_dir("roundtrip");
        let mut config = Config {
            theme: theme::Mode::Dark,
            diff_wrap: true,
            auto_fetch_minutes: 5,
            ..Default::default()
        };
        config.note_recent("C:\\repos\\alpha", 7);
        config.save(&dir).expect("save");
        let loaded = Config::load(&dir);
        assert_eq!(loaded.theme, theme::Mode::Dark);
        assert!(loaded.diff_wrap);
        assert_eq!(loaded.auto_fetch_minutes, 5);
        assert_eq!(loaded.recent_repos.len(), 1);
        assert_eq!(loaded.recent_repos[0].path, "C:\\repos\\alpha");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn auto_fetch_cadence_cycles_and_labels() {
        let mut minutes = 0;
        for expected in [1, 5, 10, 30, 0] {
            minutes = Config::next_auto_fetch(minutes);
            assert_eq!(minutes, expected);
        }
        assert_eq!(Config::auto_fetch_label(0), "Auto-fetch: Off");
        assert_eq!(Config::auto_fetch_label(1), "Auto-fetch: 1m");
        assert_eq!(Config::auto_fetch_label(30), "Auto-fetch: 30m");
    }

    #[test]
    fn missing_auto_fetch_setting_defaults_to_off() {
        let dir = temp_dir("autofetch-default");
        std::fs::write(
            dir.join("config.json"),
            r#"{"theme": "Light", "recent_repos": []}"#,
        )
        .unwrap();
        let loaded = Config::load(&dir);
        assert_eq!(loaded.auto_fetch_minutes, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recent_repos_dedupe_canonically_and_keep_newest_spelling() {
        let mut config = Config::default();
        config.note_recent("C:\\repos\\alpha", 1);
        config.note_recent("c:/repos/alpha/", 2);
        assert_eq!(config.recent_repos.len(), 1);
        assert_eq!(config.recent_repos[0].path, "c:/repos/alpha/");
        assert_eq!(config.recent_repos[0].last_opened, 2);
    }

    #[test]
    fn posix_paths_keep_case_but_windows_paths_do_not() {
        assert_eq!(canonical_key("/home/me/Repo/"), "/home/me/Repo");
        assert_eq!(canonical_key("C:\\Repos\\Alpha\\"), "c:/repos/alpha");
        assert_eq!(canonical_key("\\\\?\\C:\\Repos\\Alpha"), "c:/repos/alpha");
    }

    #[test]
    fn sorted_recent_pins_first_then_recency() {
        let mut config = Config::default();
        config.note_recent("/a", 1);
        config.note_recent("/b", 3);
        config.note_recent("/c", 2);
        config.toggle_pin("/a");
        let order: Vec<_> = config
            .recent_sorted()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(order, vec!["/a", "/b", "/c"]);
    }
}
