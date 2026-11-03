//! File-editor surface: stat/read/write plus external-editor launch.
//!
//! Thin re-exports so the UI keeps calling `service::*` only; the
//! implementations live in [`crate::workspace`] (mtime-guarded, queued)
//! and [`crate::editors`] (app detection + launch).

pub use crate::workspace::{repo_stat_file, repo_write_file};

use std::path::Path;

use crate::editors::{self, AppEntry};

/// Detected external editor apps (PATH probe; cheap but not free, so the
/// UI loads once per session).
pub fn detect_editors() -> Vec<AppEntry> {
    editors::detect_apps()
}

/// Open a worktree file in the chosen external app. `path` is
/// worktree-relative; it is joined onto the repo root.
pub fn open_in_editor(
    services: &crate::service::AppServices,
    repo_id: &crate::engine::types::RepoId,
    app_id: &str,
    path: &str,
) -> Result<(), String> {
    let handle = crate::service::get_handle(&services.repos, repo_id)?;
    let app = editors::detect_apps()
        .into_iter()
        .find(|app| app.id == app_id)
        .ok_or_else(|| format!("editor app `{app_id}` not found"))?;
    editors::open_path_with(&app, &Path::new(&handle.root).join(path))
}
