//! Persistent user configuration (`<config>/com.raulshma.mygitui/config.json`).
//!
//! Replaces the old webview's `localStorage` entries. Loading never fails:
//! a missing or corrupt file yields defaults, and unknown keys are ignored so
//! older/newer builds can share the file. Saves are atomic (temp + rename)
//! and surface I/O errors to the UI.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ui::theme;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: theme::Mode,
    pub diff_wrap: bool,
    /// Side-by-side diff layout (`false` = unified); seeds new tabs and is
    /// re-persisted whenever the diff toolbar toggles it.
    pub diff_split: bool,
    /// Minutes between background fetches (0 = off).
    pub auto_fetch_minutes: u32,
    pub recent_repos: Vec<RecentRepo>,
    /// Tint controls with the OS accent colour when the OS provides one.
    pub accent: bool,
    /// Syntax-highlight diff rows (syntect; off renders plain monochrome).
    pub highlight: bool,
    /// Query GitHub for a newer release at startup (never auto-installs;
    /// installing is an explicit palette action).
    pub update_check_on_start: bool,
    /// AI assist settings (default off; secrets live in the OS keyring,
    /// never in this file).
    pub ai: AiSettings,
    /// Open repo tabs persisted across runs (local repos only; see
    /// [`SessionState`]).
    pub session: SessionState,
    /// Main-window geometry persisted across runs ([`WindowFrame`]).
    pub window: WindowFrame,
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
            diff_split: false,
            auto_fetch_minutes: 0,
            recent_repos: Vec::new(),
            accent: true,
            highlight: true,
            update_check_on_start: true,
            ai: AiSettings::default(),
            session: SessionState::default(),
            window: WindowFrame::default(),
        }
    }
}

/// Open repo tabs persisted across runs (`config.json` `session` section):
/// ordered repo root paths plus the tab index to focus after restore.
/// Device-backed repos are never stored here (reopening them needs the SSH
/// device flow, not a plain path open); paths that no longer exist or fail
/// to open are silently dropped at boot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionState {
    /// Repo root paths in tab order (as resolved by the last open).
    pub tabs: Vec<String>,
    /// Tab to activate after restore (clamped to the restored tabs).
    pub active: usize,
}

/// Builds the persisted session from the open tabs: `(root, is_local)`
/// pairs in tab order plus the focused index. Device-backed tabs are
/// skipped; the focused index is re-based onto the filtered list (falling
/// back to the previously stored index when it points at a skipped tab,
/// clamped), and duplicate roots keep their first occurrence.
pub fn session_state(
    tabs: &[(&str, bool)],
    focused: Option<usize>,
    previous_active: usize,
) -> SessionState {
    let mut tabs_out: Vec<String> = Vec::new();
    for (root, local) in tabs {
        if *local && !tabs_out.iter().any(|seen| seen == root) {
            tabs_out.push((*root).to_string());
        }
    }
    let focused_root = focused.and_then(|index| {
        tabs.get(index)
            .copied()
            .filter(|(_, local)| *local)
            .map(|(root, _)| root)
    });
    let active = focused_root
        .and_then(|root| tabs_out.iter().position(|seen| seen == root))
        .unwrap_or_else(|| previous_active.min(tabs_out.len().saturating_sub(1)));
    SessionState {
        tabs: tabs_out,
        active,
    }
}

/// Main-window geometry persisted across runs (`config.json` `window`
/// section). `position` stays `None` on platforms that do not report
/// reliable global coordinates (Wayland never emits move events) — restore
/// then uses the platform default, so this degrades to size + maximized.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowFrame {
    /// Inner window size in logical pixels `[width, height]`.
    pub size: [f32; 2],
    /// Outer window position in logical pixels `[x, y]`, when known.
    pub position: Option<[f32; 2]>,
    /// Whether the window was maximized at the last observation.
    pub maximized: bool,
}

impl Default for WindowFrame {
    /// First-run fallback geometry (the historical fixed window size).
    fn default() -> Self {
        Self {
            size: [1152.0, 720.0],
            position: None,
            maximized: false,
        }
    }
}

/// Trailing-debounce window for persisting [`WindowFrame`]: moves and
/// resizes arrive in bursts, so every observed change bumps a generation
/// counter and arms a timer; only a timer whose generation is still the
/// newest persists the frame (a drag-resize storm collapses into one
/// write).
pub const WINDOW_SAVE_DEBOUNCE: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, PartialEq)]
pub struct WindowSave {
    /// Latest observed geometry.
    pub frame: WindowFrame,
    /// Bumped by every observed change.
    pub seq: u64,
    /// Generation last persisted.
    pub saved_seq: u64,
}

impl WindowSave {
    /// Starts from the geometry the window was opened with.
    pub fn new(frame: WindowFrame) -> Self {
        Self {
            frame,
            seq: 0,
            saved_seq: 0,
        }
    }

    /// Records an observed geometry change; `true` when the frame actually
    /// changed and a save timer must be (re)armed with the new generation
    /// (`seq`).
    pub fn note_change(&mut self, frame: WindowFrame) -> bool {
        if frame == self.frame {
            return false;
        }
        self.frame = frame;
        self.seq += 1;
        true
    }

    /// Whether the timer that fired carries the newest generation and has
    /// not been persisted yet; stale ticks (superseded by a newer change)
    /// are dropped.
    pub fn due(&self, seq: u64) -> bool {
        seq == self.seq && seq > self.saved_seq
    }

    /// Marks the generation the caller persisted.
    pub fn mark_saved(&mut self, seq: u64) {
        self.saved_seq = seq;
    }
}

/// Which AI backend serves generation requests (`ai::AiProvider` on the
/// wire-level; duplicated here because config must not depend on the
/// provider layer).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProviderChoice {
    /// A local opencode server (managed by the app's serve registry, or an
    /// attached user-run server when `opencode_url` is set).
    #[default]
    Opencode,
    /// Direct OpenRouter API access with the user's key.
    OpenRouter,
}

/// AI assist settings (`config.json` `ai` section). Everything defaults so
/// pre-AI config files keep loading; the master switch is OFF and each
/// repository opts in separately — AI stays dark until the user allows it.
/// Secrets (OpenRouter API key, opencode server password) are NEVER stored
/// here — they live in the OS keyring.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AiSettings {
    /// Master switch: AI features stay off until enabled.
    pub enabled: bool,
    /// Preferred backend for AI requests.
    pub provider: AiProviderChoice,
    /// Attach mode: manual opencode server base URL. Unset = managed (the
    /// app spawns/owns the server through its serve registry).
    pub opencode_url: Option<String>,
    /// opencode model id (`provider/model`); unset uses the server default.
    pub opencode_model: Option<String>,
    /// OpenRouter model slug; backend default when unset.
    pub openrouter_model: Option<String>,
    /// Per-repository opt-in keyed by canonical repo path: AI features stay
    /// off per repo until the user allows them (`true` entries only matter;
    /// absent/`false` = off).
    pub repo_opt_in: BTreeMap<String, bool>,
}

impl AiSettings {
    /// Whether AI may run for the repo at `path`: master switch AND
    /// per-repo opt-in (keys are [`canonical_key`] spellings).
    // Backend lands with the AI settings panel; service/ai.rs mirrors the
    // read side until the panel wires this (see its module docs).
    #[allow(dead_code)]
    pub fn enabled_for(&self, path: &str) -> bool {
        self.enabled
            && self
                .repo_opt_in
                .get(&canonical_key(path))
                .copied()
                .unwrap_or(false)
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

    /// Whether AI features may run for the repo at `path` (master switch
    /// plus per-repo opt-in; see [`AiSettings::enabled_for`]).
    // Backend lands with the AI settings panel.
    #[allow(dead_code)]
    pub fn ai_enabled_for(&self, path: &str) -> bool {
        self.ai.enabled_for(path)
    }

    /// Sets per-repo AI opt-in for the repo at `path`.
    // Backend lands with the AI settings panel.
    #[allow(dead_code)]
    pub fn set_ai_opt_in(&mut self, path: &str, enabled: bool) {
        let key = canonical_key(path);
        if enabled {
            self.ai.repo_opt_in.insert(key, true);
        } else {
            self.ai.repo_opt_in.remove(&key);
        }
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
            diff_split: true,
            auto_fetch_minutes: 5,
            accent: false,
            highlight: false,
            ..Default::default()
        };
        config.note_recent("C:\\repos\\alpha", 7);
        config.save(&dir).expect("save");
        let loaded = Config::load(&dir);
        assert_eq!(loaded.theme, theme::Mode::Dark);
        assert!(loaded.diff_wrap);
        assert!(loaded.diff_split);
        assert_eq!(loaded.auto_fetch_minutes, 5);
        assert!(!loaded.accent);
        assert!(!loaded.highlight);
        assert_eq!(loaded.recent_repos.len(), 1);
        assert_eq!(loaded.recent_repos[0].path, "C:\\repos\\alpha");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn diff_prefs_default_for_older_config_files() {
        // Pre-diff-split config files keep loading with both prefs off.
        let dir = temp_dir("diff-prefs-default");
        std::fs::write(
            dir.join("config.json"),
            r#"{"theme": "Dark", "recent_repos": []}"#,
        )
        .unwrap();
        let loaded = Config::load(&dir);
        assert!(!loaded.diff_split);
        assert!(!loaded.diff_wrap);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn accent_and_highlight_default_on_for_older_config_files() {
        // Pre-accent/highlight config files keep loading with both on.
        let dir = temp_dir("accent-highlight-default");
        std::fs::write(
            dir.join("config.json"),
            r#"{"theme": "Dark", "recent_repos": []}"#,
        )
        .unwrap();
        let loaded = Config::load(&dir);
        assert!(loaded.accent);
        assert!(loaded.highlight);
        assert!(Config::default().accent);
        assert!(Config::default().highlight);
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

    #[test]
    fn ai_defaults_to_off_and_old_configs_load_without_the_section() {
        // Missing section (pre-AI config file): everything defaults, master
        // switch off, no repo opted in.
        let dir = temp_dir("ai-default");
        std::fs::write(
            dir.join("config.json"),
            r#"{"theme": "Dark", "recent_repos": []}"#,
        )
        .unwrap();
        let loaded = Config::load(&dir);
        assert_eq!(loaded.ai, AiSettings::default());
        assert!(!loaded.ai_enabled_for("/any/repo"));
        assert_eq!(loaded.ai.provider, AiProviderChoice::Opencode);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ai_opt_in_needs_master_switch_and_is_canonical_per_repo() {
        let mut config = Config::default();
        config.ai.enabled = true;
        config.set_ai_opt_in("/repos/Demo/", true);
        // Canonical key: separator/trailing-slash variants resolve to it.
        assert!(config.ai_enabled_for("/repos/Demo"));
        assert!(config.ai_enabled_for("/repos/Demo/"));
        assert!(!config.ai_enabled_for("/repos/other"));
        // Windows-shaped paths canonicalize the same way they do for
        // recent repos (drive-letter casing + separators fold).
        config.set_ai_opt_in(r"C:\Repos\Alpha\", true);
        assert!(config.ai_enabled_for(r"C:\Repos\Alpha"));
        assert!(config.ai_enabled_for("c:/repos/alpha"));
        // Unsetting removes the entry instead of storing `false`.
        config.set_ai_opt_in("/repos/demo", false);
        assert!(!config.ai_enabled_for("/repos/demo"));
        assert!(!config.ai.repo_opt_in.contains_key("/repos/demo"));
        // Master switch off: even opted-in repos stay dark.
        config.set_ai_opt_in("/repos/demo", true);
        config.ai.enabled = false;
        assert!(!config.ai_enabled_for("/repos/demo"));
    }

    #[test]
    fn ai_settings_round_trip_through_disk() {
        let dir = temp_dir("ai-roundtrip");
        let mut config = Config::default();
        config.ai.enabled = true;
        config.ai.provider = AiProviderChoice::OpenRouter;
        config.ai.opencode_url = Some("http://127.0.0.1:4096".into());
        config.ai.opencode_model = Some("anthropic/claude-sonnet".into());
        config.ai.openrouter_model = Some("vendor/model".into());
        config.set_ai_opt_in("/repos/alpha", true);
        config.save(&dir).expect("save");
        let loaded = Config::load(&dir);
        assert_eq!(loaded.ai, config.ai);
        assert!(loaded.ai_enabled_for("/repos/alpha"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_defaults_for_older_config_files() {
        // Pre-session config files keep loading with an empty session.
        let dir = temp_dir("session-default");
        std::fs::write(
            dir.join("config.json"),
            r#"{"theme": "Dark", "recent_repos": []}"#,
        )
        .unwrap();
        let loaded = Config::load(&dir);
        assert_eq!(loaded.session, SessionState::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_round_trips_through_disk() {
        let dir = temp_dir("session-roundtrip");
        let config = Config {
            session: SessionState {
                tabs: vec!["/repos/alpha".into(), "/repos/beta".into()],
                active: 1,
            },
            ..Config::default()
        };
        config.save(&dir).expect("save");
        let loaded = Config::load(&dir);
        assert_eq!(loaded.session.tabs, vec!["/repos/alpha", "/repos/beta"]);
        assert_eq!(loaded.session.active, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_state_skips_device_tabs_and_keeps_order() {
        let tabs = [("/a", true), ("/remote", false), ("/b", true), ("/a", true)];
        let session = session_state(&tabs, Some(2), 0);
        assert_eq!(
            session.tabs,
            vec!["/a", "/b"],
            "device tabs dropped, dupes folded"
        );
        // Focused local tab re-bases onto the filtered list.
        assert_eq!(session.active, 1);
    }

    #[test]
    fn session_state_falls_back_when_focus_is_a_device_tab() {
        let tabs = [("/a", true), ("/remote", false)];
        let session = session_state(&tabs, Some(1), 0);
        assert_eq!(session.tabs, vec!["/a"]);
        // The focused tab is device-backed: keep the previous index, clamped.
        assert_eq!(session.active, 0);
        let session = session_state(&tabs, Some(1), 7);
        assert_eq!(session.active, 0, "previous index clamps to the last tab");
    }

    #[test]
    fn session_state_empty_tabs_collapse_to_default() {
        let session = session_state(&[("/remote", false)], None, 3);
        assert_eq!(session, SessionState::default());
        assert_eq!(session_state(&[], None, 2), SessionState::default());
    }

    #[test]
    fn window_defaults_are_the_first_run_fallbacks() {
        let frame = WindowFrame::default();
        assert_eq!(frame.size, [1152.0, 720.0]);
        assert_eq!(frame.position, None);
        assert!(!frame.maximized);
    }

    #[test]
    fn window_frame_round_trips_through_disk_and_defaults_for_older_files() {
        let dir = temp_dir("window-roundtrip");
        let config = Config {
            window: WindowFrame {
                size: [1440.0, 900.0],
                position: Some([64.0, 48.0]),
                maximized: true,
            },
            ..Config::default()
        };
        config.save(&dir).expect("save");
        let loaded = Config::load(&dir);
        assert_eq!(loaded.window, config.window);

        // Pre-window config files keep loading with the fallback geometry.
        std::fs::write(
            dir.join("config.json"),
            r#"{"theme": "Dark", "recent_repos": []}"#,
        )
        .unwrap();
        assert_eq!(Config::load(&dir).window, WindowFrame::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn window_save_debounce_collapses_bursts_and_drops_stale_ticks() {
        let start = WindowFrame::default();
        let mut save = WindowSave::new(start);
        assert!(!save.due(0), "nothing persisted yet");

        // Identical geometry is not a change.
        assert!(!save.note_change(start));

        // A burst of changes: every one bumps the generation.
        let mut frame = start;
        frame.size = [1000.0, 700.0];
        assert!(save.note_change(frame));
        assert_eq!(save.seq, 1);
        frame.position = Some([10.0, 20.0]);
        assert!(save.note_change(frame));
        assert_eq!(save.seq, 2);

        // The first timer's tick is stale and dropped.
        assert!(!save.due(1));
        // The newest tick persists exactly once.
        assert!(save.due(2));
        save.mark_saved(2);
        assert!(!save.due(2), "a generation is persisted at most once");

        // A later change re-arms with a fresh generation.
        frame.position = Some([30.0, 40.0]);
        assert!(save.note_change(frame));
        assert_eq!(save.seq, 3);
        assert!(save.due(3));
    }
}
