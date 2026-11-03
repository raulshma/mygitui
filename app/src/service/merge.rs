use crate::engine::types::{
    BisectLogEntry, BisectMark, BisectState, ConflictFile, ConflictResolution, MergeOptions,
    MergeResult, RebaseState, RebaseStep, RepoId, ResetKind,
};
use crate::service::{
    enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read, finish_op,
    get_handle, AppServices,
};

// ---------- M3: power + safety commands ----------

pub async fn merge_branch(
    services: &AppServices,
    repo_id: RepoId,
    ref_name: String,
    opts: MergeOptions,
) -> Result<MergeResult, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn merge_abort(services: &AppServices, repo_id: RepoId) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn sequencer_abort(services: &AppServices, repo_id: RepoId) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn conflicts(
    services: &AppServices,
    repo_id: RepoId,
) -> Result<Vec<ConflictFile>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn conflict_resolve(
    services: &AppServices,
    repo_id: RepoId,
    path: String,
    resolution: ConflictResolution,
    custom_content: Option<Vec<u8>>,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn cherry_pick(
    services: &AppServices,
    repo_id: RepoId,
    shas: Vec<String>,
) -> Result<MergeResult, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn revert(
    services: &AppServices,
    repo_id: RepoId,
    shas: Vec<String>,
) -> Result<MergeResult, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn reset(
    services: &AppServices,
    repo_id: RepoId,
    kind: ResetKind,
    to: String,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn rebase_start(
    services: &AppServices,
    repo_id: RepoId,
    plan: Vec<RebaseStep>,
    onto: Option<String>,
) -> Result<RebaseState, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

/// Plain rebase plan for "rebase HEAD onto `base`": every commit in
/// `base..HEAD` (oldest first) as a `pick` step. An empty plan means HEAD
/// is already on top of `base` (nothing to rebase).
pub async fn rebase_plan(
    services: &AppServices,
    repo_id: RepoId,
    base: String,
) -> Result<Vec<RebaseStep>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "rebase",
            move |git, _ws| git.rebase_plan(&base),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "rebase", move |_ctx, repo| {
        engine.rebase_plan(repo, &base)
    }))
    .await
}

pub async fn rebase_state(services: &AppServices, repo_id: RepoId) -> Result<RebaseState, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn rebase_continue(
    services: &AppServices,
    repo_id: RepoId,
) -> Result<RebaseState, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn rebase_abort(services: &AppServices, repo_id: RepoId) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn bisect_start(
    services: &AppServices,
    repo_id: RepoId,
    bad: Option<String>,
    good: Option<String>,
) -> Result<BisectState, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn bisect_state(services: &AppServices, repo_id: RepoId) -> Result<BisectState, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn bisect_mark(
    services: &AppServices,
    repo_id: RepoId,
    mark: BisectMark,
) -> Result<BisectState, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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

pub async fn bisect_reset(services: &AppServices, repo_id: RepoId) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
pub async fn worktree_prune(services: &AppServices, repo_id: RepoId) -> Result<u32, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
pub async fn bisect_log(
    services: &AppServices,
    repo_id: RepoId,
) -> Result<Vec<BisectLogEntry>, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
