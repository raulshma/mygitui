//! Repo lifecycle (open/close) and working-copy status/refs reads.
//!
//! These are the per-tab baseline commands: `repo_open`/`repo_close` own the
//! [`crate::repo::RepoManager`] lifecycle (watcher + stream + pty teardown),
//! while `repo_status`/`repo_refs` are queued reads on the repo's read lane
//! (see the dispatch rule in [`super`]).

use std::path::Path;

use tauri::State;

use crate::engine::types::{RepoId, RepoInfo, RepoStatus, RepoSummary, RepoSummaryCommit};
use crate::repo::{default_engine, normalize_root, RepoManager};

use super::{engine_err, enqueue_read, enqueue_remote_read, finish_op, get_handle};

/// Open (or reuse) the repository at/above `path` and start its watcher.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_open(
    path: String,
    app: tauri::AppHandle,
    state: State<'_, RepoManager>,
) -> Result<RepoInfo, String> {
    state.open(&app, Path::new(&path)).map_err(engine_err)
}

/// Close a repository: stop its watcher, cancel its streams, drop caches,
/// and kill the repo's terminal sessions (backend backstop — the FE
/// normally `pty_kill`s its terminal panels itself). Async: the watcher
/// teardown joins the debounce thread, which can wait out an in-flight
/// emit — that must not occupy the main thread.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_close(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
    registry: State<'_, crate::pty::PtyRegistry>,
) -> Result<(), String> {
    // The session root must be captured before the handle leaves the map.
    let root = state.get(&repo_id).map(|handle| handle.root.clone());
    state.close(&repo_id);
    if let Some(root) = root {
        crate::pty::kill_sessions_for_root(&registry, &root);
    }
    Ok(())
}

/// Working-copy status. `paths` (repo-relative, from a watcher batch's
/// `paths`) gates an incremental refresh: only those pathspecs are rescanned
/// and merged into the cached full snapshot; absent paths (or any fallback
/// condition — stale cache, git-dir paths, large touched set) trigger a full
/// rescan. Same `RepoStatus` shape and ordering either way; remote repos
/// always rescan fully (see `RepoHandle::status` for the exact rules).
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_status(
    repo_id: RepoId,
    paths: Option<Vec<String>>,
    state: State<'_, RepoManager>,
) -> Result<RepoStatus, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "status",
            move |git, _ws| git.status(),
        ))
        .await;
    }
    let repo = handle.repo()?;
    let status_handle = handle.clone();
    finish_op(enqueue_read(&handle, &repo, "status", move |_ctx, repo| {
        status_handle.status(repo, paths.as_deref())
    }))
    .await
}

/// Local + remote branches and tags: `(ref name, target sha)` pairs.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_refs(
    repo_id: RepoId,
    state: State<'_, RepoManager>,
) -> Result<Vec<(String, String)>, String> {
    let handle = get_handle(&state, &repo_id)?;
    if let Some(remote) = handle.remote() {
        return finish_op(enqueue_remote_read(
            &handle,
            remote,
            "refs",
            move |git, _ws| git.refs(),
        ))
        .await;
    }
    let engine = handle.engine();
    let repo = handle.repo()?;
    finish_op(enqueue_read(&handle, &repo, "refs", move |_ctx, repo| {
        engine.refs(repo)
    }))
    .await
}

// -- Overview (repo_summary) -------------------------------------------------
//
// The Overview scans repos that have NO open tab, so these reads must not
// create sessions or watchers. `repo_summary` opens the repository
// transiently (git2), computes one status + the HEAD commit, and drops the
// handle again. The FE caps concurrency itself; each call is independent.

/// Shared body of `repo_summary` (sync, git2 — callers run it off the main
/// thread). Reuses the engine's `status` (the same code path `repo_status`
/// serves open repos with), then folds the entry kinds into the row counts.
fn summarize(path: &Path) -> Result<RepoSummary, String> {
    let repo = git2::Repository::discover(path).map_err(|e| e.to_string())?;
    if repo.is_bare() {
        return Err(format!(
            "bare repositories are not supported: {}",
            repo.path().display()
        ));
    }
    let workdir = repo
        .workdir()
        .ok_or_else(|| "repository has no workdir".to_string())?;
    // Trim one trailing separator (same rule as `repo::normalize_root`).
    let root = normalize_root(workdir.to_path_buf());
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());

    let status = default_engine()
        .status(&repo, None)
        .map_err(|e| e.to_string())?;
    let mut conflicts = 0u32;
    let mut staged = 0u32;
    let mut unstaged = 0u32;
    let mut untracked = 0u32;
    for entry in &status.entries {
        if entry.index == crate::engine::types::ChangeKind::Conflicted {
            conflicts += 1;
            continue;
        }
        if entry.index != crate::engine::types::ChangeKind::Unmodified {
            staged += 1;
        }
        if entry.worktree == crate::engine::types::ChangeKind::Untracked {
            untracked += 1;
        } else if entry.worktree != crate::engine::types::ChangeKind::Unmodified {
            unstaged += 1;
        }
    }

    let last_commit = repo
        .head()
        .ok()
        .and_then(|head| head.target())
        .and_then(|oid| {
            let commit = repo.find_commit(oid).ok()?;
            Some(RepoSummaryCommit {
                sha: oid.to_string(),
                summary: commit
                    .summary()
                    .ok()
                    .flatten()
                    .unwrap_or_default()
                    .to_owned(),
                timestamp: commit.time().seconds(),
            })
        });

    Ok(RepoSummary {
        root: root.to_string_lossy().into_owned(),
        name,
        branch: status.branch,
        detached: status.detached,
        ahead: status.ahead,
        behind: status.behind,
        conflicts,
        staged,
        unstaged,
        untracked,
        entries: status.entries,
        last_commit,
    })
}

/// Read-only Overview summary of the repository at/above `path`: opens the
/// repository transiently (NO session, NO watcher) and returns one snapshot.
/// Off the main thread: git2 is synchronous and a big worktree scan is slow.
#[tauri::command(rename_all = "snake_case")]
pub async fn repo_summary(path: String) -> Result<RepoSummary, String> {
    tauri::async_runtime::spawn_blocking(move || summarize(Path::new(&path)))
        .await
        .map_err(|e| format!("repo_summary task failed: {e}"))?
}

#[cfg(test)]
mod summary_tests {
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::engine::types::ChangeKind;

    use super::{normalize_root, summarize};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mygitui-summary-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn commit_all(repo: &git2::Repository, message: &str) {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        let tree_oid = index.write_tree().unwrap();
        index.write().unwrap();
        let sig = git2::Signature::now("t", "t@example.test").unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let parents: Vec<git2::Commit> = repo
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| repo.find_commit(oid).ok())
            .into_iter()
            .collect();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .unwrap();
    }

    #[test]
    fn clean_repo_reports_branch_counts_and_last_commit() {
        let dir = temp_dir("clean");
        let repo = git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        commit_all(&repo, "init commit");

        let summary = summarize(&dir).expect("summary");
        assert_eq!(summary.name, dir.file_name().unwrap().to_string_lossy());
        let expected_root = normalize_root(repo.workdir().unwrap().to_path_buf());
        assert_eq!(summary.root, expected_root.to_string_lossy());
        assert_eq!(summary.branch.as_deref(), Some("master"));
        assert!(!summary.detached);
        assert_eq!((summary.ahead, summary.behind), (0, 0));
        assert_eq!(
            (
                summary.conflicts,
                summary.staged,
                summary.unstaged,
                summary.untracked
            ),
            (0, 0, 0, 0)
        );
        assert!(summary.entries.is_empty());
        let last = summary.last_commit.expect("last commit");
        assert_eq!(last.summary, "init commit");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(
            (last.timestamp - now).abs() < 120,
            "{} vs {now}",
            last.timestamp
        );
        assert_eq!(last.sha.len(), 40);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dirty_repo_counts_kinds_and_lists_entries() {
        let dir = temp_dir("dirty");
        let repo = git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.join("b.txt"), "two\n").unwrap();
        commit_all(&repo, "init");

        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap(); // unstaged
        std::fs::write(dir.join("c.txt"), "new\n").unwrap(); // untracked
        {
            // Stage a third file (staged addition).
            let mut index = repo.index().unwrap();
            std::fs::write(dir.join("d.txt"), "staged\n").unwrap();
            index.add_path(Path::new("d.txt")).unwrap();
            index.write().unwrap();
        }

        let summary = summarize(&dir).expect("summary");
        assert_eq!(summary.staged, 1, "d.txt staged addition");
        assert_eq!(summary.unstaged, 1, "a.txt unstaged modification");
        assert_eq!(summary.untracked, 1, "c.txt untracked");
        assert_eq!(summary.conflicts, 0);
        assert_eq!(summary.entries.len(), 3);
        // Entries reuse the full StatusEntry shape (path-sorted).
        assert_eq!(summary.entries[0].path, "a.txt");
        assert_eq!(summary.entries[0].worktree, ChangeKind::Modified);
        assert_eq!(summary.entries[1].path, "c.txt");
        assert_eq!(summary.entries[1].worktree, ChangeKind::Untracked);
        assert_eq!(summary.entries[2].path, "d.txt");
        assert_eq!(summary.entries[2].index, ChangeKind::Added);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detached_head_reports_no_branch() {
        let dir = temp_dir("detached");
        let repo = git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        commit_all(&repo, "first");
        let head = repo.head().unwrap().target().unwrap();
        repo.set_head_detached(head).unwrap();

        let summary = summarize(&dir).expect("summary");
        assert!(summary.detached);
        assert_eq!(summary.branch, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unborn_repo_has_no_last_commit() {
        let dir = temp_dir("unborn");
        git2::Repository::init(&dir).unwrap();
        let summary = summarize(&dir).expect("summary");
        assert_eq!(summary.branch.as_deref(), Some("master"));
        assert_eq!(summary.last_commit, None);
        assert_eq!(summary.entries.len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_repo_path_is_rejected() {
        let dir = temp_dir("empty");
        let err = summarize(&dir).expect_err("non-repo must be rejected");
        assert!(!err.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bare_repo_is_rejected() {
        let dir = temp_dir("bare");
        git2::Repository::init_opts(&dir, git2::RepositoryInitOptions::new().bare(true)).unwrap();
        let err = summarize(&dir).expect_err("bare must be rejected");
        assert!(err.contains("bare"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_path_is_rejected() {
        let dir = temp_dir("gone");
        let missing = dir.join("deleted");
        let err = summarize(&missing).expect_err("missing path must be rejected");
        assert!(!err.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conflicts_count_from_a_conflicted_merge() {
        let dir = temp_dir("conflict");
        let repo = git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "base\n").unwrap();
        commit_all(&repo, "base");

        // branch "side" edits a.txt; master edits the same lines; the merge
        // then leaves a genuinely conflicted index (stages 1/2/3).
        repo.branch(
            "side",
            &repo
                .find_commit(repo.head().unwrap().target().unwrap())
                .unwrap(),
            false,
        )
        .unwrap();
        repo.set_head("refs/heads/side").unwrap();
        std::fs::write(dir.join("a.txt"), "side edit\n").unwrap();
        commit_all(&repo, "side change");
        repo.set_head("refs/heads/master").unwrap();
        std::fs::write(dir.join("a.txt"), "main edit\n").unwrap();
        commit_all(&repo, "main change");

        let side = repo
            .find_branch("side", git2::BranchType::Local)
            .unwrap()
            .get()
            .target()
            .unwrap();
        let annotated = repo.find_annotated_commit(side).expect("annotated commit");
        repo.merge(&[&annotated], None, None).expect("merge");

        let summary = summarize(&dir).expect("summary");
        assert_eq!(summary.conflicts, 1);
        assert_eq!(summary.entries.len(), 1);
        assert_eq!(summary.entries[0].index, ChangeKind::Conflicted);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
