use crate::engine::types::{RepoId, StashInfo, WorktreeInfo};
use crate::service::{
    enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read, finish_op,
    get_handle, AppServices,
};

pub async fn stash_list(services: &AppServices, repo_id: RepoId) -> Result<Vec<StashInfo>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn stash_push(
    services: &AppServices,
    repo_id: RepoId,
    message: Option<String>,
    keep_index: bool,
    include_untracked: bool,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "stash",
            move |git, _ws| git.stash_push(message.as_deref(), keep_index, include_untracked),
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
            engine.stash_push(repo, message.as_deref(), keep_index, include_untracked)
        },
    ))
    .await
}

pub async fn stash_apply(
    services: &AppServices,
    repo_id: RepoId,
    index: u32,
    pop: bool,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Pop = apply + drop, and the drop only runs after a clean apply
        // (same rule as the local engine's stash_apply_impl).
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "stash",
            move |git, _ws| {
                git.stash_apply(index)?;
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
        move |_ctx, repo| engine.stash_apply(repo, index, pop),
    ))
    .await
}

pub async fn stash_drop(services: &AppServices, repo_id: RepoId, index: u32) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn stash_branch(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    index: u32,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn worktrees(
    services: &AppServices,
    repo_id: RepoId,
) -> Result<Vec<WorktreeInfo>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn worktree_add(
    services: &AppServices,
    repo_id: RepoId,
    path: String,
    branch: Option<String>,
    new_branch: Option<String>,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "worktree",
            move |git, _ws| git.worktree_add(&path, branch.as_deref(), new_branch.as_deref()),
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
            engine.worktree_add(repo, &path, branch.as_deref(), new_branch.as_deref())
        },
    ))
    .await
}

pub async fn worktree_remove(
    services: &AppServices,
    repo_id: RepoId,
    name: String,
    force: bool,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
