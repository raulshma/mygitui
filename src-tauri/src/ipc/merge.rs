//! History-rewriting and conflict lane: merge / cherry-pick / revert / reset
//! / rebase / bisect, conflict listing + resolution, and the external
//! mergetool bridge. Queue mutations with the generation bump; the
//! cancellation-aware ext paths (`merge_branch_ext`, `rebase_start_ext`)
//! poll the op's cancel token so `op_cancel` can abort a long rebase.
//!
//! `mergetool_run` is deliberately NOT on the op queue: the external tool
//! runs interactively for minutes and would freeze every other repo op.

use tauri::State;

use crate::engine::types::{
    BisectLogEntry, BisectMark, BisectState, ConflictFile, ConflictResolution, MergeOptions,
    MergeResult, MergetoolInfo, MergetoolResult, RebaseState, RebaseStep, RepoId, ResetKind,
};
use crate::repo::RepoManager;

use super::{
    emit_checkout_progress, enqueue_mutation, enqueue_read, enqueue_remote_mutation,
    enqueue_remote_read, finish_op, get_handle,
};

/// Configured external merge tools (pure config read).
#[tauri::command(rename_all = "snake_case")]
pub async fn mergetool_info(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<MergetoolInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    if handle.remote().is_some() {
        return Err("mergetool_info is not supported for remote projects".into());
    }
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "merge", move |_ctx, repo| {
        Ok(crate::mergetool::mergetool_info(repo))
    }))
    .await
}

/// Launch `git mergetool` for one conflicted path. Deliberately NOT on the
/// op queue: the external tool runs interactively for minutes, and blocking
/// every repo op behind it would freeze the app (watcher events still fire
/// when the tool writes files).
#[tauri::command(rename_all = "snake_case")]
pub async fn mergetool_run(
    repo_id: RepoId,
    path: String,
    tool: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<MergetoolResult, String> {
    let handle = get_handle(&state, &repo_id)?;
    if handle.remote().is_some() {
        // An interactive local merge tool cannot run against a workdir on
        // another device; refuse explicitly instead of failing on the
        // nonexistent local path.
        return Err("mergetool_run is not supported for remote projects".into());
    }
    let workdir = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::mergetool::mergetool_run(&workdir, &path, tool.as_deref())
    })
    .await
    .map_err(|e| format!("mergetool_run task failed: {e}"))?
}

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
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "merge",
        move |ctx, repo| {
            // Cancellation-aware ext path (see fetch): `op_cancel` flips the
            // ctx token; merge polls it up-front and between the analysis +
            // mutating phases. Deliberate `Libgit2Engine` exception to the
            // dispatch rule — `merge_branch_ext` is not on the M3 trait
            // (same reason as fetch).
            let engine = crate::engine::libgit2::Libgit2Engine::new();
            engine.merge_branch_ext(repo, &ref_name, &opts, ctx.cancel_token().as_deref())
        },
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

/// Resolve one conflicted file. `custom_content_b64` carries the editor's
/// bytes as base64 (STANDARD, with padding — same encoding as
/// `repo_write_file`'s `content_b64`): a compact string over the IPC bridge
/// instead of a JSON number array (~7-8 bytes per byte of file).
#[tauri::command(rename_all = "snake_case")]
pub async fn conflict_resolve(
    repo_id: RepoId,
    path: String,
    resolution: ConflictResolution,
    custom_content_b64: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let custom_content = match custom_content_b64.as_deref() {
        Some(b64) => Some(
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.as_bytes())
                .map_err(|e| format!("invalid conflict content encoding: {e}"))?,
        ),
        None => None,
    };
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
        move |ctx, repo| {
            engine.reset(repo, kind, &to, &mut |progress| {
                emit_checkout_progress(&ctx, progress)
            })
        },
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
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "rebase",
        move |ctx, repo| {
            // Cancellation-aware ext path (see fetch): `op_cancel` flips the
            // ctx token; the rebase loop polls it per step. Deliberate
            // `Libgit2Engine` exception to the dispatch rule —
            // `rebase_start_ext` is not on the M3 trait (same reason as
            // fetch).
            let engine = crate::engine::libgit2::Libgit2Engine::new();
            engine.rebase_start_ext(repo, &plan, onto.as_deref(), ctx.cancel_token().as_deref())
        },
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

/// Skip the paused rebase step (`git rebase --skip`): valid only while
/// paused on a conflicted pick or a failed `exec`; the step records nothing.
#[tauri::command(rename_all = "snake_case")]
pub async fn rebase_skip(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<RebaseState, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "rebase",
            move |git, _ws| git.rebase_skip(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "rebase",
        move |_ctx, repo| engine.rebase_skip(repo),
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

/// `git describe --tags` for a commit-ish.
#[tauri::command(rename_all = "snake_case")]
pub async fn describe(
    repo_id: RepoId,
    spec: String,
    state: State<'_, RepoManager>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "describe",
            move |git, _ws| git.describe(&spec),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(
        &handle,
        &repo,
        "describe",
        move |_ctx, repo| engine.describe(repo, &spec),
    ))
    .await
}

/// Builds an autosquash rebase plan for `base..HEAD` (feeds `rebase_start`).
#[tauri::command(rename_all = "snake_case")]
pub async fn autosquash_plan(
    repo_id: RepoId,
    base: String,
    state: State<'_, RepoManager>,
) -> Result<Vec<RebaseStep>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "rebase",
            move |git, _ws| git.autosquash_plan(&base),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "rebase", move |_ctx, repo| {
        engine.autosquash_plan(repo, &base)
    }))
    .await
}
