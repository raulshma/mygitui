use tauri::ipc::Channel;
use tauri::State;

use crate::engine::types::{DiffSide, FileDiff, RepoId};
use crate::repo::RepoManager;
use crate::service::{
    chunk_vec, engine_err, enqueue_read, enqueue_remote_read, finish_op, get_handle,
};

/// Files per `repo_diff_stream` page.
const DIFF_PAGE_SIZE: usize = 50;

/// One-shot diff for small/medium changes; large diffs use
/// [`repo_diff_stream`].
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_diff(
    repo_id: RepoId,
    old: DiffSide,
    new: DiffSide,
    paths: Option<Vec<String>>,
    state: State<'_, RepoManager>,
) -> Result<Vec<FileDiff>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "diff",
            move |git, _ws| git.diff(&old, &new, paths.as_deref()),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "diff", move |_ctx, repo| {
        engine.diff(repo, &old, &new, paths.as_deref())
    }))
    .await
}

/// Stream a diff in pages of [`DIFF_PAGE_SIZE`] files over `on_page`.
/// Stops early when the channel send fails (client gone).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_diff_stream(
    repo_id: RepoId,
    old: DiffSide,
    new: DiffSide,
    paths: Option<Vec<String>>,
    on_page: Channel<Vec<FileDiff>>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The remote engine returns the complete diff in one shot: one
        // queued read, then page it out exactly like the local result.
        let diffs = finish_op(enqueue_remote_read(
            &handle,
            remote,
            "diff",
            move |git, _ws| git.diff(&old, &new, paths.as_deref()),
        ))
        .await?;
        for page in chunk_vec(diffs, DIFF_PAGE_SIZE) {
            if on_page.send(page).is_err() {
                break; // client dropped the channel
            }
        }
        return Ok(());
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    let diffs = tauri::async_runtime::spawn_blocking(move || {
        let repo = repo.lock();
        engine
            .diff(&repo, &old, &new, paths.as_deref())
            .map_err(engine_err)
    })
    .await
    .map_err(|e| format!("diff task failed: {e}"))??;
    for page in chunk_vec(diffs, DIFF_PAGE_SIZE) {
        if on_page.send(page).is_err() {
            break; // client dropped the channel
        }
    }
    Ok(())
}
