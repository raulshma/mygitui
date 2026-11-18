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

static ENGINE: Libgit2Engine = Libgit2Engine::new();
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
        .stash_push_impl(&repo, None, false, false, None)
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
        .stash_push_impl(&repo, Some("wip: half done"), false, false, None)
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
        .stash_push_impl(&repo, None, false, false, None)
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
        .stash_push_impl(&repo, None, false, false, None)
        .expect("stash push");
    assert_eq!(read_file(&repo, "a.txt"), "one\n");

    ENGINE
        .stash_apply_impl(&repo, 0, false, false)
        .expect("apply");
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
        .stash_push_impl(&repo, None, false, false, None)
        .expect("stash push");

    ENGINE.stash_apply_impl(&repo, 0, true, false).expect("pop");
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
        .stash_push_impl(&repo, None, false, false, None)
        .expect("stash push without untracked");
    assert_eq!(read_file(&repo, "u.txt"), "untracked\n");
    assert_eq!(read_file(&repo, "a.txt"), "one\n");
    ENGINE.stash_drop_impl(&repo, 0).expect("drop");

    // include_untracked: the file is swept into the stash and off the disk,
    // then restored by apply.
    ENGINE
        .stash_push_impl(&repo, None, false, true, None)
        .expect("stash push with untracked");
    assert!(
        !repo.workdir().unwrap().join("u.txt").exists(),
        "untracked file should be stashed away"
    );
    ENGINE
        .stash_apply_impl(&repo, 0, false, false)
        .expect("apply");
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
        .stash_push_impl(&repo, None, true, false, None)
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
        .stash_push_impl(&repo, Some("first"), false, false, None)
        .expect("stash 1");
    write_file(&repo, "a.txt", "v2\n");
    ENGINE
        .stash_push_impl(&repo, Some("second"), false, false, None)
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
        .stash_push_impl(&repo, Some("wip changes"), false, false, None)
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
// stash paths / show / rename / apply --index (Phase 5 batch A)
// ---------------------------------------------------------------------------

#[test]
fn stash_push_pathspec_stashes_only_matching_paths() {
    let dir = TempDir::new("push-paths");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");
    commit_file(&repo, "b.txt", "one\n", "base");

    write_file(&repo, "a.txt", "two\n");
    write_file(&repo, "b.txt", "two\n");
    ENGINE
        .stash_push_impl(&repo, None, false, false, Some(&["a.txt".to_string()]))
        .expect("selective stash push");

    // Only the pathspec'd file was stashed away; the other keeps its edit.
    assert_eq!(read_file(&repo, "a.txt"), "one\n");
    assert_eq!(read_file(&repo, "b.txt"), "two\n");

    // The stash itself carries exactly the matching file.
    let shown = ENGINE.stash_show_impl(&repo, 0).expect("stash show");
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].path, "a.txt");

    // Popping restores only that file.
    ENGINE.stash_apply_impl(&repo, 0, true, false).expect("pop");
    assert_eq!(read_file(&repo, "a.txt"), "two\n");
    assert!(ENGINE.stash_list_impl(&repo).expect("list").is_empty());
}

#[test]
fn stash_push_pathspec_includes_matching_untracked() {
    // --include-untracked + pathspec: the matching untracked file is swept
    // into the stash, non-matching ones stay on disk.
    let dir = TempDir::new("push-paths-untracked");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");
    write_file(&repo, "new.txt", "untracked\n");
    write_file(&repo, "other.txt", "other\n");

    ENGINE
        .stash_push_impl(&repo, None, false, true, Some(&["new.txt".to_string()]))
        .expect("selective stash with untracked");
    assert!(!repo.workdir().unwrap().join("new.txt").exists());
    assert!(repo.workdir().unwrap().join("other.txt").exists());

    ENGINE
        .stash_apply_impl(&repo, 0, false, false)
        .expect("apply");
    assert_eq!(read_file(&repo, "new.txt"), "untracked\n");
    assert!(repo.workdir().unwrap().join("other.txt").exists());
}

#[test]
fn stash_show_returns_stash_file_diffs() {
    let dir = TempDir::new("show");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");
    commit_file(&repo, "b.txt", "keep\n", "base");

    write_file(&repo, "a.txt", "two\n");
    write_file(&repo, "b.txt", "changed\n");
    ENGINE
        .stash_push_impl(&repo, None, false, false, None)
        .expect("stash push");

    let files = ENGINE.stash_show_impl(&repo, 0).expect("stash show");
    assert_eq!(files.len(), 2, "{files:?}");
    let find = |p: &str| {
        files
            .iter()
            .find(|f| f.path == p)
            .unwrap_or_else(|| panic!("{p} missing from {files:?}"))
    };
    let a = find("a.txt");
    assert_eq!(a.additions, 1);
    assert_eq!(a.deletions, 1);
    assert!(
        a.hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| l.text == "two")),
        "{a:?}"
    );
    assert_eq!(find("b.txt").deletions, 1);

    // Out-of-range index errors before any diff is built.
    let err = ENGINE.stash_show_impl(&repo, 3).expect_err("bad index");
    assert_eq!(expect_invalid(err), "no stash entry at index 3");
}

#[test]
fn stash_apply_index_restores_staged_state() {
    use super::git_engine::GitEngine;
    use super::types::DiffSide;

    let dir = TempDir::new("apply-index");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    // Stage "two"; the worktree holds the same content (nothing unstaged).
    stage_content(&repo, "a.txt", "two\n");
    write_file(&repo, "a.txt", "two\n");
    ENGINE
        .stash_push_impl(&repo, None, false, false, None)
        .expect("stash push");
    assert!(ENGINE
        .diff(&repo, &DiffSide::Head, &DiffSide::Index, None, false)
        .expect("clean after push")
        .is_empty());

    // Plain apply: the change comes back UNSTAGED (index stays at HEAD).
    ENGINE
        .stash_apply_impl(&repo, 0, false, false)
        .expect("plain apply");
    assert!(
        ENGINE
            .diff(&repo, &DiffSide::Head, &DiffSide::Index, None, false)
            .expect("index vs head")
            .is_empty(),
        "plain apply must not stage anything"
    );
    assert_eq!(
        ENGINE
            .diff(&repo, &DiffSide::Index, &DiffSide::Worktree, None, false)
            .expect("index vs worktree")
            .len(),
        1,
        "the applied change sits in the worktree"
    );

    // Back to a clean tree (index == HEAD == workdir) for the --index leg.
    write_file(&repo, "a.txt", "one\n");
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("a.txt")).unwrap();
    index.write().unwrap();

    // --index apply: the stashed staged state comes back staged, and the
    // entry is dropped (pop).
    ENGINE
        .stash_apply_impl(&repo, 0, true, true)
        .expect("apply --index");
    let staged = ENGINE
        .diff(&repo, &DiffSide::Head, &DiffSide::Index, None, false)
        .expect("index vs head");
    assert_eq!(staged.len(), 1, "staged change restored as staged");
    assert_eq!(staged[0].path, "a.txt");
    assert!(
        ENGINE.stash_list_impl(&repo).expect("list").is_empty(),
        "pop removed the entry"
    );
}

#[test]
fn stash_rename_rewrites_top_message_and_keeps_stack() {
    let dir = TempDir::new("rename-top");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");
    write_file(&repo, "a.txt", "two\n");
    ENGINE
        .stash_push_impl(&repo, Some("first"), false, false, None)
        .expect("stash 1");
    write_file(&repo, "a.txt", "three\n");
    ENGINE
        .stash_push_impl(&repo, Some("second"), false, false, None)
        .expect("stash 2");

    ENGINE
        .stash_rename_impl(&repo, 0, "renamed: wip number two")
        .expect("rename top");

    let after = ENGINE.stash_list_impl(&repo).expect("list after rename");
    assert_eq!(after.len(), 2, "rename must not grow or shrink the stack");
    assert_eq!(after[0].message, "renamed: wip number two");
    assert_eq!(after[1].message, "On main: first", "older entry untouched");
    assert_eq!(after[0].index, 0);
    // The author comes from the (preserved) stash commit.
    assert_eq!(after[0].author.name, "Stash Test");

    // The ref and the newest reflog entry agree on the renamed commit.
    let stash_ref = repo.find_reference("refs/stash").expect("stash ref");
    assert_eq!(
        stash_ref.target().expect("target").to_string(),
        after[0].sha
    );
    // The renamed entry still shows its diff, the older one still applies.
    let shown = ENGINE
        .stash_show_impl(&repo, 0)
        .expect("show renamed stash");
    assert_eq!(shown.len(), 1);
    ENGINE
        .stash_apply_impl(&repo, 1, false, false)
        .expect("apply older entry");
    assert_eq!(read_file(&repo, "a.txt"), "two\n");
}

#[test]
fn stash_rename_rejects_non_top_and_empty_message() {
    let dir = TempDir::new("rename-guards");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");
    write_file(&repo, "a.txt", "two\n");
    ENGINE
        .stash_push_impl(&repo, Some("first"), false, false, None)
        .expect("stash 1");
    write_file(&repo, "a.txt", "three\n");
    ENGINE
        .stash_push_impl(&repo, Some("second"), false, false, None)
        .expect("stash 2");

    let err = ENGINE
        .stash_rename_impl(&repo, 1, "nope")
        .expect_err("non-top rename");
    assert_eq!(
        expect_invalid(err),
        "only the newest stash can be renamed (stash@{0})"
    );
    let err = ENGINE
        .stash_rename_impl(&repo, 0, "   ")
        .expect_err("empty message");
    assert_eq!(expect_invalid(err), "stash message cannot be empty");

    // Guards fired before any mutation: the stack is unchanged.
    let stashes = ENGINE.stash_list_impl(&repo).expect("list");
    assert_eq!(stashes.len(), 2);
    assert_eq!(stashes[0].message, "On main: second");
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
        .worktree_add_impl(
            &repo,
            wt_path.to_str().unwrap(),
            Some("feature"),
            None,
            None,
        )
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
fn worktree_remove_rejects_dirty_linked_worktrees_without_losing_data() {
    for dirty in ["staged", "unstaged", "untracked", "mixed"] {
        let (dir, repo) = repo_with_feature_branch(&format!("wt-dirty-{dirty}"));
        let wt_path = dir.sub("wt-dirty");
        ENGINE
            .worktree_add_impl(
                &repo,
                wt_path.to_str().unwrap(),
                Some("feature"),
                None,
                None,
            )
            .expect("worktree add");
        let linked = Repository::open(&wt_path).expect("open linked worktree");
        if matches!(dirty, "staged" | "mixed") {
            stage_content(&linked, "a.txt", "staged bytes\n");
        }
        if matches!(dirty, "unstaged" | "mixed") {
            write_file(&linked, "a.txt", "unstaged bytes\n");
        }
        if matches!(dirty, "untracked" | "mixed") {
            write_file(&linked, "scratch/data.txt", "untracked bytes\n");
        }
        let index_path = linked.path().join("index");
        let index_before = std::fs::read(&index_path).unwrap();
        let head_before = std::fs::read(linked.path().join("HEAD")).unwrap();
        let tracked_before = std::fs::read(wt_path.join("a.txt")).unwrap();
        let gitfile_before = std::fs::read(wt_path.join(".git")).unwrap();
        // Release the linked handle: an OS file lock must not be what saves it.
        drop(linked);

        ENGINE
            .worktree_remove_impl(&repo, "wt-dirty", false)
            .expect_err("non-force removal must reject local changes");
        assert_eq!(std::fs::read(&index_path).unwrap(), index_before, "{dirty}");
        assert_eq!(
            std::fs::read(index_path.with_file_name("HEAD")).unwrap(),
            head_before
        );
        assert_eq!(
            std::fs::read(wt_path.join("a.txt")).unwrap(),
            tracked_before
        );
        assert_eq!(std::fs::read(wt_path.join(".git")).unwrap(), gitfile_before);
        if matches!(dirty, "untracked" | "mixed") {
            assert_eq!(
                std::fs::read(wt_path.join("scratch/data.txt")).unwrap(),
                b"untracked bytes\n"
            );
        }
        assert!(repo.find_worktree("wt-dirty").is_ok());

        ENGINE
            .worktree_remove_impl(&repo, "wt-dirty", true)
            .expect("explicit force may discard local changes");
        assert!(!wt_path.exists());
        assert!(repo.find_worktree("wt-dirty").is_err());
    }
}

#[test]
fn worktree_remove_matches_git_for_ignored_only_worktrees() {
    let (dir, repo) = repo_with_feature_branch("wt-ignored-only");
    let wt_path = dir.sub("wt-ignored");
    ENGINE
        .worktree_add_impl(
            &repo,
            wt_path.to_str().unwrap(),
            Some("feature"),
            None,
            None,
        )
        .unwrap();
    std::fs::write(repo.path().join("info/exclude"), "*.ignored\n").unwrap();
    std::fs::write(wt_path.join("cache.ignored"), "ignored cache\n").unwrap();

    // Git worktree remove treats ignored-only files as clean (unlike untracked).
    ENGINE
        .worktree_remove_impl(&repo, "wt-ignored", false)
        .unwrap();
    assert!(!wt_path.exists());
}

#[test]
fn worktree_add_new_branch_and_argument_guards() {
    let (_dir, repo) = repo_with_feature_branch("wt-new-branch");
    let wt_path = _dir.sub("wt-fresh");

    ENGINE
        .worktree_add_impl(&repo, wt_path.to_str().unwrap(), None, Some("fresh"), None)
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
        .worktree_add_impl(
            &repo,
            _dir.sub("wt-none").to_str().unwrap(),
            None,
            None,
            None,
        )
        .expect_err("detached add unsupported");
    assert!(expect_invalid(err).contains("requires"));

    // Unknown existing branch.
    let err = ENGINE
        .worktree_add_impl(
            &repo,
            _dir.sub("wt-nope").to_str().unwrap(),
            Some("nope"),
            None,
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
            None,
        )
        .expect("first add ok");
    let err = ENGINE
        .worktree_add_impl(
            &repo,
            _dir.sub("wt-b").to_str().unwrap(),
            Some("feature"),
            None,
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

#[test]
fn log_walk_excludes_stash_commits_and_starts_at_head() {
    use super::git_engine::GitEngine;
    use super::types::LogFilter;

    let dir = TempDir::new("log-excludes-stash");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    write_file(&repo, "a.txt", "two\n");
    ENGINE
        .stash_push_impl(&repo, None, false, false, None)
        .expect("stash push");
    let stashes = ENGINE.stash_list_impl(&repo).expect("stash list");
    assert_eq!(stashes.len(), 1);

    let (page, next) = ENGINE
        .log(&repo, &LogFilter::default(), 10, None)
        .expect("log");
    assert!(next.is_none(), "one real commit must end the walk");
    // The merge-shaped stash commits never enter the commit-graph walk; the
    // list is real history only, newest first.
    for commit in &page {
        assert_ne!(
            commit.sha, stashes[0].sha,
            "stash commit leaked into the log walk"
        );
    }
    let head = repo.head().expect("head").target().expect("target");
    assert_eq!(page[0].sha, head.to_string(), "HEAD pops first");
    assert_eq!(page[0].summary, "base");
}

// ---------------------------------------------------------------------------
// worktree lock/unlock + from_remote (Phase 5 item 48c)
// ---------------------------------------------------------------------------

/// Lock/unlock round-trip by NAME and by PATH, with the main-worktree
/// refusal git enforces ("The main working tree cannot be locked or
/// unlocked") mirrored up front.
#[test]
fn worktree_lock_unlock_roundtrip() {
    let (dir, repo) = repo_with_feature_branch("wt-lock-unlock");
    let wt_path = dir.sub("wt-lock");
    ENGINE
        .worktree_add_impl(
            &repo,
            wt_path.to_str().unwrap(),
            Some("feature"),
            None,
            None,
        )
        .expect("worktree add");

    // Lock by name with a reason: the reason is readable through libgit2
    // (what the FE tooltip shows) and the listing flips `locked`.
    ENGINE
        .worktree_lock_impl(&repo, "wt-lock", Some("release freeze"))
        .expect("lock by name");
    let wt = repo.find_worktree("wt-lock").expect("worktree");
    match wt.is_locked().expect("lock status") {
        git2::WorktreeLockStatus::Locked(reason) => {
            assert_eq!(reason.as_deref(), Some("release freeze"))
        }
        other => panic!("expected Locked, got {other:?}"),
    }
    let wts = ENGINE.worktrees_impl(&repo).expect("worktrees");
    assert!(wts.iter().find(|w| w.name == "wt-lock").unwrap().locked);

    // Unlock by name, re-lock by PATH without a reason (empty -> plain lock).
    ENGINE
        .worktree_unlock_impl(&repo, "wt-lock")
        .expect("unlock by name");
    assert!(!repo
        .find_worktree("wt-lock")
        .expect("worktree")
        .is_locked()
        .map(|s| matches!(s, git2::WorktreeLockStatus::Locked(_)))
        .unwrap_or(false));
    ENGINE
        .worktree_lock_impl(&repo, wt_path.to_str().unwrap(), None)
        .expect("lock by path");
    assert!(matches!(
        repo.find_worktree("wt-lock")
            .expect("worktree")
            .is_locked()
            .expect("lock status"),
        git2::WorktreeLockStatus::Locked(_)
    ));
    ENGINE
        .worktree_unlock_impl(&repo, wt_path.to_str().unwrap())
        .expect("unlock by path");

    // Main worktree refusal (git refuses too; we reject before libgit2).
    let main_path = repo.workdir().unwrap().to_str().unwrap().to_owned();
    let err = ENGINE
        .worktree_lock_impl(&repo, &main_path, None)
        .expect_err("main worktree lock");
    assert_eq!(expect_invalid(err), "the main worktree cannot be locked");
    let err = ENGINE
        .worktree_unlock_impl(&repo, &main_path)
        .expect_err("main worktree unlock");
    assert_eq!(expect_invalid(err), "the main worktree cannot be unlocked");

    // Unknown spec (neither a worktree name nor a path).
    let err = ENGINE
        .worktree_lock_impl(&repo, "nope", None)
        .expect_err("unknown worktree");
    assert_eq!(expect_invalid(err), "worktree `nope` not found");
}

/// `worktree_add(from_remote = "origin/feature")`: creates the local branch
/// at the tracking tip with upstream configured (the
/// `branch_checkout_remote` logic) and adds the worktree on it — without
/// touching the main worktree's checkout.
#[test]
fn worktree_add_from_remote_creates_branch_and_worktree() {
    // Origin repo holding the remote `feature` branch.
    let (origin_dir, origin_repo) = repo_with_feature_branch("wt-fromremote-origin");
    let (dir, repo) = repo_with_feature_branch("wt-fromremote");

    // Register the local origin and fetch its tracking ref (plain path
    // transport, no credentials needed).
    repo.remote("origin", origin_dir.path().to_str().unwrap())
        .expect("add remote")
        .fetch(
            &["refs/heads/feature:refs/remotes/origin/feature"],
            None,
            None,
        )
        .expect("fetch tracking ref");
    let origin_sha = origin_repo
        .find_branch("feature", git2::BranchType::Local)
        .expect("origin feature")
        .get()
        .target()
        .expect("target")
        .to_string();
    // The main repo has no local `feature` yet (repo_with_feature_branch
    // creates `feature` on the origin side too — drop it here so the
    // from_remote lane has room to create it).
    repo.find_branch("feature", git2::BranchType::Local)
        .expect("main repo feature")
        .delete()
        .expect("delete local feature");

    let wt_path = dir.sub("wt-fromremote");
    ENGINE
        .worktree_add_impl(
            &repo,
            wt_path.to_str().unwrap(),
            None,
            None,
            Some("origin/feature"),
        )
        .expect("worktree add from remote");

    // The local branch was created at the tracking tip, upstream configured.
    let branch = repo
        .find_branch("feature", git2::BranchType::Local)
        .expect("local branch created");
    assert_eq!(
        branch.get().target().expect("target").to_string(),
        origin_sha,
        "branch points at the tracking tip"
    );
    assert_eq!(
        branch.upstream().expect("upstream").name(),
        Ok(Some("origin/feature"))
    );

    // The worktree sits on it; the main worktree stayed on `main`.
    assert!(wt_path.join(".git").is_file(), "worktree checked out");
    let wts = ENGINE.worktrees_impl(&repo).expect("worktrees");
    let wt = wts
        .iter()
        .find(|w| w.name == "wt-fromremote")
        .expect("worktree entry");
    assert_eq!(wt.branch.as_deref(), Some("feature"));

    // Argument guards: shorthand must be `<remote>/<branch>`, and
    // `from_remote` cannot combine with the other two selectors.
    let err = ENGINE
        .worktree_add_impl(
            &repo,
            dir.sub("wt-bad-spec").to_str().unwrap(),
            None,
            None,
            Some("feature"),
        )
        .expect_err("no slash in spec");
    assert!(
        expect_invalid(err).contains("<remote>/<branch>"),
        "shorthand guard"
    );
    let err = ENGINE
        .worktree_add_impl(
            &repo,
            dir.sub("wt-bad-combo").to_str().unwrap(),
            Some("main"),
            None,
            Some("origin/feature"),
        )
        .expect_err("combined selectors");
    assert!(expect_invalid(err).contains("`from_remote` cannot be combined"));
}
