//! Lane D1 tests: merge, conflicts, cherry-pick, revert, reset.
//!
//! Everything runs against throwaway repositories under
//! `std::env::temp_dir()` (never fixtures/) — same pattern as net_tests.rs.

use std::path::{Path, PathBuf};

use git2::{IndexAddOption, Repository, RepositoryInitOptions, RepositoryState};

use super::git_engine::{EngineError, GitEngine, GitEngineM3};
use super::libgit2::Libgit2Engine;
use super::types::{CommitOptions, ConflictResolution, MergeOutcome, ResetKind};

const ENGINE: Libgit2Engine = Libgit2Engine;

// ---------------------------------------------------------------------------
// temp scaffolding
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("mygitui-merge-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
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
    opts.bare(false).initial_head("main");
    let repo = Repository::init_opts(path, &opts).expect("init temp repo");
    repo.config()
        .and_then(|mut c| {
            c.set_str("user.name", "Merge Test")?;
            c.set_str("user.email", "merge@test.local")?;
            // Deterministic workdir bytes: the machine-wide core.autocrlf
            // would otherwise rewrite checked-out files to CRLF.
            c.set_bool("core.autocrlf", false)
        })
        .expect("configure identity");
    repo
}

/// Repo + its temp dir (the dir is dropped last so the repo closes before
/// the directory goes away on Windows).
struct Fixture {
    repo: Repository,
    _dir: Option<TempDir>,
}

fn init_fixture(name: &str) -> Fixture {
    let dir = TempDir::new(name);
    let repo = init_repo(dir.path());
    Fixture {
        repo,
        _dir: Some(dir),
    }
}

fn write_file(repo: &Repository, path: &str, content: &str) {
    let file = repo.workdir().unwrap().join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(file, content).expect("write file");
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

/// `commit_file` with an explicit author (the committer stays configured).
fn commit_file_as(
    repo: &Repository,
    path: &str,
    content: &str,
    message: &str,
    name: &str,
    email: &str,
) -> String {
    write_file(repo, path, content);
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .expect("add all");
    index.write().expect("write index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("tree");
    let author = git2::Signature::now(name, email).expect("author signature");
    let committer = repo.signature().expect("committer signature");
    let parents: Vec<git2::Commit<'_>> = match repo.head() {
        Ok(head) => vec![head.peel_to_commit().expect("head commit")],
        Err(_) => Vec::new(),
    };
    let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
    let oid = repo
        .commit(
            Some("HEAD"),
            &author,
            &committer,
            message,
            &tree,
            &parent_refs,
        )
        .expect("commit");
    oid.to_string()
}

fn head_sha(repo: &Repository) -> String {
    repo.head()
        .expect("head")
        .peel_to_commit()
        .expect("head commit")
        .id()
        .to_string()
}

fn branch_sha(repo: &Repository, name: &str) -> String {
    repo.find_reference(&format!("refs/heads/{name}"))
        .expect("branch ref")
        .target()
        .expect("direct branch ref")
        .to_string()
}

fn workdir_bytes(repo: &Repository, path: &str) -> Vec<u8> {
    std::fs::read(repo.workdir().unwrap().join(path)).expect("read workdir file")
}

fn workdir_text(repo: &Repository, path: &str) -> String {
    String::from_utf8(workdir_bytes(repo, path)).expect("utf-8 workdir file")
}

fn staged_content(repo: &Repository, path: &str, stage: i32) -> Option<Vec<u8>> {
    let index = repo.index().expect("index");
    let entry = index.get_path(Path::new(path), stage)?;
    let blob = repo.find_blob(entry.id).expect("blob");
    Some(blob.content().to_vec())
}

fn assert_invalid(result: EngineError) {
    assert!(
        matches!(result, EngineError::Invalid(_)),
        "want Invalid, got {result:?}"
    );
}

/// Base: three files both sides will edit + one untouched file. `feature`
/// diverges (3 conflicting edits + one clean new file); HEAD is back on
/// `main` with its own edits to the same 3 files.
fn conflicted_fixture(name: &str) -> Fixture {
    let fx = init_fixture(name);
    commit_file(&fx.repo, "conflict1.txt", "base one\n", "base: conflict1");
    commit_file(&fx.repo, "conflict2.txt", "base two\n", "base: conflict2");
    commit_file(&fx.repo, "conflict3.txt", "base three\n", "base: conflict3");
    commit_file(&fx.repo, "keep.txt", "same\n", "base: keep");
    ENGINE
        .branch_create(&fx.repo, "feature", None, true)
        .expect("branch feature");
    commit_file(
        &fx.repo,
        "conflict1.txt",
        "feature one\n",
        "feature: conflict1",
    );
    commit_file(
        &fx.repo,
        "conflict2.txt",
        "feature two\n",
        "feature: conflict2",
    );
    commit_file(
        &fx.repo,
        "conflict3.txt",
        "feature three\n",
        "feature: conflict3",
    );
    commit_file(
        &fx.repo,
        "feature_new.txt",
        "brand new\n",
        "feature: new file",
    );
    ENGINE
        .branch_switch(&fx.repo, "main", false)
        .expect("switch back to main");
    commit_file(&fx.repo, "conflict1.txt", "main one\n", "main: conflict1");
    commit_file(&fx.repo, "conflict2.txt", "main two\n", "main: conflict2");
    commit_file(&fx.repo, "conflict3.txt", "main three\n", "main: conflict3");
    fx
}

/// Merge `feature` into `main` in `conflicted_fixture`, expecting a conflict.
fn merge_feature_conflicted(repo: &Repository) -> super::types::MergeResult {
    let result = ENGINE
        .merge_branch(repo, "feature", false)
        .expect("merge branch");
    assert!(
        matches!(result.outcome, MergeOutcome::Conflicted),
        "want Conflicted, got {:?}",
        result.outcome
    );
    result
}

// ---------------------------------------------------------------------------
// merge_branch
// ---------------------------------------------------------------------------

#[test]
fn merge_fast_forward_moves_branch_and_workdir() {
    let fx = init_fixture("merge-ff");
    let base = commit_file(&fx.repo, "file.txt", "base\n", "base");
    ENGINE
        .branch_create(&fx.repo, "feature", None, true)
        .expect("branch feature");
    let feature_tip = commit_file(&fx.repo, "file.txt", "feature\n", "feature work");
    ENGINE
        .branch_switch(&fx.repo, "main", false)
        .expect("switch back");

    let result = ENGINE
        .merge_branch(&fx.repo, "feature", false)
        .expect("merge");

    assert!(matches!(result.outcome, MergeOutcome::FastForward));
    assert_eq!(result.new_head.as_deref(), Some(feature_tip.as_str()));
    assert_eq!(branch_sha(&fx.repo, "main"), feature_tip);
    assert_ne!(branch_sha(&fx.repo, "main"), base);
    assert_eq!(head_sha(&fx.repo), feature_tip);
    assert_eq!(workdir_text(&fx.repo, "file.txt"), "feature\n");
}

#[test]
fn merge_diverged_creates_two_parent_commit() {
    let fx = init_fixture("merge-diverged");
    let base = commit_file(&fx.repo, "base.txt", "base\n", "base");
    ENGINE
        .branch_create(&fx.repo, "feature", None, true)
        .expect("branch feature");
    let feature_tip = commit_file(&fx.repo, "feature.txt", "from feature\n", "feature adds");
    ENGINE
        .branch_switch(&fx.repo, "main", false)
        .expect("switch back");
    let main_tip = commit_file(&fx.repo, "main.txt", "from main\n", "main adds");

    let result = ENGINE
        .merge_branch(&fx.repo, "feature", false)
        .expect("merge");

    assert!(matches!(result.outcome, MergeOutcome::Merged));
    let new_head = result.new_head.expect("merge commit sha");
    let head = fx
        .repo
        .head()
        .expect("head")
        .peel_to_commit()
        .expect("commit");
    assert_eq!(head.id().to_string(), new_head);
    assert_eq!(head.parent_count(), 2, "merge commit has two parents");
    assert_eq!(head.parent(0).expect("p0").id().to_string(), main_tip);
    assert_eq!(head.parent(1).expect("p1").id().to_string(), feature_tip);
    assert_eq!(head.parent(0).expect("p0").parent_count(), 1);
    assert_eq!(
        head.parent(0)
            .expect("p0")
            .parent(0)
            .expect("base")
            .id()
            .to_string(),
        base
    );
    assert_eq!(head.message().ok(), Some("Merge branch 'feature'"));
    // Both sides landed in the workdir, MERGE_HEAD consumed.
    assert_eq!(workdir_text(&fx.repo, "feature.txt"), "from feature\n");
    assert_eq!(workdir_text(&fx.repo, "main.txt"), "from main\n");
    assert!(!fx.repo.path().join("MERGE_HEAD").exists());
    assert_eq!(fx.repo.state(), RepositoryState::Clean);
}

#[test]
fn merge_no_ff_forces_merge_commit() {
    let fx = init_fixture("merge-no-ff");
    let main_before = commit_file(&fx.repo, "file.txt", "base\n", "base");
    ENGINE
        .branch_create(&fx.repo, "feature", None, true)
        .expect("branch feature");
    let feature_tip = commit_file(&fx.repo, "file.txt", "feature\n", "feature work");
    ENGINE
        .branch_switch(&fx.repo, "main", false)
        .expect("switch back");

    let result = ENGINE
        .merge_branch(&fx.repo, "feature", true)
        .expect("merge --no-ff");

    assert!(matches!(result.outcome, MergeOutcome::Merged));
    let head = fx
        .repo
        .head()
        .expect("head")
        .peel_to_commit()
        .expect("commit");
    assert_eq!(head.parent_count(), 2, "no_ff forces a real merge commit");
    assert_eq!(head.parent(0).expect("p0").id().to_string(), main_before);
    assert_eq!(head.parent(1).expect("p1").id().to_string(), feature_tip);
    assert_eq!(workdir_text(&fx.repo, "file.txt"), "feature\n");
}

#[test]
fn merge_up_to_date_is_noop() {
    let fx = init_fixture("merge-up-to-date");
    let base = commit_file(&fx.repo, "file.txt", "base\n", "base");
    ENGINE
        .branch_create(&fx.repo, "feature", None, false)
        .expect("branch feature at base");

    let result = ENGINE
        .merge_branch(&fx.repo, "feature", false)
        .expect("merge");

    assert!(matches!(result.outcome, MergeOutcome::UpToDate));
    assert_eq!(result.new_head.as_deref(), Some(base.as_str()));
    assert_eq!(head_sha(&fx.repo), base, "HEAD did not move");
}

#[test]
fn merge_conflict_reports_files_and_keeps_clean_paths() {
    let fx = conflicted_fixture("merge-conflict");
    let main_tip = head_sha(&fx.repo);

    let result = merge_feature_conflicted(&fx.repo);

    // Conflicted entries: one per both-sides-edited file, all 3 stages there.
    let mut paths: Vec<&str> = result.conflicts.iter().map(|c| c.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["conflict1.txt", "conflict2.txt", "conflict3.txt"]);
    for file in &result.conflicts {
        assert_eq!(file.source, "merge");
        assert!(
            file.has_base && file.has_ours && file.has_theirs,
            "{file:?}"
        );
    }
    assert_eq!(result.new_head, None);

    // Merge state is live: MERGE_HEAD, libgit2 state, status flag, markers.
    assert!(fx.repo.path().join("MERGE_HEAD").exists());
    assert_eq!(fx.repo.state(), RepositoryState::Merge);
    let status = ENGINE.status(&fx.repo).expect("status");
    assert!(status.merging);
    let marked = workdir_text(&fx.repo, "conflict1.txt");
    assert!(
        marked.contains("<<<<<<<"),
        "workdir has markers: {marked:?}"
    );

    // The clean side of the merge is untouched (workdir + HEAD content).
    assert_eq!(workdir_text(&fx.repo, "feature_new.txt"), "brand new\n");
    assert_eq!(workdir_text(&fx.repo, "keep.txt"), "same\n");
    assert_eq!(
        staged_content(&fx.repo, "keep.txt", 0).as_deref(),
        Some(b"same\n".as_slice())
    );
    assert_eq!(head_sha(&fx.repo), main_tip, "no commit while conflicted");
}

#[test]
fn merge_abort_restores_head_state() {
    let fx = conflicted_fixture("merge-abort");
    let main_tip = head_sha(&fx.repo);
    merge_feature_conflicted(&fx.repo);

    ENGINE.merge_abort(&fx.repo).expect("merge abort");

    assert!(!fx.repo.path().join("MERGE_HEAD").exists());
    assert_eq!(fx.repo.state(), RepositoryState::Clean);
    assert_eq!(head_sha(&fx.repo), main_tip);
    // Conflicted file restored to HEAD content, merge-added file removed.
    assert_eq!(workdir_text(&fx.repo, "conflict1.txt"), "main one\n");
    assert!(!fx.repo.workdir().unwrap().join("feature_new.txt").exists());
    let status = ENGINE.status(&fx.repo).expect("status");
    assert!(status.entries.is_empty(), "workdir clean after abort");
    assert!(!status.merging);
}

#[test]
fn merge_abort_without_merge_errors() {
    let fx = init_fixture("merge-abort-noop");
    commit_file(&fx.repo, "file.txt", "base\n", "base");

    let err = ENGINE.merge_abort(&fx.repo).expect_err("no merge running");
    assert_invalid(err);
}

// ---------------------------------------------------------------------------
// conflict_resolve
// ---------------------------------------------------------------------------

#[test]
fn conflict_resolve_ours_theirs_and_custom() {
    let fx = conflicted_fixture("resolve-three");
    merge_feature_conflicted(&fx.repo);

    ENGINE
        .conflict_resolve(&fx.repo, "conflict1.txt", ConflictResolution::Ours, None)
        .expect("resolve ours");
    ENGINE
        .conflict_resolve(&fx.repo, "conflict2.txt", ConflictResolution::Theirs, None)
        .expect("resolve theirs");
    // Custom content (editor bytes) resolves + stages in one step.
    ENGINE
        .conflict_resolve(
            &fx.repo,
            "conflict3.txt",
            ConflictResolution::Both,
            Some(b"merged by hand\n".as_slice()),
        )
        .expect("resolve with custom content");

    // Workdir files carry the chosen bytes.
    assert_eq!(workdir_text(&fx.repo, "conflict1.txt"), "main one\n");
    assert_eq!(workdir_text(&fx.repo, "conflict2.txt"), "feature two\n");
    assert_eq!(workdir_text(&fx.repo, "conflict3.txt"), "merged by hand\n");
    // Resolutions are staged (index no longer conflicted, entry at stage 0).
    assert_eq!(
        staged_content(&fx.repo, "conflict1.txt", 0).as_deref(),
        Some(b"main one\n".as_slice())
    );
    assert_eq!(
        staged_content(&fx.repo, "conflict2.txt", 0).as_deref(),
        Some(b"feature two\n".as_slice())
    );
    assert_eq!(
        staged_content(&fx.repo, "conflict3.txt", 0).as_deref(),
        Some(b"merged by hand\n".as_slice())
    );
    assert!(
        ENGINE.conflicts(&fx.repo).expect("conflicts").is_empty(),
        "all conflicts resolved"
    );
    // Resolving does not finish the merge: still awaiting the merge commit.
    assert_eq!(fx.repo.state(), RepositoryState::Merge);
}

#[test]
fn conflict_resolve_both_keeps_conflicted_with_markers() {
    let fx = conflicted_fixture("resolve-both");
    merge_feature_conflicted(&fx.repo);

    ENGINE
        .conflict_resolve(&fx.repo, "conflict1.txt", ConflictResolution::Both, None)
        .expect("resolve both");

    // Standard 2-way markers with the path as label; ours (HEAD) on top.
    assert_eq!(
        workdir_text(&fx.repo, "conflict1.txt"),
        "<<<<<<< conflict1.txt\nmain one\n=======\nfeature one\n>>>>>>> conflict1.txt\n"
    );
    // Index stays conflicted so the FE editor can finish with Custom.
    let still = ENGINE.conflicts(&fx.repo).expect("conflicts");
    assert_eq!(still.len(), 3);
    assert!(still.iter().any(|c| c.path == "conflict1.txt"));
    assert!(
        staged_content(&fx.repo, "conflict1.txt", 0).is_none(),
        "no stage-0 entry while conflicted"
    );
}

#[test]
fn conflict_resolve_after_both_preview_resolves_with_content() {
    let fx = conflicted_fixture("resolve-then-custom");
    merge_feature_conflicted(&fx.repo);

    ENGINE
        .conflict_resolve(&fx.repo, "conflict1.txt", ConflictResolution::Both, None)
        .expect("marker preview");
    assert_eq!(
        ENGINE.conflicts(&fx.repo).expect("conflicts").len(),
        3,
        "preview keeps the index conflicted"
    );
    ENGINE
        .conflict_resolve(
            &fx.repo,
            "conflict1.txt",
            ConflictResolution::Both,
            Some(b"hand merged\n".as_slice()),
        )
        .expect("resolve with content");

    assert_eq!(workdir_text(&fx.repo, "conflict1.txt"), "hand merged\n");
    assert_eq!(
        staged_content(&fx.repo, "conflict1.txt", 0).as_deref(),
        Some(b"hand merged\n".as_slice())
    );
    assert_eq!(ENGINE.conflicts(&fx.repo).expect("conflicts").len(), 2);
}

// ---------------------------------------------------------------------------
// cherry_pick
// ---------------------------------------------------------------------------

#[test]
fn cherry_pick_preserves_message_and_author() {
    let fx = init_fixture("cherry-pick");
    commit_file(&fx.repo, "base.txt", "base\n", "base");
    ENGINE
        .branch_create(&fx.repo, "feature", None, true)
        .expect("branch feature");
    let x1 = commit_file_as(
        &fx.repo,
        "widget.txt",
        "widget v1\n",
        "feat: add widget\n\nbody line\n",
        "Cherry Author",
        "cherry@example.test",
    );
    let x2 = commit_file(&fx.repo, "widget.txt", "widget v2\n", "feat: widget v2");
    ENGINE
        .branch_switch(&fx.repo, "main", false)
        .expect("switch back");
    let main_tip = commit_file(&fx.repo, "mainline.txt", "main\n", "mainline work");

    let result = ENGINE
        .cherry_pick(&fx.repo, &[x1.clone(), x2])
        .expect("cherry-pick two commits");

    assert!(matches!(result.outcome, MergeOutcome::Merged));
    let head = fx
        .repo
        .head()
        .expect("head")
        .peel_to_commit()
        .expect("commit");
    // Second pick is HEAD: message + single parent; first pick is its parent.
    assert_eq!(head.message().ok(), Some("feat: widget v2"));
    assert_eq!(head.parent_count(), 1);
    let first_pick = head.parent(0).expect("first pick");
    assert_eq!(
        first_pick.message().ok(),
        Some("feat: add widget\n\nbody line\n")
    );
    assert_eq!(first_pick.author().name().ok(), Some("Cherry Author"));
    assert_eq!(
        first_pick.author().email().ok(),
        Some("cherry@example.test")
    );
    assert_eq!(
        first_pick.committer().name().ok(),
        Some("Merge Test"),
        "committer is the local identity"
    );
    assert_eq!(
        first_pick.parent(0).expect("pick parent").id().to_string(),
        main_tip
    );
    assert_ne!(first_pick.id().to_string(), x1, "picks are new commits");
    assert_eq!(workdir_text(&fx.repo, "widget.txt"), "widget v2\n");
    assert_eq!(workdir_text(&fx.repo, "mainline.txt"), "main\n");
}

#[test]
fn cherry_pick_conflict_writes_sequencer_state() {
    let fx = init_fixture("cherry-pick-conflict");
    commit_file(&fx.repo, "shared.txt", "base\n", "base");
    ENGINE
        .branch_create(&fx.repo, "feature", None, true)
        .expect("branch feature");
    let x = commit_file(&fx.repo, "shared.txt", "feature\n", "feature edit");
    ENGINE
        .branch_switch(&fx.repo, "main", false)
        .expect("switch back");
    commit_file(&fx.repo, "shared.txt", "main\n", "main edit");

    let result = ENGINE
        .cherry_pick(&fx.repo, std::slice::from_ref(&x))
        .expect("cherry-pick");

    assert!(matches!(result.outcome, MergeOutcome::Conflicted));
    assert_eq!(result.new_head, None);
    let conflicts = ENGINE.conflicts(&fx.repo).expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, "shared.txt");
    assert_eq!(conflicts[0].source, "cherry-pick");
    assert!(conflicts[0].has_base && conflicts[0].has_ours && conflicts[0].has_theirs);
    // The interrupted pick is recorded as sequencer state.
    let pick_head = std::fs::read_to_string(fx.repo.path().join("CHERRY_PICK_HEAD"))
        .expect("CHERRY_PICK_HEAD written");
    assert!(
        pick_head.trim_start().starts_with(&x),
        "CHERRY_PICK_HEAD names the failed commit: {pick_head:?}"
    );
    assert_eq!(fx.repo.state(), RepositoryState::CherryPick);
    let status = ENGINE.status(&fx.repo).expect("status");
    assert!(status.sequencer);
    // Workdir shows the 3-way markers.
    let marked = workdir_text(&fx.repo, "shared.txt");
    assert!(marked.contains("<<<<<<<"), "markers in workdir: {marked:?}");
}

#[test]
fn cherry_pick_sequence_stops_at_first_conflict() {
    let fx = init_fixture("cherry-pick-sequence");
    commit_file(&fx.repo, "a.txt", "a base\n", "base a");
    commit_file(&fx.repo, "b.txt", "b base\n", "base b");
    ENGINE
        .branch_create(&fx.repo, "feature", None, true)
        .expect("branch feature");
    let x1 = commit_file(&fx.repo, "a.txt", "a feature\n", "feature: a");
    let x2 = commit_file(&fx.repo, "b.txt", "b feature\n", "feature: b");
    let x3 = commit_file(&fx.repo, "a.txt", "a feature 3\n", "feature: a again");
    ENGINE
        .branch_switch(&fx.repo, "main", false)
        .expect("switch back");
    commit_file(&fx.repo, "b.txt", "b main\n", "main: b");

    // x1 applies cleanly, x2 conflicts with the main-side b.txt edit, x3
    // must never be applied.
    let result = ENGINE
        .cherry_pick(&fx.repo, &[x1, x2, x3])
        .expect("cherry-pick sequence");

    assert!(matches!(result.outcome, MergeOutcome::Conflicted));
    // Exactly one new commit: the pick of x1 (reported via new_head).
    let head = fx
        .repo
        .head()
        .expect("head")
        .peel_to_commit()
        .expect("commit");
    assert_eq!(
        result.new_head.as_deref(),
        Some(head.id().to_string().as_str())
    );
    assert_eq!(head.message().ok(), Some("feature: a"));
    assert_eq!(workdir_text(&fx.repo, "a.txt"), "a feature\n");
    // The conflicted pick left standard markers over the main-side content.
    let marked = workdir_text(&fx.repo, "b.txt");
    assert!(
        marked.starts_with("<<<<<<< HEAD\nb main\n"),
        "markers: {marked:?}"
    );
    let conflicts = ENGINE.conflicts(&fx.repo).expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, "b.txt");
}

// ---------------------------------------------------------------------------
// revert
// ---------------------------------------------------------------------------

#[test]
fn revert_restores_content_and_writes_message() {
    let fx = init_fixture("revert");
    commit_file(&fx.repo, "file.txt", "one\n", "first");
    let second = commit_file(&fx.repo, "file.txt", "two\n", "second");
    let before = head_sha(&fx.repo);

    let result = ENGINE
        .revert(&fx.repo, std::slice::from_ref(&second))
        .expect("revert");

    assert!(matches!(result.outcome, MergeOutcome::Merged));
    let head = fx
        .repo
        .head()
        .expect("head")
        .peel_to_commit()
        .expect("commit");
    assert_eq!(
        head.message().ok(),
        Some(format!("Revert \"second\"\n\nThis reverts commit {second}.\n").as_str())
    );
    assert_eq!(head.parent(0).expect("parent").id().to_string(), before);
    assert_eq!(workdir_text(&fx.repo, "file.txt"), "one\n");
}

#[test]
fn revert_conflict_writes_sequencer_state() {
    let fx = init_fixture("revert-conflict");
    commit_file(&fx.repo, "file.txt", "one\n", "first");
    let second = commit_file(&fx.repo, "file.txt", "two\n", "second");
    // Diverge HEAD past the reverted commit so the reverse patch cannot apply.
    commit_file(&fx.repo, "file.txt", "three\n", "third");

    let result = ENGINE
        .revert(&fx.repo, std::slice::from_ref(&second))
        .expect("revert attempt");

    assert!(matches!(result.outcome, MergeOutcome::Conflicted));
    let conflicts = ENGINE.conflicts(&fx.repo).expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, "file.txt");
    assert_eq!(conflicts[0].source, "revert");
    let revert_head =
        std::fs::read_to_string(fx.repo.path().join("REVERT_HEAD")).expect("REVERT_HEAD written");
    assert!(
        revert_head.trim().starts_with(&second),
        "REVERT_HEAD names the reverted commit: {revert_head:?}"
    );
    assert_eq!(fx.repo.state(), RepositoryState::Revert);
    let marked = workdir_text(&fx.repo, "file.txt");
    assert!(marked.contains("<<<<<<<"), "markers in workdir: {marked:?}");
}

// ---------------------------------------------------------------------------
// sequencer_abort
// ---------------------------------------------------------------------------

/// Conflict setup shared by the abort tests: main edits `shared.txt` after
/// branching, picking the feature commit conflicts.
fn pick_conflict_fixture(name: &str) -> (Fixture, String) {
    let fx = init_fixture(name);
    commit_file(&fx.repo, "shared.txt", "base\n", "base");
    ENGINE
        .branch_create(&fx.repo, "feature", None, true)
        .expect("branch feature");
    let x = commit_file(&fx.repo, "shared.txt", "feature\n", "feature edit");
    ENGINE
        .branch_switch(&fx.repo, "main", false)
        .expect("switch back");
    commit_file(&fx.repo, "shared.txt", "main\n", "main edit");
    (fx, x)
}

fn assert_sequencer_cleared(fx: &Fixture) {
    assert!(!fx.repo.path().join("CHERRY_PICK_HEAD").exists());
    assert!(!fx.repo.path().join("REVERT_HEAD").exists());
    assert!(!fx.repo.path().join("MERGE_MSG").exists());
    assert_eq!(fx.repo.state(), RepositoryState::Clean);
    assert!(
        ENGINE.conflicts(&fx.repo).expect("conflicts").is_empty(),
        "index must be conflict-free after abort"
    );
    let status = ENGINE.status(&fx.repo).expect("status");
    assert!(!status.sequencer, "sequencer flag must drop");
}

#[test]
fn cherry_pick_abort_restores_pre_pick_state() {
    let (fx, x) = pick_conflict_fixture("pick-abort");

    let result = ENGINE
        .cherry_pick(&fx.repo, std::slice::from_ref(&x))
        .expect("cherry-pick");
    assert!(matches!(result.outcome, MergeOutcome::Conflicted));
    assert!(
        workdir_text(&fx.repo, "shared.txt").contains("<<<<<<<"),
        "conflict markers expected before abort"
    );

    ENGINE.sequencer_abort(&fx.repo).expect("abort");

    assert_sequencer_cleared(&fx);
    // Workdir and index are back at HEAD ("main\n"); the pick left no commit.
    assert_eq!(workdir_text(&fx.repo, "shared.txt"), "main\n");
    let head = fx.repo.head().expect("head").peel_to_commit().expect("commit");
    assert_eq!(head.message().ok(), Some("main edit"));
    let index = fx.repo.index().expect("index");
    assert!(!index.has_conflicts());
    let entry = index.get_path(Path::new("shared.txt"), 0).expect("staged");
    let blob = fx.repo.find_blob(entry.id).expect("blob");
    assert_eq!(String::from_utf8_lossy(blob.content()), "main\n");

    // Nothing left in progress: a second abort reports that.
    let err = ENGINE.sequencer_abort(&fx.repo).expect_err("second abort");
    assert!(
        matches!(err, EngineError::Invalid(ref m) if m.contains("no cherry-pick")),
        "unexpected error: {err:?}"
    );
}

#[test]
fn cherry_pick_abort_keeps_landed_picks_of_the_leg() {
    let fx = init_fixture("pick-abort-sequence");
    commit_file(&fx.repo, "a.txt", "a base\n", "base a");
    commit_file(&fx.repo, "b.txt", "b base\n", "base b");
    ENGINE
        .branch_create(&fx.repo, "feature", None, true)
        .expect("branch feature");
    let x1 = commit_file(&fx.repo, "a.txt", "a feature\n", "feature: a");
    let x2 = commit_file(&fx.repo, "b.txt", "b feature\n", "feature: b");
    ENGINE
        .branch_switch(&fx.repo, "main", false)
        .expect("switch back");
    commit_file(&fx.repo, "b.txt", "b main\n", "main: b");

    // x1 lands, x2 conflicts.
    let result = ENGINE
        .cherry_pick(&fx.repo, &[x1, x2])
        .expect("cherry-pick sequence");
    assert!(matches!(result.outcome, MergeOutcome::Conflicted));

    ENGINE.sequencer_abort(&fx.repo).expect("abort");

    assert_sequencer_cleared(&fx);
    // The leg's landed pick stays; the conflicted step is fully undone.
    let head = fx.repo.head().expect("head").peel_to_commit().expect("commit");
    assert_eq!(head.message().ok(), Some("feature: a"));
    assert_eq!(workdir_text(&fx.repo, "a.txt"), "a feature\n");
    assert_eq!(workdir_text(&fx.repo, "b.txt"), "b main\n");
}

#[test]
fn revert_abort_restores_pre_revert_state() {
    let fx = init_fixture("revert-abort");
    commit_file(&fx.repo, "file.txt", "one\n", "first");
    let second = commit_file(&fx.repo, "file.txt", "two\n", "second");
    commit_file(&fx.repo, "file.txt", "three\n", "third");

    let result = ENGINE
        .revert(&fx.repo, std::slice::from_ref(&second))
        .expect("revert attempt");
    assert!(matches!(result.outcome, MergeOutcome::Conflicted));

    ENGINE.sequencer_abort(&fx.repo).expect("abort");

    assert_sequencer_cleared(&fx);
    assert_eq!(workdir_text(&fx.repo, "file.txt"), "three\n");
    let head = fx.repo.head().expect("head").peel_to_commit().expect("commit");
    assert_eq!(head.message().ok(), Some("third"));
}

#[test]
fn commit_after_resolved_pick_clears_sequencer_state() {
    let (fx, x) = pick_conflict_fixture("pick-resolve-commit");

    let result = ENGINE
        .cherry_pick(&fx.repo, std::slice::from_ref(&x))
        .expect("cherry-pick");
    assert!(matches!(result.outcome, MergeOutcome::Conflicted));

    // Resolve with edited content (the FE editor path), which stages the
    // path, then commit like the resolve flow does.
    ENGINE
        .conflict_resolve(
            &fx.repo,
            "shared.txt",
            ConflictResolution::Ours,
            Some(b"merged\n" as &[u8]),
        )
        .expect("resolve");
    ENGINE
        .commit_impl(&fx.repo, &CommitOptions {
            message: "main edit (resolved pick)".into(),
            amend: false,
            no_verify: true,
            allow_empty: false,
            author: None,
        })
        .expect("commit");

    assert_sequencer_cleared(&fx);
}

// ---------------------------------------------------------------------------
// reset
// ---------------------------------------------------------------------------

/// base -> second (a.txt) -> third (adds b.txt); returns the base sha.
fn reset_fixture(name: &str) -> (Fixture, String) {
    let fx = init_fixture(name);
    let base = commit_file(&fx.repo, "a.txt", "base\n", "base");
    commit_file(&fx.repo, "a.txt", "two\n", "second");
    commit_file(&fx.repo, "b.txt", "three\n", "third");
    (fx, base)
}

#[test]
fn reset_soft_moves_ref_only() {
    let (fx, base) = reset_fixture("reset-soft");

    ENGINE
        .reset(&fx.repo, ResetKind::Soft, &base)
        .expect("soft reset");

    assert_eq!(head_sha(&fx.repo), base);
    // Index keeps everything staged (as vs the new HEAD), workdir untouched.
    assert_eq!(
        staged_content(&fx.repo, "a.txt", 0).as_deref(),
        Some(b"two\n".as_slice())
    );
    assert_eq!(
        staged_content(&fx.repo, "b.txt", 0).as_deref(),
        Some(b"three\n".as_slice())
    );
    assert_eq!(workdir_text(&fx.repo, "a.txt"), "two\n");
    assert_eq!(workdir_text(&fx.repo, "b.txt"), "three\n");
}

#[test]
fn reset_mixed_resets_index_keeps_workdir() {
    let (fx, base) = reset_fixture("reset-mixed");

    ENGINE
        .reset(&fx.repo, ResetKind::Mixed, &base)
        .expect("mixed reset");

    assert_eq!(head_sha(&fx.repo), base);
    // Index back to the base tree; workdir files still hold later content.
    assert_eq!(
        staged_content(&fx.repo, "a.txt", 0).as_deref(),
        Some(b"base\n".as_slice())
    );
    assert!(
        staged_content(&fx.repo, "b.txt", 0).is_none(),
        "b.txt left the index"
    );
    assert_eq!(workdir_text(&fx.repo, "a.txt"), "two\n");
    assert_eq!(workdir_text(&fx.repo, "b.txt"), "three\n");
}

#[test]
fn reset_hard_checks_out_target_tree() {
    let (fx, base) = reset_fixture("reset-hard");

    ENGINE
        .reset(&fx.repo, ResetKind::Hard, &base)
        .expect("hard reset");

    assert_eq!(head_sha(&fx.repo), base);
    assert_eq!(workdir_text(&fx.repo, "a.txt"), "base\n");
    assert!(
        !fx.repo.workdir().unwrap().join("b.txt").exists(),
        "b.txt removed from workdir and index"
    );
    assert_eq!(
        staged_content(&fx.repo, "a.txt", 0).as_deref(),
        Some(b"base\n".as_slice())
    );
    let status = ENGINE.status(&fx.repo).expect("status");
    assert!(status.entries.is_empty(), "fully clean after hard reset");
}

#[test]
fn reset_hard_on_unborn_head_seeds_branch() {
    let dir = TempDir::new("reset-unborn");
    let repo = init_repo(dir.path());
    // A commit on an orphan branch; `main` (HEAD) stays unborn.
    write_file(&repo, "seed.txt", "seed\n");
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .expect("add all");
    index.write().expect("write index");
    let tree_oid = index.write_tree().expect("tree");
    let tree = repo.find_tree(tree_oid).expect("tree");
    let sig = repo.signature().expect("signature");
    let seed = repo
        .commit(
            Some("refs/heads/other"),
            &sig,
            &sig,
            "seed commit",
            &tree,
            &[],
        )
        .expect("orphan commit");

    ENGINE
        .reset(&repo, ResetKind::Hard, &seed.to_string())
        .expect("hard reset onto unborn HEAD");

    assert_eq!(head_sha(&repo), seed.to_string());
    assert_eq!(branch_sha(&repo, "main"), seed.to_string());
    assert_eq!(workdir_text(&repo, "seed.txt"), "seed\n");
}

/// `git merge`'s default subject uses the remote-tracking wording when the
/// merged ref resolves under `refs/remotes/`.
#[test]
fn merge_remote_tracking_branch_message() {
    let fx = init_fixture("merge-remote-msg");
    commit_file(&fx.repo, "base.txt", "base\n", "base");
    ENGINE
        .branch_create(&fx.repo, "feature", None, true)
        .expect("branch feature");
    let feature_tip = commit_file(&fx.repo, "feature.txt", "f\n", "feature adds");
    ENGINE
        .branch_switch(&fx.repo, "main", false)
        .expect("switch back");
    commit_file(&fx.repo, "main.txt", "m\n", "main adds");
    // Simulate a fetched tracking ref pointing at the feature tip.
    let oid = git2::Oid::from_str(&feature_tip).expect("oid");
    fx.repo
        .reference("refs/remotes/origin/main", oid, true, "test: fetch")
        .expect("tracking ref");

    let result = ENGINE
        .merge_branch(&fx.repo, "origin/main", false)
        .expect("merge tracking ref");

    assert!(matches!(result.outcome, MergeOutcome::Merged));
    let head = fx
        .repo
        .head()
        .expect("head")
        .peel_to_commit()
        .expect("commit");
    assert_eq!(
        head.message().ok(),
        Some("Merge remote-tracking branch 'origin/main'")
    );
}
