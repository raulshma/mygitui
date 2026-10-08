//! Checkpoint + preview engine tests (lane D4).
//!
//! Everything runs against throwaway repositories under
//! `std::env::temp_dir()` (no fixtures/) so parallel lanes' files stay
//! untouched. The methods under test are the inherent `*_impl` twins of the
//! `GitEngineM3` checkpoint methods plus the shared `preview_impl` the IPC
//! `ops_preview` command factors through.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use git2::{
    IndexAddOption, Oid, Repository, RepositoryInitOptions, Signature, StatusOptions, Time,
};

use super::git_engine::EngineError;
use super::libgit2::Libgit2Engine;

const ENGINE: Libgit2Engine = Libgit2Engine;
const PREFIX: &str = "refs/mygitui/checkpoints/";
const DAY_SECS: i64 = 86_400;

// ---------------------------------------------------------------------------
// temp scaffolding (mirrors stash_tests.rs)
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir =
            std::env::temp_dir().join(format!("mygitui-checkpoint-{}-{name}", std::process::id()));
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
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
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
            c.set_str("user.name", "Checkpoint Test")?;
            c.set_str("user.email", "checkpoint@test.local")?;
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

fn file_exists(repo: &Repository, path: &str) -> bool {
    repo.workdir().unwrap().join(path).exists()
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

/// Commit with an explicit parent WITHOUT moving HEAD (builds branch history
/// for previews); returns the new sha.
fn commit_with_parent(
    repo: &Repository,
    parent_sha: &str,
    path: &str,
    content: &str,
    message: &str,
) -> String {
    write_file(repo, path, content);
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .expect("add all");
    index.write().expect("write index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("tree");
    let sig = repo.signature().expect("signature");
    let parent = repo
        .revparse_single(parent_sha)
        .expect("parent")
        .peel_to_commit()
        .expect("parent commit");
    let oid = repo
        .commit(None, &sig, &sig, message, &tree, &[&parent])
        .expect("commit");
    oid.to_string()
}

/// Forge an aged checkpoint (commit backdated `age_secs`, hidden ref) to
/// exercise the GC cutoff without sleeping for days.
fn forge_old_checkpoint(repo: &Repository, parent_sha: &str, name: &str, age_secs: i64) -> String {
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .expect("add all");
    index.write().expect("write index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("tree");
    let parent = repo
        .revparse_single(parent_sha)
        .expect("parent")
        .peel_to_commit()
        .expect("parent commit");
    let when = now_secs() - age_secs;
    let sig = Signature::new("Old Test", "old@test.local", &Time::new(when, 0))
        .expect("backdated signature");
    let message =
        format!("checkpoint: aged-{name}\n\nmygitui-checkpoint\nbranch: main\nworktree: yes\n");
    let oid = repo
        .commit(None, &sig, &sig, &message, &tree, &[&parent])
        .expect("backdated commit");
    let ref_name = format!("{PREFIX}{}-aged-{name}", now_millis());
    repo.reference(&ref_name, oid, true, "checkpoint: create")
        .expect("forge ref");
    ref_name
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

fn blob_content(repo: &Repository, tree: &git2::Tree<'_>, path: &str) -> String {
    let entry = tree.get_path(Path::new(path)).expect("tree entry");
    let blob = repo.find_blob(entry.id()).expect("blob");
    String::from_utf8_lossy(blob.content()).into_owned()
}

fn status_entry_count(repo: &Repository) -> usize {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    repo.statuses(Some(&mut opts)).expect("statuses").len()
}

fn expect_invalid(err: EngineError) -> String {
    match err {
        EngineError::Invalid(msg) => msg,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// Small pause so consecutive checkpoints get distinct millisecond stamps
/// (deterministic newest-first ordering via the ref-name tie-break).
fn tick() {
    thread::sleep(Duration::from_millis(5));
}

// ---------------------------------------------------------------------------
// checkpoint_create + checkpoints
// ---------------------------------------------------------------------------

#[test]
fn checkpoint_captures_dirty_workdir_and_lists_newest_first() {
    let dir = TempDir::new("create-list");
    let repo = init_repo(dir.path());
    let c1 = commit_file(&repo, "a.txt", "one\n", "base");

    // Dirty workdir: modified tracked file + untracked file.
    write_file(&repo, "a.txt", "two\n");
    write_file(&repo, "u.txt", "untracked\n");

    let info = ENGINE
        .checkpoint_create_impl(&repo, "before rebase")
        .expect("checkpoint create");

    // Hidden ref exists with the documented shape; id = ref suffix.
    assert!(info.ref_name.starts_with(PREFIX));
    assert!(
        info.ref_name.ends_with("-before-rebase"),
        "{}",
        info.ref_name
    );
    assert_eq!(info.id, info.ref_name.strip_prefix(PREFIX).unwrap());
    assert!(
        repo.find_reference(&info.ref_name).is_ok(),
        "snapshot ref must exist"
    );
    assert_eq!(info.reason, "before rebase");
    assert_eq!(info.branch.as_deref(), Some("main"));
    assert!(info.has_worktree_state);
    assert!(info.created_at > 0);

    // The snapshot tree holds the WORKDIR state: modified content AND the
    // untracked file.
    let snapshot = repo
        .find_reference(&info.ref_name)
        .unwrap()
        .peel_to_commit()
        .expect("snapshot commit");
    assert_eq!(snapshot.parent(0).unwrap().id().to_string(), c1);
    let tree = snapshot.tree().expect("snapshot tree");
    assert_eq!(blob_content(&repo, &tree, "a.txt"), "two\n");
    assert_eq!(blob_content(&repo, &tree, "u.txt"), "untracked\n");
    assert!(snapshot.message().unwrap().contains("mygitui-checkpoint"));

    // Taking a checkpoint must not stage anything: the on-disk index still
    // matches HEAD (nothing reports an index-side delta).
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut opts)).expect("statuses");
    for entry in statuses.iter() {
        assert!(
            entry.head_to_index().is_none(),
            "checkpoint must not touch the index"
        );
    }

    // Further changes -> second checkpoint; listing is newest first.
    tick();
    write_file(&repo, "a.txt", "three\n");
    let info2 = ENGINE
        .checkpoint_create_impl(&repo, "half done")
        .expect("second checkpoint");

    let list = ENGINE.checkpoints_impl(&repo).expect("checkpoints");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, info2.id, "newest first");
    assert_eq!(list[0].reason, "half done");
    assert_eq!(list[1].id, info.id);
    assert_eq!(list[1].reason, "before rebase");
    // Branch trailer parsed from every snapshot.
    assert!(list.iter().all(|c| c.branch.as_deref() == Some("main")));
}

#[test]
fn checkpoint_reason_is_sanitized_into_ref_name() {
    let dir = TempDir::new("sanitize");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    let info = ENGINE
        .checkpoint_create_impl(&repo, "Fix BUG #42!")
        .expect("checkpoint");
    // `<millis>-fix-bug-42`: lowercase, digits and dashes only.
    assert!(info.id.ends_with("-fix-bug-42"), "{}", info.id);
    let stamp = info.id.strip_suffix("-fix-bug-42").unwrap();
    assert!(
        !stamp.is_empty() && stamp.chars().all(|c| c.is_ascii_digit()),
        "{info:?}"
    );

    // A reason with nothing ref-safe falls back.
    let info2 = ENGINE
        .checkpoint_create_impl(&repo, "///")
        .expect("fallback checkpoint");
    assert!(
        info2.ref_name.ends_with("-checkpoint"),
        "{}",
        info2.ref_name
    );
    assert_eq!(info2.reason, "///");
}

// ---------------------------------------------------------------------------
// checkpoint_restore
// ---------------------------------------------------------------------------

#[test]
fn restore_returns_workdir_and_index_to_exact_snapshot() {
    let dir = TempDir::new("restore");
    let repo = init_repo(dir.path());
    let c1 = commit_file(&repo, "a.txt", "one\n", "base");

    write_file(&repo, "a.txt", "two\n");
    write_file(&repo, "u1.txt", "first-untracked\n");
    let first = ENGINE
        .checkpoint_create_impl(&repo, "first")
        .expect("checkpoint 1");

    tick();
    write_file(&repo, "a.txt", "three\n");
    write_file(&repo, "u2.txt", "second-untracked\n");
    ENGINE
        .checkpoint_create_impl(&repo, "second")
        .expect("checkpoint 2");

    ENGINE
        .checkpoint_restore_impl(&repo, &first.id)
        .expect("restore");

    // Workdir matches the FIRST snapshot exactly: tracked content back to
    // "two", the file that existed then restored, the file created after it
    // GONE (remove_untracked).
    assert_eq!(read_file(&repo, "a.txt"), "two\n");
    assert_eq!(read_file(&repo, "u1.txt"), "first-untracked\n");
    assert!(
        !file_exists(&repo, "u2.txt"),
        "post-checkpoint file removed"
    );

    // Index matches the snapshot tree.
    let snapshot_tree = repo
        .find_reference(&first.ref_name)
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap();
    let index = repo.index().unwrap();
    let diff = repo
        .diff_tree_to_index(Some(&snapshot_tree), Some(&index), None)
        .unwrap();
    assert_eq!(diff.deltas().count(), 0, "index must match the snapshot");

    // HEAD/branch never moved.
    assert_eq!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        c1
    );

    // SAFETY: the restore auto-created a pre-restore checkpoint, newest in
    // the list.
    let list = ENGINE.checkpoints_impl(&repo).expect("list after restore");
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].reason, "pre-restore");
    assert_eq!(list[1].reason, "second");
    assert_eq!(list[2].reason, "first");
}

#[test]
fn restore_unknown_id_is_invalid() {
    let dir = TempDir::new("restore-missing");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    let err = ENGINE
        .checkpoint_restore_impl(&repo, "nope")
        .expect_err("unknown checkpoint");
    assert_eq!(expect_invalid(err), "checkpoint `nope` not found");
    assert!(ENGINE.checkpoints_impl(&repo).unwrap().is_empty());
}

#[test]
fn checkpoint_on_unborn_head_roundtrips() {
    let dir = TempDir::new("unborn");
    let repo = init_repo(dir.path());
    write_file(&repo, "wip.txt", "wip content\n");

    let info = ENGINE
        .checkpoint_create_impl(&repo, "no commits yet")
        .expect("checkpoint on unborn HEAD");
    // Branch name still resolved from the unborn HEAD's symbolic target.
    assert_eq!(info.branch.as_deref(), Some("main"));

    let snapshot = repo
        .find_reference(&info.ref_name)
        .unwrap()
        .peel_to_commit()
        .expect("root snapshot");
    assert_eq!(snapshot.parent_count(), 0, "root snapshot has no parent");
    let tree = snapshot.tree().unwrap();
    assert_eq!(blob_content(&repo, &tree, "wip.txt"), "wip content\n");

    // Overwrite and restore: content comes back.
    write_file(&repo, "wip.txt", "changed\n");
    ENGINE
        .checkpoint_restore_impl(&repo, &info.id)
        .expect("restore root snapshot");
    assert_eq!(read_file(&repo, "wip.txt"), "wip content\n");
}

// ---------------------------------------------------------------------------
// checkpoint_gc
// ---------------------------------------------------------------------------

#[test]
fn gc_zero_days_keeps_only_newest_and_thirty_keeps_all() {
    let dir = TempDir::new("gc-zero");
    let repo = init_repo(dir.path());
    let base = commit_file(
        &repo, "a.txt", "one
", "base",
    );

    // Backdated refs make the cutoff deterministic — no wall-clock sleeps.
    let oldest = forge_old_checkpoint(&repo, &base, "one", 300);
    let middle = forge_old_checkpoint(&repo, &oldest, "two", 200);
    let newest = forge_old_checkpoint(&repo, &middle, "three", 100);

    let deleted = ENGINE.checkpoint_gc_impl(&repo, 0).expect("gc 0 days");
    assert_eq!(deleted, 2, "all but the newest are deleted");
    let list = ENGINE.checkpoints_impl(&repo).expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].ref_name, newest, "safety floor keeps the newest");

    // Fresh checkpoints: nothing is older than 30 days, none deleted.
    let deleted = ENGINE.checkpoint_gc_impl(&repo, 30).expect("gc 30 days");
    assert_eq!(deleted, 0);
    assert_eq!(ENGINE.checkpoints_impl(&repo).unwrap().len(), 1);
}

#[test]
fn gc_cutoff_deletes_aged_checkpoints_but_never_the_newest() {
    let dir = TempDir::new("gc-cutoff");
    let repo = init_repo(dir.path());
    let c1 = commit_file(&repo, "a.txt", "one\n", "base");

    // Two aged snapshots, no fresh one: the newest aged one survives (safety
    // floor applies regardless of age).
    forge_old_checkpoint(&repo, &c1, "oldest", 3 * DAY_SECS);
    tick();
    forge_old_checkpoint(&repo, &c1, "newer-old", 2 * DAY_SECS);

    let deleted = ENGINE.checkpoint_gc_impl(&repo, 1).expect("gc 1 day");
    assert_eq!(
        deleted, 1,
        "only checkpoints strictly older than the cutoff"
    );
    let left = ENGINE.checkpoints_impl(&repo).expect("list");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].reason, "aged-newer-old");

    // A fresh checkpoint becomes the newest; the aged one now falls under
    // the cutoff and is collected.
    tick();
    write_file(&repo, "a.txt", "v2\n");
    let fresh = ENGINE.checkpoint_create_impl(&repo, "fresh").unwrap();
    let deleted = ENGINE.checkpoint_gc_impl(&repo, 1).expect("gc again");
    assert_eq!(deleted, 1);
    let left = ENGINE.checkpoints_impl(&repo).expect("list");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, fresh.id);
}

// ---------------------------------------------------------------------------
// previews (pure — no mutation)
// ---------------------------------------------------------------------------

#[test]
fn preview_reset_hard_lists_files_and_never_mutates() {
    let dir = TempDir::new("preview-reset");
    let repo = init_repo(dir.path());
    let c1 = commit_file(&repo, "a.txt", "one\n", "base");
    let c2 = commit_file(&repo, "b.txt", "bee\n", "add b");

    // Dirty workdir on top of HEAD: modified + untracked.
    write_file(&repo, "a.txt", "dirty\n");
    write_file(&repo, "u.txt", "untracked\n");
    let dirty_before = status_entry_count(&repo);

    let preview = ENGINE
        .preview_impl(&repo, "reset_hard", &serde_json::json!({ "to": c1 }))
        .expect("preview");

    // HEAD tree -> target tree: only b.txt disappears (it exists only in
    // HEAD; a.txt is identical in both trees).
    assert_eq!(preview.kind, "reset_hard");
    assert_eq!(preview.files.len(), 1);
    let b = preview.files.iter().find(|f| f.path == "b.txt").unwrap();
    assert_eq!(b.change, "deleted");
    assert_eq!(
        preview.summary,
        format!(
            "Hard reset to {}: 1 files change, {dirty_before} uncommitted changes are discarded",
            &c1[..7]
        )
    );

    // Pure: status count, workdir contents and HEAD all unchanged.
    assert_eq!(status_entry_count(&repo), dirty_before);
    assert_eq!(read_file(&repo, "a.txt"), "dirty\n");
    assert!(file_exists(&repo, "u.txt"));
    assert_eq!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        c2,
        "HEAD must not move"
    );
}

#[test]
fn preview_clean_lists_untracked_paths_only() {
    let dir = TempDir::new("preview-clean");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");
    write_file(&repo, "a.txt", "dirty\n"); // tracked dirt: never listed
    write_file(&repo, "u.txt", "untracked\n");
    write_file(&repo, "d/c.txt", "c\n");
    write_file(&repo, "d/e.txt", "e\n");

    // dirs=false: an untracked directory collapses to a single "d/" path.
    let preview = ENGINE
        .preview_impl(&repo, "clean", &serde_json::json!({ "dirs": false }))
        .expect("preview clean");
    assert_eq!(preview.kind, "clean");
    assert_eq!(preview.files.len(), 2, "{:?}", preview.files);
    assert!(preview.files.iter().any(|f| f.path == "u.txt"));
    assert!(
        preview.files.iter().any(|f| f.path == "d/"),
        "{:?}",
        preview.files
    );
    assert!(preview.files.iter().all(|f| f.change == "deleted"));
    assert!(!preview.files.iter().any(|f| f.path == "a.txt"));
    assert_eq!(preview.summary, "Clean removes 2 untracked paths");

    // dirs=true: recurse, listing every file inside.
    let preview = ENGINE
        .preview_impl(&repo, "clean", &serde_json::json!({ "dirs": true }))
        .expect("preview clean -d");
    assert_eq!(preview.files.len(), 3, "{:?}", preview.files);
    assert!(preview.files.iter().any(|f| f.path == "d/c.txt"));
    assert!(preview.files.iter().any(|f| f.path == "d/e.txt"));
    assert_eq!(preview.summary, "Clean removes 3 untracked paths");

    // Pure: nothing was deleted.
    assert!(file_exists(&repo, "u.txt"));
    assert!(file_exists(&repo, "d/c.txt"));
    assert_eq!(read_file(&repo, "a.txt"), "dirty\n");
}

#[test]
fn preview_checkout_force_lists_branch_diff_and_never_mutates() {
    let dir = TempDir::new("preview-checkout");
    let repo = init_repo(dir.path());
    let c1 = commit_file(&repo, "a.txt", "one\n", "base");
    repo.branch(
        "feature",
        &repo.revparse_single(&c1).unwrap().peel_to_commit().unwrap(),
        false,
    )
    .expect("feature branch");
    commit_file(&repo, "b.txt", "bee\n", "main-only commit");

    let preview = ENGINE
        .preview_impl(
            &repo,
            "checkout_force",
            &serde_json::json!({ "name": "feature" }),
        )
        .expect("preview");

    // HEAD -> feature tip (= c1): only b.txt would vanish (added after the
    // branch point; a.txt is identical on both sides).
    assert_eq!(preview.kind, "checkout_force");
    assert_eq!(preview.files.len(), 1);
    assert!(preview
        .files
        .iter()
        .any(|f| f.path == "b.txt" && f.change == "deleted"));
    assert!(
        preview.summary.starts_with("Force checkout to feature ("),
        "{}",
        preview.summary
    );
    assert!(preview
        .summary
        .ends_with("1 files change, 0 uncommitted changes are discarded"));

    // Pure: still on main with an intact workdir.
    assert_eq!(read_file(&repo, "b.txt"), "bee\n");
    assert_eq!(read_file(&repo, "a.txt"), "one\n");
    assert!(
        repo.find_reference("HEAD")
            .unwrap()
            .symbolic_target()
            .unwrap()
            == Some("refs/heads/main"),
        "HEAD must not move"
    );
}

#[test]
fn preview_branch_delete_unmerged_lists_orphaned_commits() {
    let dir = TempDir::new("preview-branch-delete");
    let repo = init_repo(dir.path());
    let c1 = commit_file(&repo, "a.txt", "one\n", "base");

    // Two commits reachable ONLY from feature (HEAD stays on main).
    let f1 = commit_with_parent(&repo, &c1, "f.txt", "f1\n", "feature one");
    let f2 = commit_with_parent(&repo, &f1, "f.txt", "f2\n", "feature two");
    repo.reference(
        "refs/heads/feature",
        Oid::from_str(&f2).unwrap(),
        true,
        "test: feature",
    )
    .expect("feature ref");

    let preview = ENGINE
        .preview_impl(
            &repo,
            "branch_delete",
            &serde_json::json!({ "name": "feature", "into": "main" }),
        )
        .expect("preview");
    assert_eq!(preview.kind, "branch_delete");
    assert_eq!(preview.summary, "UNMERGED: 2 commits would be unreachable");
    let shas: Vec<&str> = preview.files.iter().map(|f| f.path.as_str()).collect();
    assert!(shas.contains(&f1.as_str()));
    assert!(shas.contains(&f2.as_str()));
    let summaries: Vec<&str> = preview.files.iter().map(|f| f.change.as_str()).collect();
    assert!(summaries.contains(&"feature one"));
    assert!(summaries.contains(&"feature two"));

    // Merged branch: empty preview with the documented summary.
    let main_tip = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("merged", &main_tip, false)
        .expect("merged branch");
    let preview = ENGINE
        .preview_impl(
            &repo,
            "branch_delete",
            &serde_json::json!({ "name": "merged" }),
        )
        .expect("preview merged");
    assert_eq!(preview.summary, "merged into HEAD");
    assert!(preview.files.is_empty());
}

#[test]
fn preview_unknown_kind_and_missing_params_are_invalid() {
    let dir = TempDir::new("preview-invalid");
    let repo = init_repo(dir.path());
    commit_file(&repo, "a.txt", "one\n", "base");

    let err = ENGINE
        .preview_impl(&repo, "explode", &serde_json::json!({}))
        .expect_err("unknown kind");
    assert_eq!(expect_invalid(err), "unknown preview kind `explode`");

    let err = ENGINE
        .preview_impl(&repo, "reset_hard", &serde_json::json!({}))
        .expect_err("missing `to`");
    assert!(
        expect_invalid(err).contains("requires string param `to`"),
        "unexpected error"
    );
}
