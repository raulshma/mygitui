use std::sync::Arc;

use tauri::State;

use crate::actions::{ActionRegistry, ActionSpec, TauriActionSink};
use crate::cli;
use crate::engine::git_engine::EngineError;
use crate::engine::types::{
    ChangeKind, CheckpointInfo, CommitOptions, HookInfo, PreviewInfo, RepoId, SigningInfo,
    StageRequest, StageTarget,
};
use crate::repo::RepoManager;
use crate::service::{
    engine_err, enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read,
    finish_op, get_handle,
};

/// M12: argv captured at first launch (`mygitui <path>`); consumed once.
/// Second-launch arguments arrive via the `cli-args` event instead.
#[tauri::command(rename_all = "snake_case")]
pub async fn cli_args_initial(state: State<'_, crate::CliArgs>) -> Result<Vec<String>, String> {
    Ok(std::mem::take(&mut state.0.lock().unwrap()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stage(
    repo_id: RepoId,
    request: StageRequest,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "stage",
            move |git, _ws| git.stage(&request),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "stage",
        move |_ctx, repo| engine.stage(repo, &request),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stage_all(
    repo_id: RepoId,
    unstage: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The remote engine has no stage-all primitive: mirror the local
        // `add -A` / index-reset semantics through status + per-file stage
        // targets (worktree-changed entries stage, index-changed unstage).
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "stage",
            move |git, _ws| {
                let status = git.status()?;
                let targets: Vec<StageTarget> = status
                    .entries
                    .iter()
                    .filter(|entry| {
                        if unstage {
                            entry.index != ChangeKind::Unmodified
                        } else {
                            entry.worktree != ChangeKind::Unmodified
                        }
                    })
                    .map(|entry| StageTarget::File(entry.path.clone()))
                    .collect();
                git.stage(&StageRequest { targets, unstage })
            },
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "stage",
        move |_ctx, repo| engine.stage_all(repo, unstage),
    ))
    .await
}

/// Discard local changes (files or hunks). A checkpoint is snapshotted
/// BEFORE any side effect — a failed discard leaves the repo untouched
/// (same ordering rule as repo_clean).
#[tauri::command(rename_all = "snake_case")]
pub async fn discard(
    repo_id: RepoId,
    targets: Vec<StageTarget>,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Same ordering rule as the local path: checkpoint BEFORE discard.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "discard",
            move |git, _ws| {
                git.checkpoint_create("discard changes")?;
                git.discard(&targets)
            },
        ))
        .await;
    }
    let engine = handle.engine();
    let m3 = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "discard",
        move |_ctx, repo| {
            m3.checkpoint_create(repo, "discard changes")?;
            engine.discard(repo, &targets)
        },
    ))
    .await
}

/// Commit: engine first; `EngineError::Unsupported` (active hooks/signing
/// per C1's routing) falls back to the git CLI (cli.rs) so hooks and
/// signing actually run. Returns the new sha.
#[tauri::command(rename_all = "snake_case")]
pub async fn commit(
    repo_id: RepoId,
    options: CommitOptions,
    state: State<'_, RepoManager>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The device's git runs hooks and signing natively — no CLI
        // fallback needed (the remote engine IS the git CLI there).
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "commit",
            move |git, _ws| git.commit(&options),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    let workdir = handle.root.clone();
    let generation = handle.generation_shared();
    let (_op_id, rx) = handle.ops().enqueue("commit", move |ctx| {
        let outcome = {
            let guard = repo.lock();
            engine.commit(&guard, &options)
        };
        let sha = match outcome {
            Ok(sha) => Ok(sha),
            Err(EngineError::Unsupported(reason)) => {
                ctx.emit_progress(&format!("falling back to git CLI: {reason}"), None);
                ctx.emit_log(&format!("using git CLI: {reason}"));
                cli::commit_via_cli(&workdir, &options, &mut |line| ctx.emit_log(line))
            }
            Err(err) => Err(engine_err(err)),
        };
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        sha
    });
    finish_op(rx).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn signing_info(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<SigningInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "commit",
            move |git, _ws| git.signing_info(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "commit", move |_ctx, repo| {
        engine.signing_info(repo)
    }))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn hooks_list(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<HookInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "commit",
            move |git, _ws| git.hooks(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "commit", move |_ctx, repo| {
        engine.hooks(repo)
    }))
    .await
}

// ---------------------------------------------------------------------------
// M3 (lane D4): checkpoints (undo system) + dangerous-op previews.
//
// Checkpoints are snapshot commits under hidden refs
// `refs/mygitui/checkpoints/<unix_millis>-<reason>` capturing the full
// workdir state (tracked + untracked) over the current HEAD. Restore
// auto-creates a `pre-restore` checkpoint first (the undo system is itself
// undoable) and never moves HEAD or branch refs; GC keeps the newest
// checkpoint regardless of age.
//
// Routing: create/restore/gc are mutations and go through the serial op
// queue with the generation bump (`enqueue_mutation`, mirroring M2); the
// reads (`checkpoints`, `ops_preview`) use `enqueue_read`. The trait
// forwarding lives in merge.rs (the crate's single `impl GitEngineM3`), so
// every handler below reaches the engine through `handle.m3()` — no direct
// `Libgit2Engine` instantiation (see the DISPATCH RULE atop the M2 section;
// the test module at the bottom of this file still drives `*_impl` directly
// and imports it itself).
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn checkpoint_create(
    repo_id: RepoId,
    reason: String,
    state: State<'_, RepoManager>,
) -> Result<CheckpointInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "checkpoint",
            move |git, _ws| git.checkpoint_create(&reason),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "checkpoint",
        move |_ctx, repo| engine.checkpoint_create(repo, &reason),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn checkpoints(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<CheckpointInfo>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "checkpoint",
            move |git, _ws| git.checkpoints(),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(
        &handle,
        &repo,
        "checkpoint",
        move |_ctx, repo| engine.checkpoints(repo),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn checkpoint_restore(
    repo_id: RepoId,
    id: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "checkpoint",
            move |git, _ws| git.checkpoint_restore(&id),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "checkpoint",
        move |_ctx, repo| engine.checkpoint_restore(repo, &id),
    ))
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn checkpoint_gc(
    repo_id: RepoId,
    older_than_days: u32,
    state: State<'_, RepoManager>,
) -> Result<u32, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "checkpoint",
            move |git, _ws| git.checkpoint_gc(older_than_days),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "checkpoint",
        move |_ctx, repo| engine.checkpoint_gc(repo, older_than_days),
    ))
    .await
}

/// Pure what-if preview of a dangerous op (no mutation, no checkpoint):
/// `kind` is one of "reset_hard" / "clean" / "checkout_force" /
/// "branch_delete", `params` carries the kind's arguments (e.g.
/// `{"to": "<commit-ish>"}`, `{"dirs": bool}`, `{"name": "...",
/// "into": "..."}`).
#[tauri::command(rename_all = "snake_case")]
pub async fn ops_preview(
    repo_id: RepoId,
    kind: String,
    params: serde_json::Value,
    state: State<'_, RepoManager>,
) -> Result<PreviewInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "preview",
            move |git, _ws| git.ops_preview(&kind, &params),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_read(
        &handle,
        &repo,
        "preview",
        move |_ctx, repo| engine.ops_preview(repo, &kind, &params),
    ))
    .await
}
/// Snapshot current state as an undo point right before a dangerous op.
/// FE flow: ops_preview → dialog → guard_checkpoint → confirm → op. It
/// writes a checkpoint ref, so it is a queue mutation (serialized with the
/// op it guards and generation-bumping like any other mutation).
#[tauri::command(rename_all = "snake_case")]
pub async fn guard_checkpoint(
    repo_id: RepoId,
    reason: String,
    state: State<'_, RepoManager>,
) -> Result<CheckpointInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "checkpoint",
            move |git, _ws| git.guard_checkpoint(&reason),
        ))
        .await;
    }
    let engine = handle.m3();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "checkpoint",
        move |_ctx, repo| engine.checkpoint_create(repo, &reason),
    ))
    .await
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

/// Best-effort cancel of a queued/running mutation op. Cheap (atomics only)
/// so it runs inline, off the repo mutex.
#[tauri::command(rename_all = "snake_case")]
pub async fn op_cancel(
    repo_id: RepoId,
    op_id: String,
    state: State<'_, RepoManager>,
) -> Result<bool, String> {
    let handle = get_handle(&state, &repo_id)?;
    Ok(handle.ops().cancel_op(repo_id.0.as_str(), &op_id))
}

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
