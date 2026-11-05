use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::engine::git_engine::EngineError;
use crate::engine::types::{LfsStatus, RepoHealth, RepoId, SparseInfo};
use crate::repo::RepoHandle;
use crate::service::{
    engine_err, enqueue_mutation, enqueue_remote_mutation, enqueue_remote_read, finish_op,
    get_handle, AppServices,
};

// ---------- M11: repo health / maintenance / sparse / LFS / archive ----------

/// Health snapshot (fs-only, fast; safe to poll from a panel).
pub async fn repo_health(services: &AppServices, repo_id: RepoId) -> Result<RepoHealth, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
    crate::rt::spawn_blocking(move || {
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
pub async fn maintenance_run(
    services: &AppServices,
    repo_id: RepoId,
    op: String,
) -> Result<String, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
                        .map_err(EngineError::Invalid);
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
                        .map_err(EngineError::Invalid);
                }
                other => {
                    return Err(EngineError::Invalid(format!(
                        "unknown maintenance op `{other}`"
                    )))
                }
            };
            let args = parsed.args();
            crate::maintenance::git_run(workdir, args).map_err(EngineError::Invalid)
        },
    ))
    .await
}

/// Arguments reach `git archive` as argv entries (never a shell), but the
/// service layer still applies a metacharacter guard on `spec`/`destination`:
/// reject shell metacharacters, control characters, and option-shaped
/// values (a leading `-` would be parsed as a flag).
fn archive_arg_ok(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.chars().any(|c| {
            matches!(
                c,
                ';' | '&'
                    | '|'
                    | '$'
                    | '`'
                    | '('
                    | ')'
                    | '<'
                    | '>'
                    | '"'
                    | '\''
                    | '\n'
                    | '\r'
                    | '\0'
            )
        })
}

/// `git archive --format <zip|tar|tar.gz> <ref> -o <destination>`.
pub async fn archive(
    services: &AppServices,
    repo_id: RepoId,
    spec: String,
    format: String,
    destination: String,
) -> Result<(), String> {
    if !["zip", "tar", "tar.gz"].contains(&format.as_str()) {
        return Err(format!("unsupported archive format `{format}`"));
    }
    if !archive_arg_ok(&spec) || !archive_arg_ok(&destination) {
        return Err("invalid archive arguments".into());
    }
    let handle = get_handle(&services.repos, &repo_id)?;
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
            .map_err(EngineError::Invalid)
        },
    ))
    .await
}

/// Sparse-checkout state.
pub async fn sparse_info(services: &AppServices, repo_id: RepoId) -> Result<SparseInfo, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
    crate::rt::spawn_blocking(move || {
        Ok(crate::maintenance::sparse_info(std::path::Path::new(&root)))
    })
    .await
    .map_err(|e| format!("sparse_info task failed: {e}"))?
}

/// Applies sparse-checkout patterns (cone mode; empty = full checkout).
pub async fn sparse_apply(
    services: &AppServices,
    repo_id: RepoId,
    patterns: Vec<String>,
    add: bool,
) -> Result<(), String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
                .map_err(EngineError::Invalid)
        },
    ))
    .await
}

/// `git lfs` availability + tracked patterns.
pub async fn lfs_status(services: &AppServices, repo_id: RepoId) -> Result<LfsStatus, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
    crate::rt::spawn_blocking(move || {
        Ok(crate::maintenance::lfs_status(std::path::Path::new(&root)))
    })
    .await
    .map_err(|e| format!("lfs_status task failed: {e}"))?
}

/// Runs `git lfs pull|push|install|fetch`.
pub async fn lfs_run(
    services: &AppServices,
    repo_id: RepoId,
    subcommand: String,
) -> Result<String, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
                .map_err(EngineError::Invalid)
        },
    ))
    .await
}

/// Blobless (partial) clone via the git CLI: `git clone --filter=blob:none`.
/// The CLI child can't be cancelled mid-run, so `cancel` is checked before
/// spawning and the result reports cancelled if the flag flipped while it
/// ran (the abandoned partial directory stays for cleanup).
pub async fn clone_blobless(
    _services: &AppServices,
    url: String,
    destination: String,
    depth: Option<u32>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<String, String> {
    if url.contains(';') || destination.is_empty() {
        return Err("invalid clone arguments".into());
    }
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("clone cancelled".into());
    }
    let parent = std::path::Path::new(&destination)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    crate::rt::spawn_blocking(move || {
        let mut args: Vec<String> = vec![
            "clone".into(),
            "--filter=blob:none".into(),
            url.clone(),
            destination.clone(),
        ];
        if let Some(depth) = depth {
            args.insert(2, format!("--depth={depth}"));
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = crate::maintenance::git_run(&parent, &arg_refs);
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("clone cancelled".into());
        }
        result?;
        Ok(destination)
    })
    .await
    .map_err(|e| format!("clone_blobless task failed: {e}"))?
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
// "pre-clean"), then the paths are removed. Missing paths are skipped;
// symlinks are unlinked, never traversed. Returns the count of removed
// paths. It is an op-queue mutation: serialized with other mutations and
// bumps the generation after (the watcher-driven clients resync).
//
// `action_run` / `action_cancel` delegate to the `actions` runner; the
// registry is process-global managed state (see actions.rs).
// ---------------------------------------------------------------------------

/// git clean execution: checkpoint first, then delete each validated path.
/// Returns the number of paths removed.
pub async fn repo_clean(
    services: &AppServices,
    repo_id: RepoId,
    paths: Vec<String>,
    checkpoint_reason: String,
) -> Result<u32, String> {
    let handle = get_handle(&services.repos, &repo_id)?;
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
                git.checkpoint_create(&checkpoint_reason)?;
                git.repo_clean(&paths)
            },
        ))
        .await;
    }
    let repo = handle.repo()?;
    finish_op(clean_op(&handle, repo, paths, checkpoint_reason)).await
}

/// Enqueued `clean` body (op-queue mutation with the generation bump, like
/// [`enqueue_mutation`]). Split from the command so tests can drive it
/// through a real handle without `State`.
fn clean_op(
    handle: &RepoHandle,
    repo: Arc<Mutex<git2::Repository>>,
    paths: Vec<String>,
    checkpoint_reason: String,
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
            // untouched).
            {
                let guard = repo.lock();
                engine
                    .checkpoint_create(&guard, &checkpoint_reason)
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
    use crate::repo::RepoManager;

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
}
