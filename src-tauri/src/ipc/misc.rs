//! Odds and ends: first-launch CLI args (`cli_args_initial`), the OS accent
//! color probe, custom shell actions (`action_run`/`action_cancel`), and
//! project-type detection. None of these take the repo op queue — actions
//! and detection are process spawns; the accent color is a registry read.

use std::sync::Arc;

use tauri::State;

use crate::engine::types::RepoId;
use crate::repo::RepoManager;

use super::{enqueue_remote_read, finish_op, get_handle};

/// M12: argv captured at first launch (`mygitui <path>`); consumed once.
/// Second-launch arguments arrive via the `cli-args` event instead.
#[tauri::command(rename_all = "snake_case")]
pub async fn cli_args_initial(state: State<'_, crate::CliArgs>) -> Result<Vec<String>, String> {
    Ok(std::mem::take(&mut state.0.lock()))
}

/// M12: OS accent color for the dynamic-color pipeline (Windows DWM
/// registry read; null on other platforms). Frontend probes it once at
/// theme init.
#[tauri::command(rename_all = "snake_case")]
pub async fn os_accent_color() -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        Ok(windows_accent_color())
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

#[cfg(windows)]
fn windows_accent_color() -> Option<String> {
    // `reg query` output line: "    AccentColor    REG_DWORD    0xff9966dd"
    // The DWORD is 0xAABBGGRR — swizzle to #RRGGBB.
    let output = crate::process::command("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\DWM",
            "/v",
            "AccentColor",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|l| l.contains("AccentColor"))?;
    let raw = line.split_whitespace().last()?;
    let dword = u32::from_str_radix(raw.trim_start_matches("0x"), 16).ok()?;
    let r = dword & 0xff;
    let g = (dword >> 8) & 0xff;
    let b = (dword >> 16) & 0xff;
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

// ---------------------------------------------------------------------------
// `action_run` / `action_cancel` delegate to the `actions` runner; the
// registry is process-global managed state (see actions.rs).
// ---------------------------------------------------------------------------

use crate::actions::{ActionRegistry, ActionSpec, TauriActionSink};

/// Run a user-defined shell action with cwd = repo root, streaming its
/// output as `action-output` events. Returns the run id (cancel token).
/// Remote repos run the action on the device over SSH exec through the
/// remote registry (same `action-output` events, same Tauri sink).
#[tauri::command(rename_all = "snake_case")]
pub fn action_run(
    repo_id: RepoId,
    name: String,
    command: String,
    app: tauri::AppHandle,
    state: State<'_, RepoManager>,
    runs: State<'_, ActionRegistry>,
    remote_runs: State<'_, crate::remote::actions::RemoteActionRegistry>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return remote_runs.run(
            &repo_id.0,
            &name,
            &command,
            Arc::new(remote.git()),
            Arc::new(TauriActionSink::new(app)),
        );
    }
    let launch = crate::actions::run_action(
        &runs,
        ActionSpec {
            repo_id: repo_id.0,
            name,
            command,
            cwd: handle.root.clone(),
        },
        Arc::new(TauriActionSink::new(app)),
    )?;
    Ok(launch.run_id)
}

/// Kill a running action; `false` when the run is unknown or already done.
/// The command carries no repo id and run ids are disjoint (`act-` vs
/// `ract-` prefixes), so local is tried first and the remote registry
/// covers remote runs.
#[tauri::command(rename_all = "snake_case")]
pub fn action_cancel(
    run_id: String,
    runs: State<'_, ActionRegistry>,
    remote_runs: State<'_, crate::remote::actions::RemoteActionRegistry>,
) -> Result<bool, String> {
    if runs.cancel(&run_id) {
        return Ok(true);
    }
    Ok(remote_runs.cancel(&run_id))
}

/// Detected project types + runnable scripts under one repository (bounded
/// root + immediate-child scan; see `projects.rs`).
#[tauri::command(rename_all = "snake_case")]
pub async fn project_detect(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<crate::projects::ProjectInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // A local fs scan cannot see the device's POSIX paths; snapshot the
        // tree over SSH and run the same detectors on it.
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "projects",
            move |git, _ws| crate::remote::projects::detect(&git.session, &git.root),
        ))
        .await;
    }
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || crate::projects::detect_sync(&root))
        .await
        .map_err(|e| format!("project_detect task failed: {e}"))
}
