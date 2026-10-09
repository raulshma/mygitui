//! Stash / worktree / reflog engine tests (lane D3).
//!
//! Everything here runs against throwaway repositories under
//! `std::env::temp_dir()` (no fixtures/) so parallel lanes' files stay
//! untouched. The methods under test are the inherent `*_impl` twins of the
//! `GitEngineM3` trait methods on `Libgit2Engine`.

use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Repository, RepositoryInitOptions};

use super::git_engine::EngineError;
use super::libgit2::Libgit2Engine;

const ENGINE: Libgit2Engine = Libgit2Engine;
const ZERO_SHA: &str = "0000000000000000000000000000000000000000";

// ---------------------------------------------------------------------------
// temp scaffolding (mirrors net_tests.rs)
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("mygitui-stash-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn sub(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best-effort delayed cleanup: on Windows, repository handles can
        // outlive the drop that spawned the remover by a few milliseconds.
        let dir = self.0.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}

fn init_repo(path: &Path) -> Repository {
    let mut opts = RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = Repository::init_opts(path, &opts).expect("init temp repo");
    repo.config()
        .and_then(|mut c| {
            c.set_str("user.name", "Stash Test")?;
            c.set_str("user.email", "stash@test.local")?;
            // Byte-exact file contents: neutralize a global core.autocrlf.
            c.set_bool("core.autocrlf", false)
        })
        .expect("configure identity");
    repo
}

fn write_file(repo: &Repository, path: &str, content: &str) {
    let file = repo.workdir().unwrap().join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(file, content).expect("write file");
}

fn read_file(repo: &Repository, path: &str) -> String {
    std::fs::read_to_string(repo.workdir().unwrap().join(path)).expect("read file")
}

/// Write + stage + commit on the current branch; returns the new sha.
fn commit_file(repo: &Repository, path: &str, content: &str, message: &str) -> String {
    write_file(repo, path, content);
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .expect("add all");
    index.write().expect("write index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("tree");
    let sig = repo.signature().expect("signature");
    let mut parents = Vec::new();
    if let Ok(head) = repo.head() {
        parents.push(head.peel_to_commit().expect("head commit"));
    }
    let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
        .expect("commit");
    oid.to_string()
}

/// Stage `path` at `content` without committing (index differs from HEAD).
fn stage_content(repo: &Repository, path: &str, content: &str) {
    write_file(repo, path, content);
    let mut index = repo.index().expect("index");
    index.add_path(Path::new(path)).expect("add path");
    index.write().expect("write index");
}

fn expect_invalid(err: EngineError) -> String {
    match err {
        EngineError::Invalid(msg) => msg,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// Message of any engine error (git errors surface verbatim where the
/// engine intentionally does not wrap them).
fn err_text(err: EngineError) -> String {
    match err {
        EngineError::Invalid(msg) => msg,
        EngineError::Git(e) => e.message().to_owned(),
        other => panic!("expected a git/invalid error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// stash
// ---------------------------------------------------------------------------

#[test]
fn stash_push_and_list_roundtrip() {
    let dir = TempDir::new("push-list");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    write_file(&repo, "a.txt", "two\n");
    ENGINE
        .stash_push_impl(&repo, None, false, false)
        .expect("stash push");

    // The worktree is reverted to HEAD after the push.
    assert_eq!(read_file(&repo, "a.txt"), "one\n");

    let stashes = ENGINE.stash_list_impl(&repo).expect("stash list");
    assert_eq!(stashes.len(), 1);
    let stash = &stashes[0];
    assert_eq!(stash.index, 0);
    assert_eq!(stash.sha.len(), 40);
    // libgit2's default message, kept verbatim.
    assert!(
        stash.message.starts_with("WIP on main:"),
        "unexpected message: {:?}",
        stash.message
    );
    assert_eq!(stash.author.name, "Stash Test");
    assert_eq!(stash.author.email, "stash@test.local");
}

#[test]
fn stash_push_keeps_custom_message_verbatim() {
    let dir = TempDir::new("push-message");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");
    write_file(&repo, "a.txt", "two\n");

    ENGINE
        .stash_push_impl(&repo, Some("wip: half done"), false, false)
        .expect("stash push");

    let stashes = ENGINE.stash_list_impl(&repo).expect("stash list");
    // libgit2 formats a caller message as "On <branch>: <msg>" in the
    // reflog; the list reports whatever is stored verbatim.
    assert_eq!(stashes[0].message, "On main: wip: half done");
}

#[test]
fn stash_push_without_changes_is_invalid() {
    let dir = TempDir::new("push-clean");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    let err = ENGINE
        .stash_push_impl(&repo, None, false, false)
        .expect_err("clean worktree must fail to stash");
    assert_eq!(expect_invalid(err), "nothing to stash");
}

#[test]
fn stash_apply_restores_and_keeps_entry() {
    let dir = TempDir::new("apply");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    write_file(&repo, "a.txt", "two\n");
    ENGINE
        .stash_push_impl(&repo, None, false, false)
        .expect("stash push");
    assert_eq!(read_file(&repo, "a.txt"), "one\n");

    ENGINE.stash_apply_impl(&repo, 0, false).expect("apply");
    assert_eq!(read_file(&repo, "a.txt"), "two\n");
    // Plain apply leaves the stash entry in place.
    assert_eq!(ENGINE.stash_list_impl(&repo).expect("list").len(), 1);
}

#[test]
fn stash_pop_restores_and_removes_entry() {
    let dir = TempDir::new("pop");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    write_file(&repo, "a.txt", "two\n");
    ENGINE
        .stash_push_impl(&repo, None, false, false)
        .expect("stash push");

    ENGINE.stash_apply_impl(&repo, 0, true).expect("pop");
    assert_eq!(read_file(&repo, "a.txt"), "two\n");
    assert!(ENGINE.stash_list_impl(&repo).expect("list").is_empty());
}

#[test]
fn stash_untracked_semantics() {
    let dir = TempDir::new("untracked");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    // Default: untracked files stay in the worktree and out of the stash
    // (a stash needs at least one tracked change, like `git stash`).
    write_file(&repo, "a.txt", "two\n");
    write_file(&repo, "u.txt", "untracked\n");
    ENGINE
        .stash_push_impl(&repo, None, false, false)
        .expect("stash push without untracked");
    assert_eq!(read_file(&repo, "u.txt"), "untracked\n");
    assert_eq!(read_file(&repo, "a.txt"), "one\n");
    ENGINE.stash_drop_impl(&repo, 0).expect("drop");

    // include_untracked: the file is swept into the stash and off the disk,
    // then restored by apply.
    ENGINE
        .stash_push_impl(&repo, None, false, true)
        .expect("stash push with untracked");
    assert!(
        !repo.workdir().unwrap().join("u.txt").exists(),
        "untracked file should be stashed away"
    );
    ENGINE.stash_apply_impl(&repo, 0, false).expect("apply");
    assert_eq!(read_file(&repo, "u.txt"), "untracked\n");
}

#[test]
fn stash_keep_index_leaves_staged_version_in_worktree() {
    let dir = TempDir::new("keep-index");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    // Index holds "two", the worktree holds "three".
    stage_content(&repo, "a.txt", "two\n");
    write_file(&repo, "a.txt", "three\n");

    ENGINE
        .stash_push_impl(&repo, None, true, false)
        .expect("stash push --keep-index");

    // The staged (index) version survives in the worktree.
    assert_eq!(read_file(&repo, "a.txt"), "two\n");
    assert_eq!(ENGINE.stash_list_impl(&repo).expect("list").len(), 1);

    // libgit2 refuses to apply over a dirty index ("uncommitted changes
    // exist in the index"), so the stash is released with a drop here.
    ENGINE.stash_drop_impl(&repo, 0).expect("drop");
    assert!(ENGINE.stash_list_impl(&repo).expect("list").is_empty());
}

#[test]
fn stash_drop_removes_newest_and_rejects_bad_index() {
    let dir = TempDir::new("drop");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    write_file(&repo, "a.txt", "v1\n");
    ENGINE
        .stash_push_impl(&repo, Some("first"), false, false)
        .expect("stash 1");
    write_file(&repo, "a.txt", "v2\n");
    ENGINE
        .stash_push_impl(&repo, Some("second"), false, false)
        .expect("stash 2");

    // Index 0 is the newest; libgit2 prefixes custom messages with
    // "On <branch>: " in the reflog.
    let stashes = ENGINE.stash_list_impl(&repo).expect("list");
    assert_eq!(stashes[0].message, "On main: second");
    assert_eq!(stashes[1].message, "On main: first");

    ENGINE.stash_drop_impl(&repo, 0).expect("drop newest");
    let stashes = ENGINE.stash_list_impl(&repo).expect("list");
    assert_eq!(stashes.len(), 1);
    assert_eq!(stashes[0].message, "On main: first");

    let err = ENGINE
        .stash_drop_impl(&repo, 5)
        .expect_err("out-of-range index");
    assert_eq!(expect_invalid(err), "no stash entry at index 5");

    ENGINE.stash_drop_impl(&repo, 0).expect("drop last");
    let err = ENGINE.stash_drop_impl(&repo, 0).expect_err("empty stack");
    assert_eq!(expect_invalid(err), "no stash entry at index 0");
}

#[test]
fn stash_branch_creates_branch_from_stash_base_and_applies() {
    let dir = TempDir::new("stash-branch");
    let repo = init_repo(dir.path());
    let base = commit_file(&repo, "a.txt", "one\n", "base");

    write_file(&repo, "a.txt", "two\n");
    ENGINE
        .stash_push_impl(&repo, Some("wip changes"), false, false)
        .expect("stash push");

    ENGINE
        .stash_branch_impl(&repo, "rescued", 0)
        .expect("stash branch");

    // Branch created at the commit the stash was based on...
    let rescued = repo
        .find_branch("rescued", git2::BranchType::Local)
        .expect("rescued branch");
    assert_eq!(rescued.get().target().expect("target").to_string(), base);
    // ...and checked out (read the symbolic HEAD itself; `repo.head()`
    // returns the resolved direct reference).
    assert_eq!(
        repo.find_reference("HEAD")
            .expect("HEAD")
            .symbolic_target()
            .expect("symbolic"),
        Some("refs/heads/rescued")
    );
    // The stashed changes were applied on the new branch...
    assert_eq!(read_file(&repo, "a.txt"), "two\n");
    // ...and the stash entry was dropped.
    assert!(ENGINE.stash_list_impl(&repo).expect("list").is_empty());

    let err = ENGINE
        .stash_branch_impl(&repo, "rescued", 0)
        .expect_err("stash stack is empty now");
    assert_eq!(expect_invalid(err), "no stash entry at index 0");
}

// ---------------------------------------------------------------------------
// worktrees
// ---------------------------------------------------------------------------

/// Repo with one committed base and a local `feature` branch (not checked out).
fn repo_with_feature_branch(name: &str) -> (TempDir, Repository) {
    let dir = TempDir::new(name);
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");
    {
        let head = repo.head().expect("head").peel_to_commit().expect("commit");
        repo.branch("feature", &head, false)
            .expect("feature branch");
    }
    (dir, repo)
}

#[test]
fn worktree_add_list_remove_roundtrip() {
    let (_dir, repo) = repo_with_feature_branch("wt-roundtrip");
    let wt_path = _dir.sub("wt-feature");
    let feature_sha = repo
        .find_branch("feature", git2::BranchType::Local)
        .expect("feature")
        .get()
        .target()
        .expect("target")
        .to_string();

    ENGINE
        .worktree_add_impl(&repo, wt_path.to_str().unwrap(), Some("feature"), None)
        .expect("worktree add");
    assert!(wt_path.join(".git").is_file(), "worktree checked out");

    let wts = ENGINE.worktrees_impl(&repo).expect("worktrees");
    // M12: the listing leads with the main worktree; linked entries follow.
    assert_eq!(wts.len(), 2, "main + wt-feature: {wts:?}");
    assert!(wts[0].is_main);
    let wt = wts
        .iter()
        .find(|w| w.name == "wt-feature")
        .expect("linked entry present");
    assert!(!wt.is_main);
    assert_eq!(wt.name, "wt-feature"); // name derived from path file_name
    assert_eq!(wt.branch.as_deref(), Some("feature"));
    assert_eq!(wt.head.as_deref(), Some(feature_sha.as_str()));
    assert!(!wt.detached);
    assert!(!wt.locked);
    assert_eq!(wt.prunable, None);

    ENGINE
        .worktree_remove_impl(&repo, "wt-feature", false)
        .expect("worktree remove");
    assert!(!wt_path.exists(), "worktree dir removed with admin area");
    // Only the main worktree remains after the linked one is removed.
    let remaining = ENGINE.worktrees_impl(&repo).expect("list");
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].is_main);

    let err = ENGINE
        .worktree_remove_impl(&repo, "wt-feature", false)
        .expect_err("second remove");
    assert_eq!(expect_invalid(err), "worktree `wt-feature` not found");
}

#[test]
fn worktree_add_new_branch_and_argument_guards() {
    let (_dir, repo) = repo_with_feature_branch("wt-new-branch");
    let wt_path = _dir.sub("wt-fresh");

    ENGINE
        .worktree_add_impl(&repo, wt_path.to_str().unwrap(), None, Some("fresh"))
        .expect("worktree add with new branch");
    assert!(repo.find_branch("fresh", git2::BranchType::Local).is_ok());
    let wts = ENGINE.worktrees_impl(&repo).expect("worktrees");
    assert_eq!(wts.len(), 2, "main + fresh: {wts:?}");
    let fresh = wts
        .iter()
        .find(|w| w.name == "wt-fresh")
        .expect("linked entry present");
    assert_eq!(fresh.branch.as_deref(), Some("fresh"));

    // Neither branch nor new_branch is out of M3 scope.
    let err = ENGINE
        .worktree_add_impl(&repo, _dir.sub("wt-none").to_str().unwrap(), None, None)
        .expect_err("detached add unsupported");
    assert!(expect_invalid(err).contains("requires"));

    // Unknown existing branch.
    let err = ENGINE
        .worktree_add_impl(
            &repo,
            _dir.sub("wt-nope").to_str().unwrap(),
            Some("nope"),
            None,
        )
        .expect_err("unknown branch");
    assert_eq!(expect_invalid(err), "branch `nope` not found");
}

#[test]
fn worktree_add_rejects_branch_checked_out_elsewhere() {
    let (_dir, repo) = repo_with_feature_branch("wt-checked-out");
    ENGINE
        .worktree_add_impl(
            &repo,
            _dir.sub("wt-a").to_str().unwrap(),
            Some("feature"),
            None,
        )
        .expect("first add ok");
    let err = ENGINE
        .worktree_add_impl(
            &repo,
            _dir.sub("wt-b").to_str().unwrap(),
            Some("feature"),
            None,
        )
        .expect_err("second checkout of same branch");
    assert!(
        err_text(err).contains("checked out"),
        "libgit2 already-checked-out error should surface"
    );
}

#[test]
fn worktree_locked_detection_and_remove_guards() {
    let (_dir, repo) = repo_with_feature_branch("wt-locked");
    ENGINE
        .worktree_add_impl(
            &repo,
            _dir.sub("wt-l").to_str().unwrap(),
            Some("feature"),
            None,
        )
        .expect("worktree add");

    // Lock it manually (what `git worktree lock` writes).
    let lock_file = repo.path().join("worktrees").join("wt-l").join("locked");
    std::fs::write(&lock_file, "held for the test").expect("write lock file");

    let wts = ENGINE.worktrees_impl(&repo).expect("worktrees");
    // M12: the main worktree leads the list; the linked entry follows.
    let wt_l = wts
        .iter()
        .find(|w| w.name == "wt-l")
        .expect("linked entry present");
    assert!(wt_l.locked, "lock file must be detected");
    assert!(!wts[0].is_main || !wts[0].locked, "main entry is unlocked");

    let err = ENGINE
        .worktree_remove_impl(&repo, "wt-l", false)
        .expect_err("locked remove without force");
    let msg = err_text(err);
    assert!(
        msg.contains("locked") || msg.contains("prun"),
        "unexpected error: {msg}"
    );

    ENGINE
        .worktree_remove_impl(&repo, "wt-l", true)
        .expect("force removes a locked worktree");
    let remaining = ENGINE.worktrees_impl(&repo).expect("list");
    assert_eq!(remaining.len(), 1, "only the main worktree remains");
    assert!(remaining[0].is_main);

    // Main-worktree guard: not in the linked list, never removable.
    let err = ENGINE
        .worktree_remove_impl(&repo, "main", false)
        .expect_err("main worktree");
    assert_eq!(expect_invalid(err), "worktree `main` not found");
}

// ---------------------------------------------------------------------------
// reflog
// ---------------------------------------------------------------------------

#[test]
fn reflog_head_chain_after_commits() {
    let dir = TempDir::new("reflog-head");
    let repo = init_repo(dir.path());
    let c1 = commit_file(&repo, "a.txt", "1\n", "c1");
    let c2 = commit_file(&repo, "a.txt", "2\n", "c2");
    let c3 = commit_file(&repo, "a.txt", "3\n", "c3");

    for name in [None, Some("HEAD"), Some("main"), Some("refs/heads/main")] {
        let entries = ENGINE.reflog_impl(&repo, name).expect("reflog");
        assert_eq!(entries.len(), 3, "name {name:?}");

        // Newest first, chained old -> new.
        assert_eq!(entries[0].new_sha, c3);
        assert_eq!(entries[1].new_sha, c2);
        assert_eq!(entries[2].new_sha, c1);
        assert_eq!(entries[2].old_sha, ZERO_SHA, "root commit starts at zero");
        assert_eq!(entries[0].old_sha, entries[1].new_sha);
        assert_eq!(entries[1].old_sha, entries[2].new_sha);

        assert!(
            entries[0].message.contains("c3"),
            "{:?}",
            entries[0].message
        );
        assert!(
            entries[0].message.starts_with("commit:"),
            "unexpected: {:?}",
            entries[0].message
        );
        assert_eq!(entries[2].message, "commit (initial): c1");

        // Committer signature from the configured identity.
        assert_eq!(entries[0].signature.name, "Stash Test");
    }

    let err = ENGINE
        .reflog_impl(&repo, Some("nosuchref"))
        .expect_err("unknown ref");
    assert!(expect_invalid(err).starts_with("no reflog for `nosuchref`"));
}
