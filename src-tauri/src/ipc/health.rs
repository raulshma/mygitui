//! Maintenance/health lane: `repo_health` probes, `maintenance_run` (gc /
//! prune / commit-graph / pack-refs / count-objects / fsck), archive +
//! bundle exports, sparse-checkout, and `git lfs` commands. CLI-heavy: the
//! arg guards live next to the other CLI-lane guards in `maintenance.rs`
//! (`archive_arg_ok` / `clone_args_ok` are thin aliases).

use tauri::State;

use crate::engine::git_engine::EngineError;
use crate::engine::types::{LfsLock, LfsStatus, RepoHealth, RepoId, SparseInfo};
use crate::repo::RepoManager;

use super::{
    enqueue_mutation, enqueue_remote_mutation, enqueue_remote_read, finish_op, get_handle,
};

// ---------- M11: repo health / maintenance / sparse / LFS / archive ----------

/// Health snapshot (fs-only, fast; safe to poll from a panel).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_health(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<RepoHealth, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The local probe walks the fs at `root`; remote repos probe on the
        // device through the engine instead.
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "health",
            move |git, _ws| git.repo_health(),
        ))
        .await;
    }
    let root = handle.root.clone();
    let git_dir = handle.git_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        Ok(crate::maintenance::repo_health(
            std::path::Path::new(&root),
            std::path::Path::new(&git_dir),
        ))
    })
    .await
    .map_err(|e| format!("repo_health task failed: {e}"))?
}

/// Runs one maintenance op on the op queue (`gc`, `prune`, `commit_graph`,
/// `pack_refs`, `count_objects`); resolves with the command output.
#[tauri::command(rename_all = "snake_case")]
pub async fn maintenance_run(
    repo_id: RepoId,
    op: String,
    state: State<'_, RepoManager>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Same op set as local; the remote engine runs each one on the
        // device and returns a matching summary string.
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "maintenance",
            move |git, _ws| git.maintenance_run(&op),
        ))
        .await;
    }
    let root = handle.root.clone();
    let git_dir = handle.git_dir.clone();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "maintenance",
        move |_ctx, _repo| {
            let workdir = std::path::Path::new(&root);
            let parsed = match op.as_str() {
                "gc" => crate::maintenance::MaintenanceOp::Gc,
                "prune" => crate::maintenance::MaintenanceOp::Prune,
                "commit_graph" => crate::maintenance::MaintenanceOp::CommitGraph,
                "pack_refs" => crate::maintenance::MaintenanceOp::PackRefs,
                // Exact object counts (parses count-objects -v into a summary).
                "count_objects" => {
                    return crate::maintenance::count_objects(workdir)
                        .map(|(loose, packed_kib)| {
                            format!("loose objects: {loose}, packed size: {packed_kib} KiB")
                        })
                        .map_err(EngineError::cli);
                }
                // M12: dangling-object census — needs stderr capture, so it
                // bypasses plain `git_run` and writes the fsck.json cache.
                "fsck" => {
                    return crate::maintenance::fsck_run(workdir, &git_dir)
                        .map(|(dangling, samples)| {
                            format!(
                                "dangling objects: {dangling}{}",
                                if samples.is_empty() {
                                    String::new()
                                } else {
                                    format!(" (e.g. {})", samples.first().unwrap_or(&String::new()))
                                }
                            )
                        })
                        .map_err(EngineError::cli);
                }
                other => {
                    return Err(EngineError::Invalid(format!(
                        "unknown maintenance op `{other}`"
                    )))
                }
            };
            let args = parsed.args();
            crate::maintenance::git_run(workdir, args).map_err(EngineError::cli)
        },
    ))
    .await
}

/// Arguments reach `git archive` as argv entries (never a shell), but the
/// IPC contract promises a metacharacter guard on `spec`/`destination`:
/// reject shell metacharacters, control characters, and option-shaped
/// values (a leading `-` would be parsed as a flag). Lives in
/// `maintenance` so every CLI lane guards through one home.
fn archive_arg_ok(value: &str) -> bool {
    crate::maintenance::cli_arg_ok(value)
}

/// `git archive --format <zip|tar|tar.gz> <ref> -o <destination>`.
#[tauri::command(rename_all = "snake_case")]
pub async fn archive(
    repo_id: RepoId,
    spec: String,
    format: String,
    destination: String,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    if !["zip", "tar", "tar.gz"].contains(&format.as_str()) {
        return Err(format!("unsupported archive format `{format}`"));
    }
    if !archive_arg_ok(&spec) || !archive_arg_ok(&destination) {
        return Err("invalid archive arguments".into());
    }
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Remote archive streams the bytes over SSH and writes the same
        // local destination file on this machine.
        let destination_path = std::path::PathBuf::from(&destination);
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "archive",
            move |git, _ws| git.archive(&spec, &format, &destination_path),
        ))
        .await;
    }
    let root = handle.root.clone();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "archive",
        move |_ctx, _repo| {
            crate::maintenance::git_run(
                std::path::Path::new(&root),
                &["archive", "--format", &format, &spec, "-o", &destination],
            )
            .map(|_| ())
            .map_err(EngineError::cli)
        },
    ))
    .await
}

/// `git bundle create <out_path> (--all | refs...)` (Phase 5): a portable
/// packfile of the selected refs, written to a local file. Mirror of
/// [`archive`]: same arg-guard family (`archive_arg_ok` via
/// `maintenance::bundle_create`), and the remote lane streams the bundle
/// bytes over SSH into the same local destination file.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_bundle(
    repo_id: RepoId,
    out_path: String,
    refs: Vec<String>,
    all: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    // Pre-queue guards mirroring `archive` (the same rules also run inside
    // maintenance::bundle_create, so direct engine callers are covered too).
    if !archive_arg_ok(&out_path) || refs.iter().any(|r| !archive_arg_ok(r)) {
        return Err("invalid bundle arguments".into());
    }
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // Remote bundle: `git bundle create -` on the device, bytes written
        // to this machine's destination (like the archive lane).
        let destination_path = std::path::PathBuf::from(&out_path);
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "bundle",
            move |git, _ws| git.bundle(&destination_path, &refs, all),
        ))
        .await;
    }
    let root = handle.root.clone();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "bundle",
        move |_ctx, _repo| {
            crate::maintenance::bundle_create(std::path::Path::new(&root), &out_path, &refs, all)
                .map_err(EngineError::cli)
        },
    ))
    .await
}

/// Sparse-checkout state.
#[tauri::command(rename_all = "snake_case")]
pub async fn sparse_info(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<SparseInfo, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The local probe reads the fs at `root`; remote repos ask the device.
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "sparse",
            move |git, _ws| git.sparse_info(),
        ))
        .await;
    }
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        Ok(crate::maintenance::sparse_info(std::path::Path::new(&root)))
    })
    .await
    .map_err(|e| format!("sparse_info task failed: {e}"))?
}

/// Applies sparse-checkout patterns (cone mode; empty = full checkout).
#[tauri::command(rename_all = "snake_case")]
pub async fn sparse_apply(
    repo_id: RepoId,
    patterns: Vec<String>,
    add: bool,
    state: State<'_, RepoManager>,
) -> Result<(), String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "sparse",
            move |git, _ws| git.sparse_apply(&patterns, add),
        ))
        .await;
    }
    let root = handle.root.clone();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "sparse",
        move |_ctx, _repo| {
            crate::maintenance::sparse_apply(std::path::Path::new(&root), &patterns, add)
                .map_err(EngineError::cli)
        },
    ))
    .await
}

/// `git lfs` availability + tracked patterns.
#[tauri::command(rename_all = "snake_case")]
pub async fn lfs_status(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<LfsStatus, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The local probe reads the fs at `root`; remote repos ask the device.
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "lfs",
            move |git, _ws| git.lfs_status(),
        ))
        .await;
    }
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        Ok(crate::maintenance::lfs_status(std::path::Path::new(&root)))
    })
    .await
    .map_err(|e| format!("lfs_status task failed: {e}"))?
}

/// Runs `git lfs pull|push|install|fetch`.
#[tauri::command(rename_all = "snake_case")]
pub async fn lfs_run(
    repo_id: RepoId,
    subcommand: String,
    state: State<'_, RepoManager>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        // The device's own git-lfs answers (it allows fetch/pull/push/
        // checkout; `install` fails there with the engine's unsupported
        // subcommand error).
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "lfs",
            move |git, _ws| git.lfs_run(&subcommand),
        ))
        .await;
    }
    let root = handle.root.clone();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "lfs",
        move |_ctx, _repo| {
            crate::maintenance::lfs_run(std::path::Path::new(&root), &subcommand)
                .map_err(EngineError::cli)
        },
    ))
    .await
}

/// `git lfs locks` — list the repo's file locks (Phase 5). Parsed from
/// `--json` (plain-text fallback); requires git-lfs, like every lfs lane.
#[tauri::command(rename_all = "snake_case")]
pub async fn lfs_locks(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<LfsLock>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "lfs",
            move |git, _ws| git.lfs_locks(),
        ))
        .await;
    }
    let root = handle.root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::maintenance::lfs_locks(std::path::Path::new(&root))
    })
    .await
    .map_err(|e| format!("lfs_locks task failed: {e}"))?
}

/// `git lfs lock <path>` — lock a file (Phase 5); resolves with git-lfs's
/// output line. The path guard lives in the maintenance lane (option-shaped
/// values would be parsed as git flags).
#[tauri::command(rename_all = "snake_case")]
pub async fn lfs_lock(
    repo_id: RepoId,
    path: String,
    state: State<'_, RepoManager>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "lfs",
            move |git, _ws| git.lfs_lock(&path),
        ))
        .await;
    }
    let root = handle.root.clone();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "lfs",
        move |_ctx, _repo| {
            crate::maintenance::lfs_lock(std::path::Path::new(&root), &path)
                .map_err(EngineError::cli)
        },
    ))
    .await
}

/// `git lfs unlock <path>` — release a lock (Phase 5).
#[tauri::command(rename_all = "snake_case")]
pub async fn lfs_unlock(
    repo_id: RepoId,
    path: String,
    state: State<'_, RepoManager>,
) -> Result<String, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_mutation(
            &handle,
            remote,
            "lfs",
            move |git, _ws| git.lfs_unlock(&path),
        ))
        .await;
    }
    let root = handle.root.clone();
    let repo = handle.repo()?;
    finish_op(enqueue_mutation(
        &handle,
        &repo,
        "lfs",
        move |_ctx, _repo| {
            crate::maintenance::lfs_unlock(std::path::Path::new(&root), &path)
                .map_err(EngineError::cli)
        },
    ))
    .await
}
