//! Repo lifecycle (open/close) and working-copy status/refs reads.
//!
//! These are the per-tab baseline commands: `repo_open`/`repo_close` own the
//! [`crate::repo::RepoManager`] lifecycle (watcher + stream + pty teardown),
//! while `repo_status`/`repo_refs` are queued reads on the repo's read lane
//! (see the dispatch rule in [`super`]).

use std::path::Path;

use tauri::State;

use crate::engine::types::{RepoId, RepoInfo, RepoStatus};
use crate::repo::RepoManager;

use super::{engine_err, enqueue_read, enqueue_remote_read, finish_op, get_handle};

/// Open (or reuse) the repository at/above `path` and start its watcher.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_open(
    path: String,
    app: tauri::AppHandle,
    state: State<'_, RepoManager>,
) -> Result<RepoInfo, String> {
    state.open(&app, Path::new(&path)).map_err(engine_err)
}

/// Close a repository: stop its watcher, cancel its streams, drop caches,
/// and kill the repo's terminal sessions (backend backstop — the FE
/// normally `pty_kill`s its terminal panels itself). Async: the watcher
/// teardown joins the debounce thread, which can wait out an in-flight
/// emit — that must not occupy the main thread.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_close(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
    registry: State<'_, crate::pty::PtyRegistry>,
) -> Result<(), String> {
    // The session root must be captured before the handle leaves the map.
    let root = state.get(&repo_id).map(|handle| handle.root.clone());
    state.close(&repo_id);
    if let Some(root) = root {
        crate::pty::kill_sessions_for_root(&registry, &root);
    }
    Ok(())
}

/// Working-copy status. `paths` (repo-relative, from a watcher batch's
/// `paths`) gates an incremental refresh: only those pathspecs are rescanned
/// and merged into the cached full snapshot; absent paths (or any fallback
/// condition — stale cache, git-dir paths, large touched set) trigger a full
/// rescan. Same `RepoStatus` shape and ordering either way; remote repos
/// always rescan fully (see `RepoHandle::status` for the exact rules).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_status(
    repo_id: RepoId,
    paths: Option<Vec<String>>,
    state: State<'_, RepoManager>,
) -> Result<RepoStatus, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "status",
            move |git, _ws| git.status(),
        ))
        .await;
    }
    let repo = handle.repo()?;
    let status_handle = handle.clone();
    finish_op(enqueue_read(&handle, &repo, "status", move |_ctx, repo| {
        status_handle.status(repo, paths.as_deref())
    }))
    .await
}

/// Local + remote branches and tags: `(ref name, target sha)` pairs.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_refs(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<(String, String)>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "refs",
            move |git, _ws| git.refs(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "refs", move |_ctx, repo| {
        engine.refs(repo)
    }))
    .await
}
