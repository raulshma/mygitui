//! M12 lane A: deleted-branch trash (`refs/mygitui/trash/*`).
//!
//! Deleting an unmerged branch is the easiest way to lose commits, so
//! `branch_delete` parks every deleted tip under a hidden ref
//! `refs/mygitui/trash/<id>` before the branch ref is removed. The trash id
//! is `<sanitized-branch-name>-<unix_millis>` (sanitization: every character
//! outside `[A-Za-z0-9._-]` becomes `_`), so ids sort stably and the
//! deletion timestamp is recoverable from the id alone.
//!
//! The ORIGINAL branch name travels in the trash ref's reflog message
//! (`mygitui-trash: <name>`); libgit2 only writes reflogs for well-known
//! namespaces, so the entry is appended by hand and is best-effort — when
//! the reflog is missing the listing falls back to the sanitized name.
//!
//! The contract lives in [`super::git_engine::GitEngineM12`]; this module
//! owns the crate's single `impl GitEngineM12 for Libgit2Engine` block (Rust
//! coherence allows only one) and forwards `worktree_prune` to its
//! `*_impl` twin in [`super::stash`].

use std::time::{SystemTime, UNIX_EPOCH};

use git2::{BranchType, Oid, Repository};

use super::git_engine::{EngineError, EngineResult, GitEngineM12};
use super::libgit2::Libgit2Engine;
use super::types::{BisectLogEntry, BranchTrashEntry, CommitSignature};

/// Namespace holding all trash refs.
const TRASH_PREFIX: &str = "refs/mygitui/trash/";

/// Reflog-message marker carrying the original branch name.
const NAME_MARKER: &str = "mygitui-trash: ";

/// Ref-safe id fragment for a branch name: every character outside
/// `[A-Za-z0-9._-]` becomes `_`.
pub(crate) fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Park `branch_name`'s tip `sha` under a fresh trash ref; returns the trash
/// id. The reflog message carries the verbatim name (best-effort).
pub(crate) fn trash_create(
    repo: &Repository,
    branch_name: &str,
    sha: &str,
) -> EngineResult<String> {
    let oid = Oid::from_str(sha)
        .map_err(|e| EngineError::Invalid(format!("cannot trash `{branch_name}`: bad sha: {e}")))?;
    let id = format!("{}-{}", sanitize_name(branch_name), unix_millis());
    let ref_name = format!("{TRASH_PREFIX}{id}");
    repo.reference(&ref_name, oid, true, "mygitui: branch trash")?;
    // libgit2 skips reflogs for custom namespaces: materialize the log and
    // append the verbatim name so the listing can restore it losslessly.
    if let Ok(mut reflog) = repo.reflog(&ref_name) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let sig = git2::Signature::new("mygitui", "mygitui@local", &git2::Time::new(now, 0))?;
        let message = format!("{NAME_MARKER}{branch_name}");
        if reflog.append(oid, &sig, Some(&message)).is_ok() {
            let _ = reflog.write();
        }
    }
    Ok(id)
}

/// All trashed branches, newest first. `deleted_at` is parsed from the id's
/// unix-millis suffix; `name` prefers the reflog-stored verbatim name.
pub(crate) fn trash_list_impl(repo: &Repository) -> EngineResult<Vec<BranchTrashEntry>> {
    let mut out = Vec::new();
    for reference in repo.references_glob(&format!("{TRASH_PREFIX}*"))? {
        let reference = reference?;
        let Some(full) = reference.name().ok().map(str::to_owned) else {
            continue; // non-UTF-8 ref name; skip rather than fail the listing
        };
        let id = full.strip_prefix(TRASH_PREFIX).unwrap_or(&full).to_owned();
        let sha = reference
            .peel_to_commit()
            .map(|c| c.id().to_string())
            .or_else(|_| {
                reference
                    .target()
                    .map(|o| o.to_string())
                    .ok_or_else(|| EngineError::Invalid(format!("trash ref `{full}` unresolvable")))
            })?;
        out.push(BranchTrashEntry {
            deleted_at: deleted_at_from_id(&id),
            name: entry_name(repo, &full, &id),
            id,
            sha,
        });
    }
    out.sort_by(|a, b| {
        b.deleted_at
            .cmp(&a.deleted_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(out)
}

/// Restore a trashed branch: recreate `new_name` (or the recorded name) at
/// the trash ref's peeled commit and drop the trash ref. Refuses when the
/// target name already exists (pick a new name instead).
pub(crate) fn trash_restore_impl(
    repo: &Repository,
    id: &str,
    new_name: Option<&str>,
) -> EngineResult<String> {
    let ref_name = format!("{TRASH_PREFIX}{id}");
    let mut reference = repo
        .find_reference(&ref_name)
        .map_err(|_| EngineError::Invalid(format!("trash entry `{id}` not found")))?;
    let commit = reference.peel_to_commit().map_err(|e| {
        EngineError::Invalid(format!("trash entry `{id}` is not a commit anymore: {e}"))
    })?;
    let recorded = entry_name(repo, &ref_name, id);
    let name = new_name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(&recorded);
    if repo.find_branch(name, BranchType::Local).is_ok() {
        return Err(EngineError::Invalid(format!(
            "branch `{name}` already exists; restore under a different name"
        )));
    }
    repo.branch(name, &commit, false)?;
    reference.delete()?;
    Ok(name.to_owned())
}

/// Original branch name for a trash ref: the verbatim name from the reflog
/// marker when present, else the sanitized fragment of the id.
fn entry_name(repo: &Repository, ref_name: &str, id: &str) -> String {
    if let Ok(reflog) = repo.reflog(ref_name) {
        for entry in reflog.iter() {
            let message = entry
                .message()
                .ok()
                .flatten()
                .unwrap_or_default()
                .trim()
                .to_owned();
            if let Some(name) = message.strip_prefix(NAME_MARKER) {
                let name = name.trim();
                if !name.is_empty() {
                    return name.to_owned();
                }
            }
        }
    }
    name_from_id(id)
}

/// Name fragment of a trash id: everything before the trailing `-<millis>`.
fn name_from_id(id: &str) -> String {
    match id.rsplit_once('-') {
        Some((name, millis)) if is_millis(millis) => name.to_owned(),
        _ => id.to_owned(),
    }
}

/// Deletion time (unix seconds) parsed from a trash id's millis suffix.
fn deleted_at_from_id(id: &str) -> i64 {
    id.rsplit_once('-')
        .and_then(|(_, millis)| millis.parse::<i64>().ok())
        .map(|millis| millis / 1000)
        .unwrap_or(0)
}

fn is_millis(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// The crate's single M12 trait impl block (Rust coherence: one
/// `impl GitEngineM12 for Libgit2Engine` per type). Lanes B (signing.rs) and
/// the bisect lane own `commit_signature_impl` / `bisect_log_impl` — the
/// forwarding lives here, per their WIRING NOTEs.
impl GitEngineM12 for Libgit2Engine {
    fn branch_trash_list(&self, repo: &Repository) -> EngineResult<Vec<BranchTrashEntry>> {
        trash_list_impl(repo)
    }

    fn branch_trash_restore(
        &self,
        repo: &Repository,
        id: &str,
        new_name: Option<&str>,
    ) -> EngineResult<String> {
        trash_restore_impl(repo, id, new_name)
    }

    fn worktree_prune(&self, repo: &Repository) -> EngineResult<u32> {
        // Implementation lives with the other worktree ops (stash.rs); the
        // trait impl block itself can only exist once, hence here.
        self.worktree_prune_impl(repo)
    }

    fn commit_signature(&self, repo: &Repository, sha: &str) -> EngineResult<CommitSignature> {
        self.commit_signature_impl(repo, sha)
    }

    fn bisect_log(&self, repo: &Repository) -> EngineResult<Vec<BisectLogEntry>> {
        self.bisect_log_impl(repo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::IndexAddOption;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("mygitui-trash-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let dir = self.0.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let _ = std::fs::remove_dir_all(dir);
            });
        }
    }

    fn init_repo(path: &std::path::Path) -> Repository {
        let repo = Repository::init(path).expect("init temp repo");
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Trash Test")?;
                c.set_str("user.email", "trash@test.local")
            })
            .expect("configure identity");
        repo
    }

    fn commit_file(repo: &Repository, path: &str, content: &str, message: &str) -> String {
        let file = repo.workdir().unwrap().join(path);
        std::fs::write(file, content).expect("write file");
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
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit")
            .to_string()
    }

    fn now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn sanitize_keeps_ref_safe_characters_only() {
        assert_eq!(sanitize_name("feature/foo.bar"), "feature_foo.bar");
        assert_eq!(sanitize_name("my-branch_1.2"), "my-branch_1.2");
        assert_eq!(sanitize_name("a b:c"), "a_b_c");
        assert_eq!(sanitize_name(""), "");
    }

    #[test]
    fn delete_list_roundtrip_recovers_name_and_sha() {
        let dir = TempDir::new("roundtrip");
        let repo = init_repo(&dir.0);
        let tip = commit_file(&repo, "a.txt", "one\n", "one");

        let engine = Libgit2Engine;
        engine
            .branch_create_impl(&repo, "feature/foo.bar", None, false)
            .unwrap();
        // Advance main so the feature branch is unmerged (deletable with force).
        commit_file(&repo, "b.txt", "two\n", "two");
        engine
            .branch_delete_impl(&repo, "feature/foo.bar", true)
            .expect("delete parks the branch in trash");

        // The branch ref is gone; exactly one trash entry remains.
        assert!(repo
            .find_branch("feature/foo.bar", BranchType::Local)
            .is_err());
        let trash = engine.branch_trash_list(&repo).expect("trash list");
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].sha, tip, "trash holds the deleted tip");
        assert_eq!(
            trash[0].name, "feature/foo.bar",
            "verbatim name from reflog"
        );
        assert!(
            trash[0].id.starts_with("feature_foo.bar-"),
            "id = sanitized name + millis stamp: {}",
            trash[0].id
        );
        let millis = trash[0].id.rsplit('-').next().unwrap_or_default();
        assert!(
            !millis.is_empty() && millis.bytes().all(|b| b.is_ascii_digit()),
            "id ends with a unix-millis stamp: {}",
            trash[0].id
        );
        assert!(
            (now_secs() - trash[0].deleted_at).abs() < 60,
            "deleted_at parsed from the id suffix"
        );
        assert!(
            repo.find_reference(&format!("refs/mygitui/trash/{}", trash[0].id))
                .is_ok(),
            "hidden trash ref exists"
        );
    }

    #[test]
    fn restore_recreates_branch_at_original_sha_and_clears_trash() {
        let dir = TempDir::new("restore");
        let repo = init_repo(&dir.0);
        let _base = commit_file(&repo, "a.txt", "one\n", "one");
        // The branch is created from HEAD, so its tip is "two".
        let branch_tip = commit_file(&repo, "b.txt", "two\n", "two");

        let engine = Libgit2Engine;
        engine
            .branch_create_impl(&repo, "topic", None, false)
            .unwrap();
        engine.branch_delete_impl(&repo, "topic", true).unwrap();
        let id = engine.branch_trash_list(&repo).unwrap()[0].id.clone();

        let restored = engine
            .branch_trash_restore(&repo, &id, None)
            .expect("restore");
        assert_eq!(restored, "topic");
        assert_eq!(
            repo.find_branch("topic", BranchType::Local)
                .unwrap()
                .get()
                .peel_to_commit()
                .unwrap()
                .id()
                .to_string(),
            branch_tip,
            "branch recreated at the trashed sha"
        );
        assert!(
            engine.branch_trash_list(&repo).unwrap().is_empty(),
            "trash ref deleted after restore"
        );
    }

    #[test]
    fn restore_under_new_name() {
        let dir = TempDir::new("restore-rename");
        let repo = init_repo(&dir.0);
        commit_file(&repo, "a.txt", "one\n", "one");

        let engine = Libgit2Engine;
        engine
            .branch_create_impl(&repo, "old-name", None, false)
            .unwrap();
        engine.branch_delete_impl(&repo, "old-name", true).unwrap();
        let id = engine.branch_trash_list(&repo).unwrap()[0].id.clone();

        let restored = engine
            .branch_trash_restore(&repo, &id, Some("new-name"))
            .expect("restore renamed");
        assert_eq!(restored, "new-name");
        assert!(repo.find_branch("new-name", BranchType::Local).is_ok());
        assert!(repo.find_branch("old-name", BranchType::Local).is_err());
    }

    #[test]
    fn restore_when_name_exists_errors_and_keeps_trash() {
        let dir = TempDir::new("restore-collide");
        let repo = init_repo(&dir.0);
        let tip = commit_file(&repo, "a.txt", "one\n", "one");
        commit_file(&repo, "b.txt", "two\n", "two");

        let engine = Libgit2Engine;
        engine
            .branch_create_impl(&repo, "topic", None, false)
            .unwrap();
        engine.branch_delete_impl(&repo, "topic", true).unwrap();
        let id = engine.branch_trash_list(&repo).unwrap()[0].id.clone();

        // Recreate a DIFFERENT branch with the same name; restore must refuse.
        engine
            .branch_create_impl(&repo, "topic", Some("HEAD"), false)
            .unwrap();
        match engine.branch_trash_restore(&repo, &id, None) {
            Err(EngineError::Invalid(msg)) => {
                assert!(msg.contains("already exists"), "{msg}")
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
        // The trash entry survives so the commits stay reachable.
        assert_eq!(engine.branch_trash_list(&repo).unwrap().len(), 1);
        let _ = tip;
    }

    #[test]
    fn restore_unknown_id_is_invalid() {
        let dir = TempDir::new("restore-missing");
        let repo = init_repo(&dir.0);
        match trash_restore_impl(&repo, "ghost-123", None) {
            Err(EngineError::Invalid(msg)) => assert!(msg.contains("ghost"), "{msg}"),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn list_orders_newest_first_and_parses_deleted_at() {
        let dir = TempDir::new("ordering");
        let repo = init_repo(&dir.0);
        let tip = commit_file(&repo, "a.txt", "one\n", "one");

        // Two hand-forged trash refs with distinct ages.
        let oid = repo
            .revparse_single(&tip)
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id();
        let old_id = format!("older-{}", unix_millis() - 60_000);
        let new_id = format!("newer-{}", unix_millis());
        repo.reference(&format!("{TRASH_PREFIX}{old_id}"), oid, true, "test")
            .unwrap();
        repo.reference(&format!("{TRASH_PREFIX}{new_id}"), oid, true, "test")
            .unwrap();

        let list = trash_list_impl(&repo).expect("list");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, new_id, "newest first");
        assert_eq!(list[1].id, old_id);
        assert_eq!(
            list[0].name, "newer",
            "sanitized-name fallback without reflog"
        );
        assert!(list[0].deleted_at > list[1].deleted_at);
        assert_eq!(list[0].sha, tip);
    }
}
