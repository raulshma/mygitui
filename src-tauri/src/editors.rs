//! External-app detection + launch for the repo header's "Open with…"
//! button.
//!
//! [`detect_apps`] probes PATH and well-known per-OS install locations for
//! common editors (VS Code family, Cursor, Windsurf, Zed, Sublime,
//! Notepad++, JetBrains IDEs via Toolbox/standalone installs); [`open_with`]
//! spawns a *detected* path — never a frontend-supplied one — with the
//! repository root. Detection is pure over an injectable [`Roots`] snapshot
//! so tests can fake install trees without touching the environment.
//!
//! Spawns use the sanitized environment ([`crate::cli::apply_sanitized_env`])
//! like every other child: the allowlist retains the entries GUI apps need
//! (SYSTEMROOT/USERPROFILE/APPDATA/LOCALAPPDATA/PATH/HOME/TMP…), so editors
//! locate their config while git credentials/hooks env stays out. On top of
//! that they overlay the resolved login-shell environment
//! ([`crate::shell_env`]) so shell-profile entries (nvm & co) a GUI launch
//! would miss reach the editor too.
//!
//! User-defined custom commands ([`open_with_custom`]) are the deliberate
//! exception to the machine-verified-path rule: the frontend sends the full
//! command line per launch (nothing is persisted backend-side), and it is
//! spawned through the same platform shell wrapping as the custom shell
//! actions ([`crate::actions::build_shell_command`]).

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// Install-root snapshot detection reads (the only env it may touch).
struct Roots {
    local_app_data: Option<PathBuf>,
    program_files: Option<PathBuf>,
    program_files_x86: Option<PathBuf>,
    home: Option<PathBuf>,
    path_dirs: Vec<PathBuf>,
}

fn roots_from_env() -> Roots {
    let env = |key: &str| -> Option<OsString> { std::env::var_os(key) };
    Roots {
        local_app_data: env("LOCALAPPDATA").map(PathBuf::from),
        program_files: env("ProgramFiles").map(PathBuf::from),
        program_files_x86: env("ProgramFiles(x86)").map(PathBuf::from),
        home: env("HOME")
            .or_else(|| env("USERPROFILE"))
            .map(PathBuf::from),
        path_dirs: env("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default(),
    }
}

/// One detectable editor: display metadata + per-OS probe locations.
struct EditorDef {
    id: &'static str,
    name: &'static str,
    /// Windows: paths relative to `%LOCALAPPDATA%\Programs` and
    /// `%ProgramFiles%` (both checked).
    win_rel: &'static [&'static str],
    /// macOS: bundle names under `/Applications` and `~/Applications`.
    mac_app: &'static [&'static str],
    /// Executable names scanned on PATH (`.exe` suffixed on Windows; the
    /// `.cmd`/`.bat` shims common on Windows PATH never match).
    path_exe: &'static [&'static str],
}

const EDITORS: &[EditorDef] = &[
    EditorDef {
        id: "vscode",
        name: "VS Code",
        win_rel: &["Microsoft VS Code/Code.exe"],
        mac_app: &["Visual Studio Code.app"],
        path_exe: &["code"],
    },
    EditorDef {
        id: "vscode-insiders",
        name: "VS Code Insiders",
        win_rel: &["Microsoft VS Code Insiders/Code - Insiders.exe"],
        mac_app: &["Visual Studio Code - Insiders.app"],
        path_exe: &["code-insiders"],
    },
    EditorDef {
        id: "vscodium",
        name: "VSCodium",
        win_rel: &["VSCodium/VSCodium.exe"],
        mac_app: &["VSCodium.app"],
        path_exe: &["codium"],
    },
    EditorDef {
        id: "cursor",
        name: "Cursor",
        win_rel: &["cursor/Cursor.exe"],
        mac_app: &["Cursor.app"],
        path_exe: &["cursor"],
    },
    EditorDef {
        id: "windsurf",
        name: "Windsurf",
        win_rel: &["Windsurf/Windsurf.exe"],
        mac_app: &["Windsurf.app"],
        path_exe: &["windsurf"],
    },
    EditorDef {
        id: "zed",
        name: "Zed",
        win_rel: &["Zed/Zed.exe", "Zed/zed.exe"],
        mac_app: &["Zed.app"],
        path_exe: &["zed"],
    },
    EditorDef {
        id: "sublime",
        name: "Sublime Text",
        win_rel: &["Sublime Text/sublime_text.exe"],
        mac_app: &["Sublime Text.app"],
        path_exe: &["subl"],
    },
    EditorDef {
        id: "notepadpp",
        name: "Notepad++",
        win_rel: &["Notepad++/notepad++.exe"],
        mac_app: &[],
        path_exe: &[],
    },
];

/// One JetBrains IDE. Fixed-location probes miss Toolbox layouts, so these
/// are found by scanning the Toolbox `apps` tree + `JetBrains/*` install
/// dirs for the per-IDE launcher exe (windows) or the standard app bundles /
/// PATH launcher scripts (mac/linux).
struct JetBrainsDef {
    id: &'static str,
    name: &'static str,
    /// Windows: launcher exe inside the IDE's `bin\` dir. (Windows-only read.)
    #[cfg_attr(not(windows), allow(dead_code))]
    win_launcher: &'static str,
    /// macOS: bundle names under /Applications, ~/Applications. (mac-only.)
    #[cfg_attr(windows, allow(dead_code))]
    mac_apps: &'static [&'static str],
    /// Launcher script names scanned on PATH (mac/linux). (Not read on Windows.)
    #[cfg_attr(windows, allow(dead_code))]
    path_exes: &'static [&'static str],
}

const JETBRAINS: &[JetBrainsDef] = &[
    JetBrainsDef {
        id: "idea",
        name: "IntelliJ IDEA",
        win_launcher: "idea64.exe",
        mac_apps: &[
            "IntelliJ IDEA Ultimate.app",
            "IntelliJ IDEA CE.app",
            "IntelliJ IDEA Community Edition.app",
        ],
        path_exes: &["jetbrains-idea", "idea"],
    },
    JetBrainsDef {
        id: "webstorm",
        name: "WebStorm",
        win_launcher: "webstorm64.exe",
        mac_apps: &["WebStorm.app"],
        path_exes: &["jetbrains-webstorm", "webstorm"],
    },
    JetBrainsDef {
        id: "pycharm",
        name: "PyCharm",
        win_launcher: "pycharm64.exe",
        mac_apps: &["PyCharm Professional.app", "PyCharm Community Edition.app"],
        path_exes: &["jetbrains-pycharm", "pycharm"],
    },
    JetBrainsDef {
        id: "goland",
        name: "GoLand",
        win_launcher: "goland64.exe",
        mac_apps: &["GoLand.app"],
        path_exes: &["jetbrains-goland", "goland"],
    },
    JetBrainsDef {
        id: "rider",
        name: "Rider",
        win_launcher: "rider64.exe",
        mac_apps: &["Rider.app"],
        path_exes: &["jetbrains-rider", "rider"],
    },
    JetBrainsDef {
        id: "clion",
        name: "CLion",
        win_launcher: "clion64.exe",
        mac_apps: &["CLion.app"],
        path_exes: &["jetbrains-clion", "clion"],
    },
    JetBrainsDef {
        id: "phpstorm",
        name: "PhpStorm",
        win_launcher: "phpstorm64.exe",
        mac_apps: &["PhpStorm.app"],
        path_exes: &["jetbrains-phpstorm", "phpstorm"],
    },
    JetBrainsDef {
        id: "rubymine",
        name: "RubyMine",
        win_launcher: "rubymine64.exe",
        mac_apps: &["RubyMine.app"],
        path_exes: &["jetbrains-rubymine", "rubymine"],
    },
    JetBrainsDef {
        id: "datagrip",
        name: "DataGrip",
        win_launcher: "datagrip64.exe",
        mac_apps: &["DataGrip.app"],
        path_exes: &["jetbrains-datagrip", "datagrip"],
    },
    JetBrainsDef {
        id: "android-studio",
        name: "Android Studio",
        win_launcher: "studio64.exe",
        mac_apps: &["Android Studio.app"],
        path_exes: &["jetbrains-studio", "studio"],
    },
];

/// The platform file-manager entry (always present, first in the list).
fn explorer_entry() -> AppEntry {
    let (name, path) = if cfg!(target_os = "macos") {
        ("Finder", "open")
    } else if cfg!(windows) {
        ("Explorer", "explorer")
    } else {
        ("Files", "xdg-open")
    };
    AppEntry {
        id: "explorer".into(),
        name: name.into(),
        path: path.into(),
    }
}

/// Scans PATH dirs for an executable file name (`.exe` suffixed on Windows;
/// the PATH×PATHEXT dance of opencode.rs is unnecessary because every name
/// probed here is a real `.exe` on Windows and extensionless elsewhere).
fn find_on_path(roots: &Roots, exe: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{exe}.exe")
    } else {
        exe.to_string()
    };
    roots
        .path_dirs
        .iter()
        .map(|dir| dir.join(&file))
        .find(|p| p.is_file())
}

/// First existing entry: bundles (dirs) win over executables (files), so
/// macOS app bundles and PATH shims can share one probe list.
fn pick_existing(probes: &[PathBuf]) -> Option<PathBuf> {
    probes
        .iter()
        .find(|p| p.is_dir())
        .or_else(|| probes.iter().find(|p| p.is_file()))
        .cloned()
}

/// macOS probe roots for one app-bundle name (`/Applications` + user-local).
fn push_mac_bundle(app: &str, roots: &Roots, probes: &mut Vec<PathBuf>) {
    probes.push(PathBuf::from("/Applications").join(app));
    if let Some(home) = roots.home.as_deref() {
        probes.push(home.join("Applications").join(app));
    }
}

/// Windows probe roots for one install-relative path (`Programs` under
/// LOCALAPPDATA + both ProgramFiles variants).
fn push_win_install(win_rel: &str, roots: &Roots, probes: &mut Vec<PathBuf>) {
    for root in [
        roots.local_app_data.as_deref().map(|r| r.join("Programs")),
        roots.program_files.as_deref().map(PathBuf::from),
        roots.program_files_x86.as_deref().map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    {
        probes.push(root.join(win_rel.replace('/', "\\")));
    }
}

fn find_editor(def: &EditorDef, roots: &Roots) -> Option<PathBuf> {
    let mut probes: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for rel in def.win_rel {
            push_win_install(rel, roots, &mut probes);
        }
    }
    if cfg!(target_os = "macos") {
        for app in def.mac_app {
            push_mac_bundle(app, roots, &mut probes);
        }
    }
    // PATH probing everywhere (windows too: PATH shims like `code` are
    // .cmd/.bat and never match the `.exe` names probed here; real exes
    // on PATH — zed, subl via chocolatey — are spawnable directly).
    for exe in def.path_exe {
        if let Some(p) = find_on_path(roots, exe) {
            probes.push(p);
        }
    }
    pick_existing(&probes)
}

/// Toolbox nests apps as `apps/<ide>/bin/<launcher>.exe`, older layouts as
/// `apps/<ide>/ch-0/<version>/bin/<launcher>.exe`; standalone installs live
/// under `%ProgramFiles%\JetBrains\<IDE>\bin\<launcher>.exe`.
#[cfg(windows)]
fn find_jetbrains_windows(roots: &Roots) -> HashMap<&'static str, PathBuf> {
    let mut found = HashMap::new();
    let mut scan_dir = |dir: &Path| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for child in entries.flatten() {
            let child = child.path();
            // bin at the child, one level down (ch-0/<ver>), or two
            // (future-proofing; the walk is bounded and cheap).
            let mut bins = vec![child.join("bin")];
            if let Ok(subs) = std::fs::read_dir(&child) {
                for sub in subs.flatten() {
                    bins.push(sub.path().join("bin"));
                    if let Ok(subsubs) = std::fs::read_dir(sub.path()) {
                        for subsub in subsubs.flatten() {
                            bins.push(subsub.path().join("bin"));
                        }
                    }
                }
            }
            for bin in bins {
                for def in JETBRAINS {
                    let exe = bin.join(def.win_launcher);
                    if exe.is_file() {
                        found.entry(def.id).or_insert(exe);
                    }
                }
            }
        }
    };
    if let Some(toolbox) = roots
        .local_app_data
        .as_deref()
        .map(|l| l.join("JetBrains/Toolbox/apps"))
    {
        scan_dir(&toolbox);
    }
    for pf in [
        roots.program_files.as_deref(),
        roots.program_files_x86.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        scan_dir(&pf.join("JetBrains"));
    }
    found
}

fn find_jetbrains(roots: &Roots) -> Vec<(&'static str, &'static str, PathBuf)> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        let launcher_paths = find_jetbrains_windows(roots);
        for def in JETBRAINS {
            if let Some(path) = launcher_paths.get(def.id) {
                out.push((def.id, def.name, path.clone()));
            }
        }
    }
    #[cfg(not(windows))]
    {
        for def in JETBRAINS {
            let mut probes: Vec<PathBuf> = Vec::new();
            if cfg!(target_os = "macos") {
                for app in def.mac_apps {
                    push_mac_bundle(app, roots, &mut probes);
                }
            }
            for exe in def.path_exes {
                if let Some(p) = find_on_path(roots, exe) {
                    probes.push(p);
                }
            }
            if let Some(path) = pick_existing(&probes) {
                out.push((def.id, def.name, path));
            }
        }
    }
    out
}

/// Probes the machine for all supported editors. Cheap (a handful of
/// `stat`s); no subprocesses. Explorer/Finder is always first.
pub fn detect_apps() -> Vec<AppEntry> {
    detect_apps_with_roots(&roots_from_env())
}

fn detect_apps_with_roots(roots: &Roots) -> Vec<AppEntry> {
    let mut out = vec![explorer_entry()];
    for def in EDITORS {
        if let Some(path) = find_editor(def, roots) {
            out.push(AppEntry {
                id: def.id.into(),
                name: def.name.into(),
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    for (id, name, path) in find_jetbrains(roots) {
        out.push(AppEntry {
            id: id.into(),
            name: name.into(),
            path: path.to_string_lossy().into_owned(),
        });
    }
    out
}

/// GUI launchers want native separators: libgit2 hands out forward-slash
/// workdirs (`C:/repo`), and `explorer.exe` cannot parse those — it silently
/// falls back to the default folder (Documents) instead of opening the repo.
/// Pure string conversion: canonicalize() would add a `\\?\` verbatim prefix
/// that some launchers choke on.
fn to_native_path(path: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.as_os_str().to_string_lossy().replace('/', "\\"))
    } else {
        path.to_path_buf()
    }
}

// -- remote devices ------------------------------------------------------------

/// Editor ids whose CLI opens a folder living on an SSH device through their
/// Remote-SSH integration (`--remote ssh-remote+<host> <path>`); the VS Code
/// forks (VSCodium, Cursor, Windsurf) share the flag.
const REMOTE_SSH_EDITORS: [&str; 5] = [
    "vscode",
    "vscode-insiders",
    "vscodium",
    "cursor",
    "windsurf",
];

/// The Remote-SSH host label of one device profile: `[user@]host`, plus
/// `:port` only when the port is not 22 (this label forms
/// `ssh-remote+<label>`; the Remote-SSH extension parses `[user@]host[:port]`
/// and dials `ssh [user@]host -p port`).
fn ssh_host_label(username: &str, host: &str, port: u16) -> String {
    let host = host.trim();
    let base = match username.trim() {
        "" => host.to_string(),
        user => format!("{user}@{host}"),
    };
    if port == 0 || port == 22 {
        base
    } else {
        format!("{base}:{port}")
    }
}

/// argv that opens a device-side folder in one Remote-SSH editor (pure).
/// The path is the DEVICE-side workdir; the editor dials the device itself.
fn remote_launch_args(root: &str, host_label: &str) -> Vec<String> {
    vec![
        "--remote".to_string(),
        format!("ssh-remote+{host_label}"),
        root.to_string(),
    ]
}

/// Whether this editor id can open folders on an SSH device (pure).
fn supports_remote_ssh(id: &str) -> bool {
    REMOTE_SSH_EDITORS.contains(&id)
}

/// Spawns a detected editor with a folder on a device over SSH. Only the
/// VS Code family supports remote folders; every other detected app gets a
/// named error (the FE toasts it) instead of a silently broken local launch
/// against a path that does not exist on this machine.
fn open_remote_with(
    app: &AppEntry,
    device_name: &str,
    root: &str,
    host_label: &str,
) -> Result<(), String> {
    if !supports_remote_ssh(&app.id) {
        return Err(format!(
            "{} cannot open repositories on the device \"{}\" — opening a remote folder needs an editor with SSH remote support (VS Code, VSCodium, Cursor, or Windsurf)",
            app.name, device_name
        ));
    }
    let mut cmd = crate::process::command(&app.path);
    for arg in remote_launch_args(root, host_label) {
        cmd.arg(arg);
    }
    // Same environment as the local launch (sanitized + login-shell overlay).
    crate::cli::apply_sanitized_env(&mut cmd);
    crate::shell_env::apply_resolved(&mut cmd);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to launch {}: {e}", app.name))
}

/// Spawns `app` with the repository root. The app entry must come from
/// [`detect_apps`] (the Tauri command re-detects to enforce that), so the
/// spawned path is machine-verified, not frontend input.
pub fn open_path_with(app: &AppEntry, root: &Path) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") {
        // `open [-a <app>] <dir>`: Finder needs no -a.
        let mut c = crate::process::command("open");
        if app.id != "explorer" {
            c.arg("-a").arg(&app.path);
        }
        c
    } else if app.id == "explorer" {
        crate::process::command(if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open"
        })
    } else if app.id == "notepadpp" {
        // Folder-as-workspace flag; a bare dir arg opens the file dialog.
        let mut c = crate::process::command(&app.path);
        c.arg("-openFoldersAsWorkspace");
        c
    } else {
        crate::process::command(&app.path)
    };
    cmd.arg(to_native_path(root));
    // Same sanitized environment as every other child; the allowlist keeps
    // the entries GUI apps need (see the module docblock). The login-shell
    // overlay runs after the sanitize (which env_clear()s) so editors also
    // see shell-profile entries (nvm & co) a GUI launch would miss.
    crate::cli::apply_sanitized_env(&mut cmd);
    crate::shell_env::apply_resolved(&mut cmd);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to launch {}: {e}", app.name))
}

// -- Tauri commands -----------------------------------------------------------

/// Editors available on this machine (Explorer/Finder always first).
#[tauri::command(rename_all = "snake_case")]
pub async fn detect_editors() -> Result<Vec<AppEntry>, String> {
    tauri::async_runtime::spawn_blocking(detect_apps)
        .await
        .map_err(|e| format!("editor detection failed: {e}"))
}

/// Opens the repository root in the app with the given id. The id must
/// resolve through a fresh detection (so only machine-verified paths spawn).
/// `device_id` selects a remote-device repo: the path is device-side and the
/// launch goes through the editor's Remote-SSH integration (VS Code family
/// only; the device profile supplies the `[user@]host[:port]` label).
#[tauri::command(rename_all = "snake_case")]
pub async fn open_with(
    repo_root: String,
    app_id: String,
    device_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let app_entry = detect_apps()
            .into_iter()
            .find(|a| a.id == app_id)
            .ok_or_else(|| format!("app {app_id:?} is not available"))?;
        match device_id {
            None => open_path_with(&app_entry, Path::new(&repo_root)),
            Some(device_id) => {
                let profile = crate::remote::commands::device_store(&app)
                    .get(&device_id)
                    .ok_or_else(|| format!("unknown device: {device_id}"))?;
                let host_label = ssh_host_label(&profile.username, &profile.host, profile.port);
                open_remote_with(&app_entry, &profile.name, &repo_root, &host_label)
            }
        }
    })
    .await
    .map_err(|e| format!("open_with failed: {e}"))?
}

// -- user-defined custom commands ----------------------------------------------

/// Shell-quotes `value` for the platform shell that wraps custom commands
/// (`actions::build_shell_command`): POSIX single quotes with the `'\''`
/// escape for `sh -c`, double quotes for `cmd /C`. The cmd branch is
/// conservative: `%` still expands variables inside double quotes and cmd
/// has no safe escape for it there, so a root containing `%` may expand
/// (documented limitation; `%` in a repo path is exotic).
fn quote_for_platform_shell(value: &str) -> String {
    #[cfg(windows)]
    {
        format!("\"{value}\"")
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Renders the command line spawned for a user-defined custom app:
/// every `{path}` occurrence becomes the (shell-quoted) repo root; a
/// template without `{path}` gets the quoted root appended as the final
/// argument. The platform shell parses the line (`sh -c` on Unix, `cmd /C`
/// on Windows — see [`crate::actions::build_shell_command`]), so the app
/// quotes the root for that shell in BOTH cases — quoting is not the
/// template author's job: a repo path with spaces, quotes or shell
/// metacharacters must never terminate the quoted region or inject a
/// second word into the command line.
pub fn render_command(command: &str, root: &str) -> String {
    let quoted = quote_for_platform_shell(root);
    if command.contains("{path}") {
        command.replace("{path}", &quoted)
    } else {
        format!("{command} {quoted}")
    }
}

/// Validates a user-defined custom command template (pure). The frontend
/// enforces the same rule inline; this is the backend's last line of
/// defense before a shell line is built from frontend input.
pub fn validate_command(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        Err("command must not be empty".to_string())
    } else {
        Ok(())
    }
}

/// Spawns a user-defined custom "open with" command with the repository
/// root. Unlike [`open_path_with`] the command line is explicitly
/// user-provided (sent per launch; nothing is persisted backend-side), so
/// the machine-verified-path contract of detected apps does not apply.
/// Fire-and-forget: the shell is spawned detached and never waited on, with
/// null stdio (no output is captured or streamed) and the same sanitized
/// environment as every other child.
pub fn open_custom_with(command: &str, root: &Path) -> Result<(), String> {
    validate_command(command)?;
    if !root.is_dir() {
        return Err(format!(
            "repository root {} does not exist or is not a directory",
            root.display()
        ));
    }
    let root_str = to_native_path(root).to_string_lossy().into_owned();
    let rendered = render_command(command, &root_str);
    // Same platform shell wrapping as the custom shell actions (actions.rs):
    // `cmd /C` on Windows — PATH shims like `code` are .cmd files — and
    // `sh -c` elsewhere.
    let mut cmd = crate::actions::build_shell_command(&rendered);
    crate::cli::apply_sanitized_env(&mut cmd);
    // Login-shell overlay after the sanitize (which env_clear()s) — same
    // env as the custom shell actions (see shell_env.rs).
    crate::shell_env::apply_resolved(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to launch `{command}`: {e}"))
}

/// Opens the repository root with a user-defined command. The frontend
/// sends the full spec per launch (`app_id`/`name` are log context only);
/// nothing is persisted backend-side.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_with_custom(
    repo_root: String,
    app_id: String,
    name: String,
    command: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        open_custom_with(&command, Path::new(&repo_root)).map_err(|e| {
            tracing::warn!(app = %name, app_id = %app_id, error = %e, "open_with_custom failed");
            e
        })
    })
    .await
    .map_err(|e| format!("open_with_custom failed: {e}"))?
}

// -- tests ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty roots: only the platform file manager is detected.
    #[test]
    fn explorer_always_present_and_first() {
        let apps = detect_apps_with_roots(&Roots {
            local_app_data: None,
            program_files: None,
            program_files_x86: None,
            home: None,
            path_dirs: vec![],
        });
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "explorer");
    }

    /// `repo.workdir()` is forward-slashed on Windows and `explorer.exe`
    /// falls back to Documents for those, so the spawn argument must be
    /// converted to the platform form.
    #[test]
    fn spawn_path_uses_native_separators() {
        let converted = to_native_path(Path::new("C:/Code/Projects/repo"));
        if cfg!(windows) {
            assert_eq!(converted.as_os_str(), r"C:\Code\Projects\repo");
        } else {
            assert_eq!(converted.as_os_str(), "C:/Code/Projects/repo");
        }
    }

    /// Builds a fake install tree: `<root>/<rel>` as an empty file.
    /// (Only the Windows-gated tests below install fake editors.)
    #[cfg(windows)]
    fn fake_install(root: &Path, rel: &str) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(&path, b"").expect("touch");
        path
    }

    #[cfg(windows)]
    #[test]
    fn detects_vscode_in_user_install_dir() {
        let dir = std::env::temp_dir().join(format!("mygitui-editors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let programs = dir.join("Programs");
        let expected = fake_install(&programs, "Microsoft VS Code/Code.exe");
        let apps = detect_apps_with_roots(&Roots {
            local_app_data: Some(dir.clone()),
            program_files: None,
            program_files_x86: None,
            home: None,
            path_dirs: vec![],
        });
        let _ = std::fs::remove_dir_all(&dir);
        let vscode = apps
            .iter()
            .find(|a| a.id == "vscode")
            .expect("vscode found");
        // Detection normalizes rel separators; compare the same way.
        assert_eq!(
            vscode.path,
            expected.to_string_lossy().replace('/', "\\").as_str()
        );
    }

    #[cfg(windows)]
    #[test]
    fn detects_jetbrains_toolbox_layouts() {
        let dir = std::env::temp_dir().join(format!("mygitui-editors-jb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // New Toolbox layout: apps/idea/bin/idea64.exe.
        fake_install(&dir, "JetBrains/Toolbox/apps/idea/bin/idea64.exe");
        // Old layout: apps/pycharm/ch-0/241.1/bin/pycharm64.exe.
        fake_install(
            &dir,
            "JetBrains/Toolbox/apps/pycharm/ch-0/241.1/bin/pycharm64.exe",
        );
        let apps = detect_apps_with_roots(&Roots {
            local_app_data: Some(dir.clone()),
            program_files: None,
            program_files_x86: None,
            home: None,
            path_dirs: vec![],
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert!(apps.iter().any(|a| a.id == "idea"));
        assert!(apps.iter().any(|a| a.id == "pycharm"));
        assert!(!apps.iter().any(|a| a.id == "webstorm"));
    }

    #[test]
    fn detects_editor_on_path() {
        let dir = std::env::temp_dir().join(format!("mygitui-editors-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let exe = if cfg!(windows) { "zed.exe" } else { "zed" };
        std::fs::write(dir.join(exe), b"").expect("touch");
        let apps = detect_apps_with_roots(&Roots {
            local_app_data: None,
            program_files: None,
            program_files_x86: None,
            home: None,
            path_dirs: vec![dir.clone()],
        });
        let _ = std::fs::remove_dir_all(&dir);
        let zed = apps.iter().find(|a| a.id == "zed").expect("zed found");
        assert!(zed.path.ends_with(exe));
    }

    /// Custom command templates: `{path}` is substituted (every occurrence)
    /// with the shell-quoted root; a template without the placeholder gets
    /// the quoted root appended.
    #[test]
    fn render_command_substitutes_path_placeholder() {
        #[cfg(windows)]
        {
            assert_eq!(
                render_command("code {path}", "/repos/demo"),
                "code \"/repos/demo\""
            );
            assert_eq!(
                render_command("{path} --git-dir {path}/.git", "/r"),
                "\"/r\" --git-dir \"/r\"/.git"
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(
                render_command("code {path}", "/repos/demo"),
                "code '/repos/demo'"
            );
            assert_eq!(
                render_command("{path} --git-dir {path}/.git", "/r"),
                "'/r' --git-dir '/r'/.git"
            );
        }
    }

    #[test]
    fn render_command_appends_quoted_root_without_placeholder() {
        #[cfg(windows)]
        assert_eq!(
            render_command("kitty", "/repos/my repo"),
            "kitty \"/repos/my repo\""
        );
        #[cfg(not(windows))]
        assert_eq!(
            render_command("kitty", "/repos/my repo"),
            "kitty '/repos/my repo'"
        );
    }

    /// A hostile root (quotes, `$()`, `;`, newline) stays inside one quoted
    /// region: on POSIX the single-quoted form escapes every `'` as `'\''`
    /// so nothing can end the quote early or start a new shell word.
    #[cfg(not(windows))]
    #[test]
    fn render_command_quotes_hostile_roots() {
        let rendered = render_command("code {path}", "/a b$(reboot); `x` \"q\" 'a' \n$V");
        assert_eq!(
            rendered, "code '/a b$(reboot); `x` \"q\" '\\''a'\\'' \n$V'",
            "every single quote must become the '\\'' escape"
        );
        // Same for the appended (no placeholder) form.
        assert_eq!(
            render_command("code", "/a; rm -rf ~"),
            "code '/a; rm -rf ~'"
        );
    }

    /// Empty/whitespace templates are rejected before anything is spawned.
    #[test]
    fn validate_command_rejects_empty() {
        assert!(validate_command("").is_err());
        assert!(validate_command("   ").is_err());
        assert!(validate_command("code {path}").is_ok());
    }

    /// Remote-SSH host label: `[user@]host`, `:port` only when not 22.
    #[test]
    fn ssh_host_label_forms() {
        assert_eq!(
            super::ssh_host_label("u", "dev.example.com", 22),
            "u@dev.example.com"
        );
        assert_eq!(
            super::ssh_host_label("u", "dev.example.com", 2222),
            "u@dev.example.com:2222"
        );
        assert_eq!(
            super::ssh_host_label("", "dev.example.com", 22),
            "dev.example.com"
        );
        assert_eq!(super::ssh_host_label("  ", "h", 0), "h", "port 0 = unset");
    }

    /// Remote launch argv: the VS Code family gets `--remote ssh-remote+<label>`
    /// plus the DEVICE-side path; other apps are refused with a named error
    /// (never a broken local launch).
    #[test]
    fn remote_open_targets_only_vscode_family() {
        let args = super::remote_launch_args("/home/u/repo", "u@dev:2222");
        assert_eq!(
            args,
            vec!["--remote", "ssh-remote+u@dev:2222", "/home/u/repo"]
        );

        assert!(super::supports_remote_ssh("vscode"));
        assert!(super::supports_remote_ssh("vscode-insiders"));
        assert!(super::supports_remote_ssh("vscodium"));
        assert!(super::supports_remote_ssh("cursor"));
        assert!(super::supports_remote_ssh("windsurf"));
        assert!(!super::supports_remote_ssh("zed"));
        assert!(!super::supports_remote_ssh("explorer"));
        assert!(!super::supports_remote_ssh("idea"));

        let entry = |id: &str, name: &str| AppEntry {
            id: id.into(),
            name: name.into(),
            path: "unused".into(),
        };
        // Refusal happens before any spawn (so it is safely testable).
        let err = super::open_remote_with(&entry("zed", "Zed"), "dev", "/r", "u@dev").unwrap_err();
        assert!(err.contains("Zed"), "the error names the editor: {err}");
        assert!(err.contains("SSH remote support"), "unexpected: {err}");
    }

    /// The repo root must exist and be a directory; this fires before any
    /// spawn, so the test never launches a process.
    #[test]
    fn custom_open_rejects_missing_root() {
        let err = open_custom_with("true {path}", Path::new("/definitely/not/a/dir")).unwrap_err();
        assert!(err.contains("does not exist"), "unexpected error: {err}");
    }
}
