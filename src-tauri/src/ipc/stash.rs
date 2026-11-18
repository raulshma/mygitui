//! Stash and linked-worktree lane: `stash_*` (list/push/show/rename/apply/
//! drop/branch) and `worktree_*` (list/add/remove/lock/unlock/prune), all
//! queue-dispatched mutations or reads over the engine's M3/M12 traits.

use tauri::State;

use crate::engine::types::{FileDiff, RepoId, StashInfo, WorktreeInfo};
use crate::repo::RepoManager;

use super::{
    enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read, finish_op,
    get_handle,
};

/// `git worktree prune`; returns the number of pruned worktrees.
#[tauri::command(rename_all = "snake_case")]
pub async fn worktree_prune(repo_id: RepoId, state: State<'_, RepoManager>) -> Result<u32, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "worktree",
            move |git, _ws| {
                // The remote engine prunes but reports no count; 0 = best-effort.
                git.worktree_prune()?;
                Ok(0)
            },
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "worktree",
        move |_ctx, repo| engine.worktree_prune(repo),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stash_list(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<StashInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "stash",
            move |git, _ws| git.stash_list(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "stash", move |_ctx, repo| {
        engine.stash_list(repo)
    }))
    .await
}

/// Stash the worktree/index. `paths` limits the stash to those pathspecs
/// (`git stash push -- <paths>`); `None`/empty stashes everything. Existing
/// FE payloads without `paths` keep stashing all changes.
#[tauri::command(rename_all = "snake_case")]
pub async fn stash_push(
    repo_id: RepoId,
    message: Option<String>,
    keep_index: bool,
    include_untracked: bool,
    paths: Option<Vec<String>>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "stash",
            move |git, _ws| {
                git.stash_push(
                    message.as_deref(),
                    keep_index,
                    include_untracked,
                    paths.as_deref(),
                )
            },
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "stash",
        move |_ctx, repo| {
            engine.stash_push(
                repo,
                message.as_deref(),
                keep_index,
                include_untracked,
                paths.as_deref(),
            )
        },
    ))
    .await
}

/// `git stash show <stash>`: the file diffs `stash@{index}` carries (base
/// commit vs stash commit).
#[tauri::command(rename_all = "snake_case")]
pub async fn stash_show(
    repo_id: RepoId,
    index: u32,
    state: State<'_, RepoManager>,
) -> Result<Vec<FileDiff>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "stash",
            move |git, _ws| git.stash_show(index),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "stash", move |_ctx, repo| {
        engine.stash_show(repo, index)
    }))
    .await
}

/// Rename a stash entry's message. Only the newest entry (`stash@{0}`) can
/// be renamed — deeper renames would require rebuilding the whole reflog
/// stack — and the engine rejects them with a clear error.
#[tauri::command(rename_all = "snake_case")]
pub async fn stash_rename(
    repo_id: RepoId,
    index: u32,
    message: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "stash",
            move |git, _ws| git.stash_rename(index, &message),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "stash",
        move |_ctx, repo| engine.stash_rename(repo, index, &message),
    ))
    .await
}

/// Apply (or pop) `stash@{index}`. `restore_index` re-instates the stashed
/// index state (`--index`: staged changes come back staged); `None` from
/// older FE payloads means false.
#[tauri::command(rename_all = "snake_case")]
pub async fn stash_apply(
    repo_id: RepoId,
    index: u32,
    pop: bool,
    restore_index: Option<bool>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let restore_index = restore_index.unwrap_or(false);
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Pop = apply + drop, and the drop only runs after a clean apply
        // (same rule as the local engine's stash_apply_impl).
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "stash",
            move |git, _ws| {
                git.stash_apply(index, restore_index)?;
                if pop {
                    git.stash_drop(index)?;
                }
                Ok(())
            },
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "stash",
        move |_ctx, repo| engine.stash_apply(repo, index, pop, restore_index),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stash_drop(
    repo_id: RepoId,
    index: u32,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "stash",
            move |git, _ws| git.stash_drop(index),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "stash",
        move |_ctx, repo| engine.stash_drop(repo, index),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stash_branch(
    repo_id: RepoId,
    name: String,
    index: u32,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "stash",
            move |git, _ws| git.stash_branch(&name, index),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "stash",
        move |_ctx, repo| engine.stash_branch(repo, &name, index),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn worktrees(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<WorktreeInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "worktree",
            move |git, _ws| git.worktrees(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(
        &handle,
        &repo,
        "worktree",
        move |_ctx, repo| engine.worktrees(repo),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn worktree_add(
    repo_id: RepoId,
    path: String,
    branch: Option<String>,
    new_branch: Option<String>,
    from_remote: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "worktree",
            move |git, _ws| {
                git.worktree_add(
                    &path,
                    branch.as_deref(),
                    new_branch.as_deref(),
                    from_remote.as_deref(),
                )
            },
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "worktree",
        move |_ctx, repo| {
            engine.worktree_add(
                repo,
                &path,
                branch.as_deref(),
                new_branch.as_deref(),
                from_remote.as_deref(),
            )
        },
    ))
    .await
}

/// `git worktree lock [--reason <r>] <path|name>` (Phase 5): the spec may be
/// a linked worktree's name or its path. Locked worktrees show `locked` in
/// `worktrees` (and the reason drives the FE tooltip).
#[tauri::command(rename_all = "snake_case")]
pub async fn worktree_lock(
    repo_id: RepoId,
    path: String,
    reason: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "worktree",
            move |git, _ws| git.worktree_lock(&path, reason.as_deref()),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "worktree",
        move |_ctx, repo| engine.worktree_lock(repo, &path, reason.as_deref()),
    ))
    .await
}

/// `git worktree unlock <path|name>` (Phase 5).
#[tauri::command(rename_all = "snake_case")]
pub async fn worktree_unlock(
    repo_id: RepoId,
    path: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "worktree",
            move |git, _ws| git.worktree_unlock(&path),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "worktree",
        move |_ctx, repo| engine.worktree_unlock(repo, &path),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn worktree_remove(
    repo_id: RepoId,
    name: String,
    force: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // (The local path emits a one-line progress event; the silent
        // remote read/mutation helpers carry no OpCtx, so the remote op
        // relies on the queue's final done event instead.)
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "worktree",
            move |git, _ws| git.worktree_remove(&name, force),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "worktree",
        move |ctx, repo| {
            ctx.emit_progress(&format!("removing worktree `{name}`"), None);
            engine.worktree_remove(repo, &name, force)
        },
    ))
    .await
}
