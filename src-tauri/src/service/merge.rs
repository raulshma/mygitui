use tauri::State;

use crate::engine::types::{
    BisectLogEntry, BisectMark, BisectState, ConflictFile, ConflictResolution, MergeOptions,
    MergeResult, RebaseState, RebaseStep, RepoId, ResetKind,
};
use crate::repo::RepoManager;
use crate::service::{
    enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read, finish_op,
    get_handle,
};

// ---------- M3: power + safety commands ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn merge_branch(
    repo_id: RepoId,
    ref_name: String,
    opts: MergeOptions,
    state: State<'_, RepoManager>,
) -> Result<MergeResult, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "merge",
            move |git, _ws| git.merge_branch(&ref_name, &opts),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "merge",
        move |_ctx, repo| engine.merge_branch(repo, &ref_name, &opts),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn merge_abort(repo_id: RepoId, state: State<'_, RepoManager>) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "merge",
            move |git, _ws| git.merge_abort(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "merge",
        move |_ctx, repo| engine.merge_abort(repo),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn sequencer_abort(repo_id: RepoId, state: State<'_, RepoManager>) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "merge",
            move |git, _ws| git.sequencer_abort(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "merge",
        move |_ctx, repo| engine.sequencer_abort(repo),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conflicts(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<ConflictFile>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "merge",
            move |git, _ws| git.conflicts(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "merge", move |_ctx, repo| {
        engine.conflicts(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn conflict_resolve(
    repo_id: RepoId,
    path: String,
    resolution: ConflictResolution,
    custom_content: Option<Vec<u8>>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "merge",
            move |git, _ws| git.conflict_resolve(&path, resolution, custom_content),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "merge",
        move |_ctx, repo| {
            engine.conflict_resolve(repo, &path, resolution, custom_content.as_deref())
        },
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cherry_pick(
    repo_id: RepoId,
    shas: Vec<String>,
    state: State<'_, RepoManager>,
) -> Result<MergeResult, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Local path commits as it goes; `no_commit: false` mirrors that.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "merge",
            move |git, _ws| git.cherry_pick(&shas, false),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "merge",
        move |_ctx, repo| engine.cherry_pick(repo, &shas),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn revert(
    repo_id: RepoId,
    shas: Vec<String>,
    state: State<'_, RepoManager>,
) -> Result<MergeResult, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Local path commits as it goes; `no_commit: false` mirrors that.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "merge",
            move |git, _ws| git.revert(&shas, false),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "merge",
        move |_ctx, repo| engine.revert(repo, &shas),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reset(
    repo_id: RepoId,
    kind: ResetKind,
    to: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "merge",
            move |git, _ws| git.reset(kind, &to),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "merge",
        move |_ctx, repo| engine.reset(repo, kind, &to),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rebase_start(
    repo_id: RepoId,
    plan: Vec<RebaseStep>,
    onto: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<RebaseState, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "rebase",
            move |git, _ws| git.rebase_start(&plan, onto.as_deref()),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "rebase",
        move |_ctx, repo| engine.rebase_start(repo, &plan, onto.as_deref()),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rebase_state(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<RebaseState, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "rebase",
            move |git, _ws| git.rebase_state(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "rebase", move |_ctx, repo| {
        engine.rebase_state(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rebase_continue(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<RebaseState, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "rebase",
            move |git, _ws| git.rebase_continue(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "rebase",
        move |_ctx, repo| engine.rebase_continue(repo),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rebase_abort(repo_id: RepoId, state: State<'_, RepoManager>) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "rebase",
            move |git, _ws| git.rebase_abort(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "rebase",
        move |_ctx, repo| engine.rebase_abort(repo),
    ))
    .await
}

// ---------- M10: bisect / describe / autosquash ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn bisect_start(
    repo_id: RepoId,
    bad: Option<String>,
    good: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<BisectState, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "bisect",
            move |git, _ws| git.bisect_start(bad.as_deref(), good.as_deref()),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "bisect",
        move |_ctx, repo| engine.bisect_start(repo, bad.as_deref(), good.as_deref()),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn bisect_state(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<BisectState, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "bisect",
            move |git, _ws| git.bisect_state(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "bisect", move |_ctx, repo| {
        engine.bisect_state(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn bisect_mark(
    repo_id: RepoId,
    mark: BisectMark,
    state: State<'_, RepoManager>,
) -> Result<BisectState, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Local path marks HEAD; `rev: None` mirrors that.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "bisect",
            move |git, _ws| git.bisect_mark(mark, None),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "bisect",
        move |_ctx, repo| engine.bisect_mark(repo, mark),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn bisect_reset(repo_id: RepoId, state: State<'_, RepoManager>) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "bisect",
            move |git, _ws| git.bisect_reset(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "bisect",
        move |_ctx, repo| engine.bisect_reset(repo),
    ))
    .await
}

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

/// Recorded mark history for a bisect: the live run's log, else the last
/// archived run (read).
#[tauri::command(rename_all = "snake_case")]
pub async fn bisect_log(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<BisectLogEntry>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "bisect",
            move |git, _ws| git.bisect_log(),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "bisect", move |_ctx, repo| {
        engine.bisect_log(repo)
    }))
    .await
}
