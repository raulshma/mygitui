//! Staging, commit, discard, checkpoints/undo, dangerous-op previews, the
//! branch trash, git clean, and op cancellation.
//!
//! Everything here mutates the workdir/index/refs, so it rides the per-repo
//! serial op queue with the generation bump (see the dispatch rule in
//! [`super`]). `commit` falls back to the git CLI when libgit2 reports
//! `Unsupported` (hooks/signing per C1's routing).

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::State;
use tokio::sync::oneshot;

use crate::cli;
use crate::engine::git_engine::EngineError;
use crate::engine::types::{
    BranchTrashEntry, ChangeKind, CheckpointInfo, CommitOptions, CommitSignature, HookInfo, RepoId,
    SigningInfo, StageRequest, StageTarget,
};
use crate::repo::{RepoHandle, RepoManager};

use super::{
    engine_err, enqueue_mutation, enqueue_read, enqueue_remote_mutation, enqueue_remote_read,
    finish_op, get_handle,
};
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

/// Whether an optional `checkpoint` flag means "snapshot an undo point":
/// absent/unset defaults to true (checkpoints are the safety net; the FE
/// passes the global "checkpoints for git changes" setting here).
fn checkpoint_enabled(flag: Option<bool>) -> bool {
    flag.unwrap_or(true)
}

/// Outcome of the pre-discard checkpoint step. `None` = skipped (flag off)
/// or created; `Some(reason)` = creation FAILED but the discard proceeds —
/// the reason travels back to the FE as the warning payload. A checkpoint
/// failure (most commonly a missing user.name/user.email) must not abort
/// the op: the workdir still carries the changes, so failing the whole
/// discard with "Discard failed" while nothing was discarded is strictly
/// worse than proceeding and warning. `create` is lazy: a disabled flag
/// never attempts the checkpoint at all.
fn discard_checkpoint_outcome<T, E>(
    enabled: bool,
    create: impl FnOnce() -> Result<T, E>,
    render: impl FnOnce(E) -> String,
) -> Result<Option<String>, E> {
    if !enabled {
        return Ok(None);
    }
    match create() {
        Ok(_) => Ok(None),
        Err(err) => Ok(Some(render(err))),
    }
}

/// Discard local changes (files or hunks). When checkpoints are enabled
/// (`checkpoint`, default on) a checkpoint is snapshotted BEFORE any side
/// effect — a failed discard leaves the repo untouched (same ordering rule
/// as repo_clean). A checkpoint FAILURE does not abort the discard: the op
/// succeeds and returns `Ok(Some(reason))` so the FE can surface it as a
/// warning toast.
#[tauri::command(rename_all = "snake_case")]
pub async fn discard(
    repo_id: RepoId,
    targets: Vec<StageTarget>,
    checkpoint: Option<bool>,
    state: State<'_, RepoManager>,
) -> Result<Option<String>, String> {
    let handle = get_handle(&state, &repo_id)?;
    let checkpoint = checkpoint_enabled(checkpoint);
    if let Some(remote) = handle.remote() {
        // Same ordering rule as the local path: checkpoint BEFORE discard.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "discard",
            move |git, _ws| {
                let warning = discard_checkpoint_outcome(
                    checkpoint,
                    || git.checkpoint_create("discard changes"),
                    |err| err,
                )?;
                git.discard(&targets)?;
                Ok(warning)
            },
        ))
        .await;
    }
    let repo = handle.repo()?;
    finish_op(discard_op(&handle, repo, targets, checkpoint)).await
}

/// Enqueued `discard` body (op-queue mutation with the generation bump, like
/// [`enqueue_mutation`]). Split from the command so tests can drive it
/// through a real handle without `State`.
fn discard_op(
    handle: &RepoHandle,
    repo: Arc<Mutex<git2::Repository>>,
    targets: Vec<StageTarget>,
    checkpoint: bool,
) -> oneshot::Receiver<Result<Option<String>, String>> {
    let engine = handle.engine();
    let m3 = handle.m3();
    let generation = handle.generation_shared();
    // The checkpoint only needs the paths the discard can touch: a scoped
    // snapshot skips hashing the rest of the workdir (untracked files
    // included), so discarding one file in a huge dirty repo stays instant.
    let paths = target_paths(&targets);
    let (_op_id, rx) = handle.ops().enqueue("discard", move |_ctx| {
        let result = (|| -> Result<Option<String>, String> {
            // Checkpoint FIRST (when enabled): the exact pre-discard state
            // becomes an undo point. A failed checkpoint never aborts the
            // discard (see [`discard_checkpoint_outcome`]).
            let warning = {
                let guard = repo.lock();
                // The warning travels inside the SUCCESS payload — plain
                // Display text, not the transport error envelope.
                discard_checkpoint_outcome(
                    checkpoint,
                    || m3.checkpoint_create_scoped(&guard, "discard changes", &paths),
                    |err| err.to_string(),
                )
                .map_err(engine_err)?
            };
            {
                let guard = repo.lock();
                engine.discard(&guard, &targets).map_err(engine_err)?;
            }
            Ok(warning)
        })();
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        result
    });
    rx
}

/// Unique file paths across discard targets (every variant carries the path
/// it modifies). Order-stable for deterministic checkpoint messages.
fn target_paths(targets: &[StageTarget]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for target in targets {
        let path = match target {
            StageTarget::File(path) => path,
            StageTarget::Hunk { path, .. } | StageTarget::Lines { path, .. } => path,
        };
        if !out.iter().any(|seen| seen == path) {
            out.push(path.clone());
        }
    }
    out
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
                // Cancellation-aware CLI path (see fetch/push): op_cancel
                // flips the ctx token and the CLI's polled child wait
                // kills a git stuck on a hung hook instead of wedging the
                // mutation lane forever.
                let cancel = ctx.cancel_token();
                // The CLI error crosses the boundary directly (the closure
                // returns `Result<_, String>`), so it needs the envelope here
                // instead of the shared `result.map_err(engine_err)` tail.
                cli::commit_via_cli(
                    &workdir,
                    &options,
                    &mut |line| ctx.emit_log(line),
                    cancel.as_deref(),
                )
                .map_err(|e| engine_err(EngineError::cli(e)))
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

use crate::engine::types::PreviewInfo;

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

/// Verification status of one commit's signature (queued read, like the
/// other engine reads).
#[tauri::command(rename_all = "snake_case")]
pub async fn commit_signature(
    repo_id: RepoId,
    sha: String,
    state: State<'_, RepoManager>,
) -> Result<CommitSignature, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "commit",
            move |git, _ws| git.commit_signature(&sha),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "commit", move |_ctx, repo| {
        engine.commit_signature(repo, &sha)
    }))
    .await
}

/// Branches preserved under `refs/mygitui/trash/*`, newest first (read).
#[tauri::command(rename_all = "snake_case")]
pub async fn branch_trash_list(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<BranchTrashEntry>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "branch",
            move |git, _ws| git.branch_trash_list(),
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "branch", move |_ctx, repo| {
        engine.branch_trash_list(repo)
    }))
    .await
}

/// Restore a trashed branch (optionally renamed); returns the branch name.
#[tauri::command(rename_all = "snake_case")]
pub async fn branch_trash_restore(
    repo_id: RepoId,
    id: String,
    new_name: Option<String>,
    state: State<'_, RepoManager>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The remote engine's restore takes no rename; compose the rename
        // from branch_rename. The entry id is `<name>-<timestamp>`, so the
        // original name strips back from the numeric suffix (mirrors the
        // engine's own `strip_trash_suffix`).
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "branch",
            move |git, _ws| {
                git.branch_trash_restore(&id)?;
                let name = id
                    .trim_end_matches(|c: char| c.is_ascii_digit())
                    .trim_end_matches('-')
                    .to_string();
                match new_name {
                    Some(new) => {
                        git.branch_rename(&name, &new)?;
                        Ok(new)
                    }
                    None => Ok(name),
                }
            },
        ))
        .await;
    }
    let engine = handle.m12();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "branch",
        move |_ctx, repo| engine.branch_trash_restore(repo, &id, new_name.as_deref()),
    ))
    .await
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
    let cancelled = handle.ops().cancel_op(repo_id.0.as_str(), &op_id);
    // A cancelled op's auth prompt must not keep its handshake thread
    // waiting for the FE (or its 60 s timeout) — wake it now (registry
    // #4). No-op when the op had no pending request.
    crate::auth::broker().cancel_pending(&op_id);
    Ok(cancelled)
}

// ---------------------------------------------------------------------------
// M4 (lane F4): git clean execution + custom shell actions (contracts.md
// "Commands (M4)").
//
// `repo_clean` deletes a caller-supplied list of workdir-relative untracked
// paths (as computed by `ops_preview` kind "clean"). Safety chain: every
// path is validated first (relative, no `..`, inside the repo root — one
// bad path aborts the whole op with zero side effects), then a checkpoint
// of the full workdir state is created (reason argument, FE passes
// "pre-clean"; skipped when the checkpoint flag is false), then the paths
// are removed. Missing paths are skipped; symlinks are unlinked, never
// traversed. Returns the count of removed paths. It is an op-queue
// mutation: serialized with other mutations and bumps the generation after
// (the watcher-driven clients resync).
//
// ---------------------------------------------------------------------------

/// git clean execution: checkpoint first (unless `checkpoint` is false —
/// the FE passes the global "checkpoints for git changes" setting), then
/// delete each validated path. Returns the number of paths removed.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_clean(
    repo_id: RepoId,
    paths: Vec<String>,
    checkpoint_reason: String,
    checkpoint: Option<bool>,
    state: State<'_, RepoManager>,
) -> Result<u32, String> {
    let handle = get_handle(&state, &repo_id)?;
    let checkpoint = checkpoint_enabled(checkpoint);
    if let Some(remote) = handle.remote() {
        // Same ordering rule as the local clean: validate FIRST (purely
        // lexical checks on the workdir-relative paths, so a rejected op
        // has zero side effects — notably no checkpoint), then checkpoint,
        // then delete. The device's `git clean` refuses pathspecs that
        // escape the repository, mirroring the local resolve guard.
        validate_clean_paths(&handle.root, &paths)?;
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "clean",
            move |git, _ws| {
                if checkpoint {
                    git.checkpoint_create(&checkpoint_reason)?;
                }
                git.repo_clean(&paths)
            },
        ))
        .await;
    }
    let repo = handle.repo()?;
    finish_op(clean_op(
        &handle,
        repo,
        paths,
        checkpoint_reason,
        checkpoint,
    ))
    .await
}

/// Enqueued `clean` body (op-queue mutation with the generation bump, like
/// [`enqueue_mutation`]). Split from the command so tests can drive it
/// through a real handle without `State`. `checkpoint` off skips the
/// pre-clean undo point (the FE's "checkpoints for git changes" setting).
fn clean_op(
    handle: &RepoHandle,
    repo: Arc<Mutex<git2::Repository>>,
    paths: Vec<String>,
    checkpoint_reason: String,
    checkpoint: bool,
) -> oneshot::Receiver<Result<u32, String>> {
    let engine = handle.m3();
    let root = handle.root.clone();
    let generation = handle.generation_shared();
    let (_op_id, rx) = handle.ops().enqueue("clean", move |_ctx| {
        let result = (|| -> Result<u32, String> {
            // Validate FIRST so a rejected op has zero side effects —
            // notably no checkpoint.
            let resolved = validate_clean_paths(&root, &paths)?;
            // SAFETY SECOND: the exact pre-clean state becomes an undo
            // point (checkpoint refs never move HEAD/branches, status
            // untouched), scoped to the paths being removed so a huge
            // untracked tree doesn't get hashed for a one-file clean.
            // Skipped when checkpoints are disabled.
            if checkpoint {
                let guard = repo.lock();
                engine
                    .checkpoint_create_scoped(&guard, &checkpoint_reason, &paths)
                    .map_err(engine_err)?;
            }
            remove_resolved(&resolved)
        })();
        generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        result
    });
    rx
}

/// Validation half of the clean deletion: every path must be
/// workdir-relative, non-empty, free of `..` and resolve inside `root`
/// (shared rules via `workspace::validate_rel_path`). One bad path fails
/// the whole op before anything is touched.
fn validate_clean_paths(root: &Path, paths: &[String]) -> Result<Vec<std::path::PathBuf>, String> {
    let mut resolved = Vec::with_capacity(paths.len());
    for path in paths {
        if path.trim().is_empty() || path.trim() == "." {
            return Err(format!(
                "clean path must be a non-empty workdir-relative path: `{path}`"
            ));
        }
        let full = crate::workspace::validate_rel_path(root, path)?;
        // Explicit root guard: "." forms resolve onto the repo root.
        if full == root {
            return Err(format!("clean path escapes the repository: `{path}`"));
        }
        resolved.push(full);
    }
    Ok(resolved)
}

/// Deletion half of the clean deletion: remove already-validated
/// paths. A missing path (vanished between preview and confirm) is
/// skipped; a symlink is unlinked, never traversed. Returns the count of
/// paths removed.
fn remove_resolved(resolved: &[std::path::PathBuf]) -> Result<u32, String> {
    let mut removed = 0u32;
    for full in resolved {
        // symlink_metadata does not follow links: a symlinked "directory"
        // goes through remove_file below instead of remove_dir_all.
        let Ok(meta) = std::fs::symlink_metadata(full) else {
            continue; // gone already — nothing to clean
        };
        let result = if meta.is_dir() {
            std::fs::remove_dir_all(full)
        } else {
            std::fs::remove_file(full)
        };
        result.map_err(|e| format!("removing `{}` failed: {e}", full.display()))?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod m4_clean_tests {
    use super::*;
    use crate::engine::libgit2::Libgit2Engine;

    fn temp_repo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-clean-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        // Clean writes a checkpoint commit, which needs a committer identity.
        // CI runners have no global git config, so set it locally (same as the
        // other engine test helpers).
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Clean Test")?;
                c.set_str("user.email", "clean@test.local")
            })
            .unwrap();
        dir
    }

    #[test]
    fn clean_removal_removes_files_dirs_and_counts() {
        let root = temp_repo("del");
        std::fs::write(root.join("loose.txt"), b"x").unwrap();
        std::fs::create_dir_all(root.join("nested/deep")).unwrap();
        std::fs::write(root.join("nested/deep/a.txt"), b"x").unwrap();
        std::fs::write(root.join("nested/b.txt"), b"x").unwrap();

        let resolved = validate_clean_paths(
            &root,
            &["loose.txt".into(), "nested".into(), "missing.txt".into()],
        )
        .unwrap();
        let removed = remove_resolved(&resolved).unwrap();
        assert_eq!(removed, 2, "missing paths are skipped, not counted");
        assert!(!root.join("loose.txt").exists());
        assert!(!root.join("nested").exists());
        assert!(root.join(".git").exists(), "repo itself untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_validation_rejects_escaping_paths_before_touching_disk() {
        let root = temp_repo("escape");
        std::fs::write(root.join("keep.txt"), b"x").unwrap();
        let outside = root.parent().unwrap().join("outside-target.txt");
        std::fs::write(&outside, b"x").unwrap();

        let bad: Vec<String> = vec![
            "../outside-target.txt".into(),
            "a/../../b".into(),
            std::fs::canonicalize(&outside)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            String::new(),
            ".".into(),
        ];
        for path in bad {
            let err = validate_clean_paths(&root, std::slice::from_ref(&path)).unwrap_err();
            assert!(
                err.contains("relative") || err.contains("..") || err.contains("escapes"),
                "`{path}` rejected with a clear reason, got: {err}"
            );
        }
        assert!(root.join("keep.txt").exists(), "nothing deleted on reject");
        assert!(outside.exists(), "outside target untouched");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn clean_removal_unlinks_symlinks_without_traversing() {
        let root = temp_repo("symlink");
        let target_dir =
            std::env::temp_dir().join(format!("mygitui-clean-target-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&target_dir);
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("precious.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(&target_dir, root.join("linked")).unwrap();

        let resolved = validate_clean_paths(&root, &["linked".into()]).unwrap();
        let removed = remove_resolved(&resolved).unwrap();
        assert_eq!(removed, 1);
        assert!(!root.join("linked").exists(), "link removed");
        assert!(target_dir.join("precious.txt").exists(), "target survived");
        let _ = std::fs::remove_dir_all(&target_dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// End-to-end through the op queue: checkpoint lands first, files are
    /// gone, generation bumps, and the checkpoint is listed afterwards.
    #[test]
    fn clean_op_checkpoints_then_deletes_and_bumps_generation() {
        let root = temp_repo("e2e");
        std::fs::write(root.join("junk.log"), b"x").unwrap();
        std::fs::write(root.join("keep.txt"), b"x").unwrap();

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&root).unwrap();
        let handle = manager.get(&info.repo_id).unwrap();
        let before = handle.generation();

        let removed = clean_op(
            &handle,
            handle.repo().expect("local repo"),
            vec!["junk.log".to_string()],
            "pre-clean".to_string(),
            true,
        )
        .blocking_recv()
        .expect("queue alive")
        .expect("clean succeeds");
        assert_eq!(removed, 1);
        assert!(!root.join("junk.log").exists());
        assert!(root.join("keep.txt").exists());
        assert_eq!(handle.generation(), before + 1, "generation bumps");

        // The undo point exists and names the reason.
        let engine = Libgit2Engine::new();
        let checkpoints = {
            let repo = handle.repo().expect("local repo");
            let repo = repo.lock();
            engine.checkpoints_impl(&repo).unwrap()
        };
        assert!(
            checkpoints.iter().any(|cp| cp.reason == "pre-clean"),
            "pre-clean checkpoint present: {checkpoints:?}"
        );

        manager.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_op_rejects_escaping_paths_without_checkpoint_or_deletion() {
        let root = temp_repo("reject");
        std::fs::write(root.join("junk.log"), b"x").unwrap();

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&root).unwrap();
        let handle = manager.get(&info.repo_id).unwrap();

        let err = clean_op(
            &handle,
            handle.repo().expect("local repo"),
            vec!["../evil".to_string()],
            "pre-clean".to_string(),
            true,
        )
        .blocking_recv()
        .expect("queue alive")
        .unwrap_err();
        assert!(err.contains(".."), "got: {err}");
        assert!(root.join("junk.log").exists(), "file survived the reject");

        // No checkpoint was taken for the rejected op.
        let engine = Libgit2Engine::new();
        let checkpoints = {
            let repo = handle.repo().expect("local repo");
            let repo = repo.lock();
            engine.checkpoints_impl(&repo).unwrap()
        };
        assert!(
            !checkpoints.iter().any(|cp| cp.reason == "pre-clean"),
            "no checkpoint on rejected clean: {checkpoints:?}"
        );

        manager.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Checkpoint flag off: the clean still runs, but no pre-clean undo
    /// point is written (the FE's "checkpoints for git changes" setting).
    #[test]
    fn clean_op_with_checkpoint_false_skips_the_checkpoint() {
        let root = temp_repo("no-ckpt");
        std::fs::write(root.join("junk.log"), b"x").unwrap();

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&root).unwrap();
        let handle = manager.get(&info.repo_id).unwrap();

        let removed = clean_op(
            &handle,
            handle.repo().expect("local repo"),
            vec!["junk.log".to_string()],
            "pre-clean".to_string(),
            false,
        )
        .blocking_recv()
        .expect("queue alive")
        .expect("clean succeeds");
        assert_eq!(removed, 1);
        assert!(!root.join("junk.log").exists(), "clean still ran");

        let engine = Libgit2Engine::new();
        let checkpoints = {
            let repo = handle.repo().expect("local repo");
            let repo = repo.lock();
            engine.checkpoints_impl(&repo).unwrap()
        };
        assert!(
            !checkpoints.iter().any(|cp| cp.reason == "pre-clean"),
            "no checkpoint when disabled: {checkpoints:?}"
        );

        manager.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&root);
    }
}

// ---------------------------------------------------------------------------
// discard: checkpoint flag + recoverable checkpoint step.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod discard_tests {
    use super::*;
    use crate::engine::libgit2::Libgit2Engine;

    /// Repo with a committed tracked file, opened through the manager (same
    /// scaffolding as `m4_clean_tests::temp_repo`). `identity` off leaves
    /// user.name/email unset so checkpoint creation fails (the real-world
    /// "sometimes discard doesn't make changes disappear" trigger); the
    /// local config is emptied so a machine-global identity cannot leak in.
    fn discard_fixture(tag: &str, identity: bool) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-discard-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        repo.config()
            .and_then(|mut c| {
                if identity {
                    c.set_str("user.name", "Discard Test")?;
                    c.set_str("user.email", "discard@test.local")?;
                } else {
                    // An empty name is rejected by signature validation
                    // deterministically, whatever the machine-global config
                    // holds (repo-local values win).
                    c.set_str("user.name", "")?;
                    c.set_str("user.email", "")?;
                }
                c.set_bool("core.autocrlf", false)
            })
            .unwrap();
        let sig = git2::Signature::now("Discard Test", "discard@test.local").unwrap();
        let mut index = repo.index().unwrap();
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        index.add_path(std::path::Path::new("a.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        dir
    }

    fn listed_checkpoints(handle: &RepoHandle) -> Vec<CheckpointInfo> {
        let engine = Libgit2Engine::new();
        let repo = handle.repo().expect("local repo");
        let repo = repo.lock();
        engine.checkpoints_impl(&repo).unwrap()
    }

    #[test]
    fn discard_checkpoint_outcome_maps_failure_to_a_warning() {
        // Absent flag = enabled (the FE default).
        assert!(checkpoint_enabled(None));
        assert!(!checkpoint_enabled(Some(false)));
        assert!(checkpoint_enabled(Some(true)));

        let failed = || -> Result<CheckpointInfo, EngineError> {
            Err(EngineError::Invalid(
                "committer identity not configured".into(),
            ))
        };
        let created = || -> Result<CheckpointInfo, EngineError> {
            Ok(CheckpointInfo {
                id: "1-x".into(),
                ref_name: "refs/mygitui/checkpoints/1-x".into(),
                reason: "discard changes".into(),
                created_at: 0,
                branch: Some("main".into()),
                has_worktree_state: false,
            })
        };

        // Disabled: skipped — the checkpoint step is never attempted.
        let attempted = std::cell::Cell::new(false);
        assert_eq!(
            discard_checkpoint_outcome(
                false,
                || {
                    attempted.set(true);
                    failed()
                },
                |err| err.to_string()
            )
            .unwrap(),
            None
        );
        assert!(
            !attempted.get(),
            "a disabled flag must not touch the checkpoint engine"
        );

        // Enabled + failure: the reason becomes the warning, not an error.
        let warning = discard_checkpoint_outcome(true, failed, |err| err.to_string()).unwrap();
        assert!(
            warning
                .as_deref()
                .is_some_and(|w| w.contains("committer identity")),
            "warning carries the reason: {warning:?}"
        );

        // Enabled + success: no warning.
        assert_eq!(
            discard_checkpoint_outcome(true, created, |err| err.to_string()).unwrap(),
            None
        );
    }

    #[test]
    fn discard_op_default_creates_a_checkpoint_and_discards() {
        let root = discard_fixture("ckpt", true);
        std::fs::write(root.join("a.txt"), b"two\n").unwrap();

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&root).unwrap();
        let handle = manager.get(&info.repo_id).unwrap();
        let before = handle.generation();

        // checkpoint_enabled(Some(true)) is the absent-flag default.
        let warning = discard_op(
            &handle,
            handle.repo().expect("local repo"),
            vec![StageTarget::File("a.txt".to_string())],
            checkpoint_enabled(None),
        )
        .blocking_recv()
        .expect("queue alive")
        .expect("discard succeeds");
        assert_eq!(warning, None, "no warning when the checkpoint works");
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\n",
            "workdir back to HEAD"
        );
        assert_eq!(handle.generation(), before + 1, "generation bumps");
        assert!(
            listed_checkpoints(&handle)
                .iter()
                .any(|cp| cp.reason == "discard changes"),
            "undo point exists: {checkpoints:?}",
            checkpoints = listed_checkpoints(&handle)
        );

        manager.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn discard_op_with_checkpoint_false_skips_the_checkpoint() {
        let root = discard_fixture("off", true);
        std::fs::write(root.join("a.txt"), b"two\n").unwrap();

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&root).unwrap();
        let handle = manager.get(&info.repo_id).unwrap();

        let warning = discard_op(
            &handle,
            handle.repo().expect("local repo"),
            vec![StageTarget::File("a.txt".to_string())],
            false,
        )
        .blocking_recv()
        .expect("queue alive")
        .expect("discard succeeds");
        assert_eq!(warning, None);
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\n",
            "discard still ran"
        );
        assert!(
            !listed_checkpoints(&handle)
                .iter()
                .any(|cp| cp.reason == "discard changes"),
            "no undo point when disabled"
        );

        manager.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The reported bug: without user.name/user.email the checkpoint used
    /// to abort the whole discard ("Discard failed: …") while the working
    /// copy kept the changes. Now the discard proceeds and the failure
    /// comes back as a warning.
    #[test]
    fn discard_op_survives_a_failed_checkpoint_and_still_discards() {
        let root = discard_fixture("no-identity", false);
        std::fs::write(root.join("a.txt"), b"two\n").unwrap();

        let manager = RepoManager::new();
        let info = manager.open_unwatched(&root).unwrap();
        let handle = manager.get(&info.repo_id).unwrap();

        let outcome = discard_op(
            &handle,
            handle.repo().expect("local repo"),
            vec![StageTarget::File("a.txt".to_string())],
            true,
        )
        .blocking_recv()
        .expect("queue alive");
        let warning = outcome.expect("discard succeeds despite the failed checkpoint");
        assert!(
            warning
                .as_ref()
                .is_some_and(|w| w.contains("committer identity")),
            "warning names the checkpoint failure: {warning:?}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\n",
            "the changes are actually gone"
        );
        assert!(
            !listed_checkpoints(&handle)
                .iter()
                .any(|cp| cp.reason == "discard changes"),
            "no undo point was written"
        );

        manager.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&root);
    }
}
