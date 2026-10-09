//! Stash, worktrees, and reflog — lane D3.
//!
//! Implementations live as inherent `*_impl` methods on `Libgit2Engine`
//! (the M2 lane convention); the single `impl GitEngineM3 for Libgit2Engine`
//! forwarding block forwards each trait method to its `*_impl` twin.
//!
//! Semantics notes:
//! - `stash_list` walks the `refs/stash` reflog; index 0 is the newest entry
//!   (`stash@{0}`), matching `git stash list`. The reflog message is kept
//!   as-is: libgit2 stores git's formats verbatim — the default
//!   `"WIP on <branch>: <sha> <subject>"` or `"On <branch>: <caller message>"`
//!   when a message is passed to `stash_push`.
//! - git2 0.21 has no `stash_branch` binding, so `stash_branch` follows the
//!   documented `git stash branch <name>` behavior manually: create + check
//!   out a branch at the commit the stash was based on (the stash commit's
//!   first parent), apply the stash, and drop it once applied cleanly.
//! - The stash mutation APIs (`stash_save2`/`stash_apply`/`stash_drop`) need
//!   `&mut Repository` while the trait hands out `&Repository`; those ops
//!   reopen the same gitdir into a fresh owned handle (cheap metadata reads;
//!   the outer handle's index cache revalidates from disk).
//! - `worktrees` leads with the main worktree (`is_main: true`; libgit2's
//!   `git_worktree_list` never includes it) followed by the linked ones;
//!   each entry's HEAD is read from its admin dir under the common gitdir.

use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{
    BranchType, ErrorCode, Repository, StashFlags, Worktree, WorktreeAddOptions,
    WorktreeLockStatus, WorktreePruneOptions,
};

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::{GitSignature, ReflogEntry, StashInfo, WorktreeInfo};

/// Full name of the stash ref (its reflog doubles as the stash stack).
const STASH_REF: &str = "refs/stash";

/// Reflog file path for a full refname. `HEAD` is per-worktree; every other
/// ref's log lives in the common gitdir.
fn reflog_path(repo: &Repository, refname: &str) -> PathBuf {
    let base = if refname == "HEAD" {
        repo.path()
    } else {
        repo.commondir()
    };
    base.join("logs").join(refname)
}

/// Resolve a reflog request to a full refname. libgit2 reads reflogs by
/// literal path (`logs/<name>`), so a bare branch name never matches on its
/// own; resolve it the way revparse would before reading.
fn resolve_reflog_name(repo: &Repository, name: &str) -> EngineResult<String> {
    // Literal first: HEAD, ORIG_HEAD, refs/*, or any existing odd ref.
    if repo.find_reference(name).is_ok() {
        return Ok(name.to_owned());
    }
    for candidate in [
        format!("refs/heads/{name}"),
        format!("refs/remotes/{name}"),
        format!("refs/tags/{name}"),
    ] {
        if repo.find_reference(&candidate).is_ok() {
            return Ok(candidate);
        }
    }
    Err(EngineError::Invalid(format!("no reflog for `{name}`")))
}

/// git2's `Signature` borrows the repo, which conflicts with the `&mut`
/// stash calls; rebuild an unbound signature from the configured identity.
fn unbound_signature(repo: &mut Repository) -> EngineResult<git2::Signature<'static>> {
    let sig = repo.signature()?;
    let name = sig.name().unwrap_or_default().to_owned();
    let email = sig.email().unwrap_or_default().to_owned();
    Ok(git2::Signature::now(&name, &email)?)
}

/// Stash commit at `index` (0 = newest, `stash@{index}`), resolved from the
/// `refs/stash` reflog. Out-of-range indexes produce a stable `Invalid` error
/// before any worktree mutation happens.
fn stash_oid_at(repo: &Repository, index: u32) -> EngineResult<git2::Oid> {
    // libgit2 materializes an empty reflog file on read; check first so a
    // failed stash lookup leaves the repo untouched.
    if !reflog_path(repo, STASH_REF).exists() {
        return Err(EngineError::Invalid(format!(
            "no stash entry at index {index}"
        )));
    }
    let reflog = repo.reflog(STASH_REF)?;
    // Reflog iteration is newest-first (index 0 = stash@{0}).
    let newest_first: Vec<_> = reflog.iter().collect();
    newest_first
        .get(index as usize)
        .map(|entry| entry.id_new())
        .ok_or_else(|| EngineError::Invalid(format!("no stash entry at index {index}")))
}

/// Error mapper for the stash stack operations libgit2 performs by index.
fn no_such_stash(index: u32) -> impl Fn(git2::Error) -> EngineError {
    move |e: git2::Error| {
        if e.code() == ErrorCode::NotFound {
            EngineError::Invalid(format!("no stash entry at index {index}"))
        } else {
            EngineError::Git(e)
        }
    }
}

fn sig_to_model(sig: git2::Signature<'_>) -> GitSignature {
    GitSignature {
        name: sig.name().unwrap_or_default().to_owned(),
        email: sig.email().unwrap_or_default().to_owned(),
        time: sig.when().seconds(),
        offset_minutes: sig.when().offset_minutes(),
    }
}

// These are the D3 lane's `GitEngineM3` implementations. Rust only allows a
// single `impl GitEngineM3 for Libgit2Engine` block (currently owned by
// merge.rs), so until the integration pass wires the forwarding there, the
// methods below are only called from `stash_tests` and count as dead code in
// the non-test build.
#[allow(dead_code)]
impl Libgit2Engine {
    pub(crate) fn stash_list_impl(&self, repo: &Repository) -> EngineResult<Vec<StashInfo>> {
        // No reflog file yet == empty stash stack (checking first avoids
        // libgit2's create-empty-reflog-on-read side effect).
        if !reflog_path(repo, STASH_REF).exists() {
            return Ok(Vec::new());
        }
        let reflog = repo.reflog(STASH_REF)?;
        let mut out = Vec::new();
        // Reflog iteration is already newest-first (index 0 = stash@{0},
        // the order `git stash list` shows).
        for (index, entry) in reflog.iter().enumerate() {
            let oid = entry.id_new();
            // Author comes from the stash commit; fall back to the reflog
            // entry's committer if the object is unreachable.
            let author = match repo.find_commit(oid) {
                Ok(commit) => sig_to_model(commit.author()),
                Err(_) => sig_to_model(entry.committer()),
            };
            out.push(StashInfo {
                index: index as u32,
                sha: oid.to_string(),
                // Kept verbatim (e.g. "WIP on main: 1a2b3c4 subject").
                message: entry.message()?.unwrap_or_default().to_owned(),
                author,
            });
        }
        Ok(out)
    }

    pub(crate) fn stash_push_impl(
        &self,
        repo: &Repository,
        message: Option<&str>,
        keep_index: bool,
        include_untracked: bool,
    ) -> EngineResult<()> {
        let mut fresh = Repository::open(repo.path())?;
        let stasher = unbound_signature(&mut fresh)?;
        let mut flags = StashFlags::empty();
        if keep_index {
            flags |= StashFlags::KEEP_INDEX;
        }
        if include_untracked {
            flags |= StashFlags::INCLUDE_UNTRACKED;
        }
        fresh
            .stash_save2(&stasher, message, Some(flags))
            .map_err(|e| {
                if e.code() == ErrorCode::NotFound {
                    EngineError::Invalid("nothing to stash".into())
                } else {
                    EngineError::Git(e)
                }
            })?;
        Ok(())
    }

    pub(crate) fn stash_apply_impl(
        &self,
        repo: &Repository,
        index: u32,
        pop: bool,
    ) -> EngineResult<()> {
        // Pre-resolve so a bad index fails before the worktree is touched.
        stash_oid_at(repo, index)?;
        let mut fresh = Repository::open(repo.path())?;
        fresh
            .stash_apply(index as usize, None)
            .map_err(no_such_stash(index))?;
        // Pop only drops after a clean apply; conflicts leave the entry in
        // place (the FE surfaces the conflict state).
        if pop {
            fresh
                .stash_drop(index as usize)
                .map_err(no_such_stash(index))?;
        }
        Ok(())
    }

    pub(crate) fn stash_drop_impl(&self, repo: &Repository, index: u32) -> EngineResult<()> {
        stash_oid_at(repo, index)?;
        let mut fresh = Repository::open(repo.path())?;
        fresh.stash_drop(index as usize)?;
        Ok(())
    }

    /// `git stash branch <name> <stash>`: branch from the stash's base,
    /// check it out, apply the stash there, drop it on success. (git2 0.21
    /// has no `stash_branch` binding; implemented per git's documented
    /// behavior.)
    pub(crate) fn stash_branch_impl(
        &self,
        repo: &Repository,
        name: &str,
        index: u32,
    ) -> EngineResult<()> {
        let stash_oid = stash_oid_at(repo, index)?;
        let stash_commit = repo.find_commit(stash_oid)?;
        let base = stash_commit.parent(0).map_err(|_| {
            EngineError::Invalid(format!(
                "stash entry {index} has no base commit to branch from"
            ))
        })?;
        let base_oid = base.id();

        let mut fresh = Repository::open(repo.path())?;
        // Branch + checkout first (all read-only on the handle), then the
        // mutating stash calls which need `&mut`.
        {
            let base = fresh.find_commit(base_oid)?;
            let branch = fresh.branch(name, &base, false).map_err(|e| {
                if e.code() == ErrorCode::Exists {
                    EngineError::Invalid(format!("branch `{name}` already exists"))
                } else {
                    EngineError::Git(e)
                }
            })?;
            let refname = branch.get().name().unwrap_or_default().to_owned();
            let mut cb = CheckoutBuilder::new();
            cb.safe();
            fresh.checkout_tree(base.tree()?.as_object(), Some(&mut cb))?;
            fresh.set_head(&refname)?;
        }

        fresh
            .stash_apply(index as usize, None)
            .map_err(no_such_stash(index))?;
        fresh.stash_drop(index as usize)?;
        Ok(())
    }

    pub(crate) fn worktrees_impl(&self, repo: &Repository) -> EngineResult<Vec<WorktreeInfo>> {
        // M12: the main worktree leads the list; libgit2's `git_worktree_list`
        // only ever returns linked ones. HEAD comes from the main gitdir.
        let mut out = vec![main_worktree_info(repo)];
        for entry in repo.worktrees()?.iter() {
            let Some(name) = entry.ok().flatten() else {
                continue; // non-UTF-8 worktree name; skip rather than fail the listing
            };
            let Ok(wt) = repo.find_worktree(name) else {
                continue;
            };
            let locked = matches!(wt.is_locked()?, WorktreeLockStatus::Locked(_));
            let prunable = if wt.is_prunable(None)? {
                Some(prune_reason(&wt))
            } else {
                None
            };
            let (branch, head, detached) = worktree_head(repo, name);
            out.push(WorktreeInfo {
                path: wt.path().to_string_lossy().into_owned(),
                name: name.to_owned(),
                branch,
                head,
                detached,
                locked,
                prunable,
                is_main: false,
            });
        }
        // Linked entries alphabetical behind the (already first) main entry.
        out[1..].sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// `git worktree prune` — drop stale administrative files for worktrees
    /// whose directories vanished. Returns how many listed worktrees were
    /// prunable before the prune (the count the UI promised); the prune itself
    /// runs through the sanitized CLI (libgit2's `Worktree::prune` is
    /// per-worktree, `git worktree prune` sweeps everything at once).
    pub(crate) fn worktree_prune_impl(&self, repo: &Repository) -> EngineResult<u32> {
        let mut prunable = 0u32;
        for entry in repo.worktrees()?.iter() {
            let Some(name) = entry.ok().flatten() else {
                continue;
            };
            if let Ok(wt) = repo.find_worktree(name) {
                if wt.is_prunable(None).unwrap_or(false) {
                    prunable += 1;
                }
            }
        }
        let workdir = repo.workdir().unwrap_or_else(|| repo.path());
        crate::maintenance::git_run(workdir, &["worktree", "prune"])
            .map_err(EngineError::Invalid)?;
        Ok(prunable)
    }

    pub(crate) fn worktree_add_impl(
        &self,
        repo: &Repository,
        path: &str,
        branch: Option<&str>,
        new_branch: Option<&str>,
    ) -> EngineResult<()> {
        let path = Path::new(path);
        // Worktree name = last path component (what `git worktree add` shows).
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| {
                EngineError::Invalid(format!(
                    "cannot derive a worktree name from `{}`",
                    path.display()
                ))
            })?;

        // M3 scope: a worktree always starts on a branch — existing or fresh.
        // (libgit2 rejects an already-checked-out branch with
        // "reference ... is already checked out".)
        let reference = match (branch, new_branch) {
            (Some(_), Some(_)) => {
                return Err(EngineError::Invalid(
                    "pass either `branch` or `new_branch`, not both".into(),
                ));
            }
            (Some(name), None) => repo
                .find_branch(name, BranchType::Local)
                .map_err(|_| EngineError::Invalid(format!("branch `{name}` not found")))?
                .into_reference(),
            (None, Some(name)) => {
                let commit = repo
                    .head()
                    .map_err(|_| {
                        EngineError::Invalid(
                            "cannot create a worktree branch from an unborn HEAD".into(),
                        )
                    })?
                    .peel_to_commit()?;
                let branch = fresh_created_branch(repo, name, &commit)?;
                branch.into_reference()
            }
            (None, None) => {
                return Err(EngineError::Invalid(
                    "worktree_add requires `branch` or `new_branch` (detached adds are not supported)".into(),
                ));
            }
        };

        let mut opts = WorktreeAddOptions::new();
        opts.reference(Some(&reference));
        repo.worktree(name, path, Some(&opts))?;
        Ok(())
    }

    pub(crate) fn worktree_remove_impl(
        &self,
        repo: &Repository,
        name: &str,
        force: bool,
    ) -> EngineResult<()> {
        let wt = repo
            .find_worktree(name)
            .map_err(|_| EngineError::Invalid(format!("worktree `{name}` not found")))?;
        // Main-worktree guard: the linked-worktree list can never contain it,
        // and by-path comparison keeps a hypothetical "main" alias out too.
        if Some(wt.path()) == repo.workdir() {
            return Err(EngineError::Invalid(
                "the main worktree cannot be removed".into(),
            ));
        }
        // `git worktree remove` semantics: delete the admin area and the
        // working tree even though they are valid. Locked worktrees need
        // `force` (libgit2 otherwise errors with
        // "not pruning locked working tree: '<reason>'").
        let mut opts = WorktreePruneOptions::new();
        opts.valid(true).working_tree(true);
        if force {
            opts.locked(true);
        }
        wt.prune(Some(&mut opts))?;
        Ok(())
    }

    pub(crate) fn reflog_impl(
        &self,
        repo: &Repository,
        name: Option<&str>,
    ) -> EngineResult<Vec<ReflogEntry>> {
        let name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or("HEAD");
        let full = resolve_reflog_name(repo, name)?;
        // No log file yet: the ref exists but nothing was recorded on it.
        // (Pre-check also avoids libgit2's create-empty-reflog-on-read.)
        if !reflog_path(repo, &full).exists() {
            return Ok(Vec::new());
        }
        let reflog = repo
            .reflog(&full)
            .map_err(|e| EngineError::Invalid(format!("no reflog for `{name}`: {e}")))?;
        // Newest first (index 0 = most recent, the order `git reflog`
        // prints — libgit2 counts reflog indexes from the newest entry).
        let mut out = Vec::new();
        for entry in reflog.iter() {
            out.push(ReflogEntry {
                old_sha: entry.id_old().to_string(),
                new_sha: entry.id_new().to_string(),
                signature: sig_to_model(entry.committer()),
                message: entry.message()?.unwrap_or_default().to_owned(),
            });
        }
        Ok(out)
    }
}

/// Create a local branch at `commit`, mapping `Exists` to a friendly error.
fn fresh_created_branch<'r>(
    repo: &'r Repository,
    name: &str,
    commit: &git2::Commit<'_>,
) -> EngineResult<git2::Branch<'r>> {
    repo.branch(name, commit, false).map_err(|e| {
        if e.code() == ErrorCode::Exists {
            EngineError::Invalid(format!("branch `{name}` already exists"))
        } else {
            EngineError::Git(e)
        }
    })
}

/// The main worktree's listing entry: path/name from the repo workdir,
/// branch/head/detached from the main gitdir's `HEAD` file.
fn main_worktree_info(repo: &Repository) -> WorktreeInfo {
    let path = repo.workdir().unwrap_or_else(|| repo.path());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (branch, head, detached) = head_from_file(&repo.path().join("HEAD"), repo);
    WorktreeInfo {
        path: path.to_string_lossy().into_owned(),
        name,
        branch,
        head,
        detached,
        locked: false,
        prunable: None,
        is_main: true,
    }
}

/// (branch, head sha, detached) parsed from any `HEAD` file under the
/// common gitdir. A missing/unreadable HEAD is reported as a detached
/// with no target; the worktree will also show up as prunable.
fn head_from_file(head_file: &Path, repo: &Repository) -> (Option<String>, Option<String>, bool) {
    let Ok(content) = std::fs::read_to_string(head_file) else {
        return (None, None, true);
    };
    let content = content.trim();
    if let Some(target) = content.strip_prefix("ref: ") {
        let branch = target
            .strip_prefix("refs/heads/")
            .map(str::to_owned)
            .unwrap_or_else(|| target.to_owned());
        // Refs live in the common refdb, so the open repo can resolve them.
        let head = repo
            .find_reference(target)
            .ok()
            .and_then(|r| r.target())
            .map(|oid| oid.to_string());
        (Some(branch), head, false)
    } else {
        (None, Some(content.to_owned()), true)
    }
}

/// (branch, head sha, detached) from a linked worktree's `HEAD` file under
/// the common gitdir.
fn worktree_head(repo: &Repository, name: &str) -> (Option<String>, Option<String>, bool) {
    head_from_file(
        &repo.commondir().join("worktrees").join(name).join("HEAD"),
        repo,
    )
}

/// Human-readable prunable reason (libgit2 exposes only a boolean).
fn prune_reason(wt: &Worktree) -> String {
    if !wt.path().exists() {
        "worktree directory is missing".to_owned()
    } else if wt.validate().is_err() {
        "worktree metadata is invalid".to_owned()
    } else {
        "worktree can be pruned".to_owned()
    }
}

#[cfg(test)]
mod tests {
    //! M12 additions: main-worktree listing + `worktree prune`. (The older
    //! stash/worktree coverage lives in stash_tests.rs.)

    use std::path::{Path, PathBuf};

    use git2::{IndexAddOption, Repository};

    use super::super::libgit2::Libgit2Engine;

    const ENGINE: Libgit2Engine = Libgit2Engine;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("mygitui-wtmain-{}-{name}", std::process::id()));
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
            let dir = self.0.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let _ = std::fs::remove_dir_all(dir);
            });
        }
    }

    fn init_repo(path: &Path) -> Repository {
        // Pin the initial branch: init.defaultBranch varies by environment.
        let mut opts = git2::RepositoryInitOptions::new();
        opts.initial_head("main");
        let repo = Repository::init_opts(path, &opts).expect("init temp repo");
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Wt Test")?;
                c.set_str("user.email", "wt@test.local")
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

    #[test]
    fn worktree_list_leads_with_main_entry() {
        let dir = TempDir::new("main-first");
        let repo = init_repo(dir.path());
        let tip = commit_file(&repo, "a.txt", "one\n", "base");

        // A linked worktree sorts alphabetically BEFORE the repo dir name —
        // the main entry must still come first.
        let linked_path = dir.path().join("aaa-linked");
        ENGINE
            .worktree_add_impl(&repo, linked_path.to_str().unwrap(), None, Some("linked"))
            .expect("worktree add");

        let wts = ENGINE.worktrees_impl(&repo).expect("worktrees");
        assert_eq!(wts.len(), 2, "main + linked: {wts:?}");

        let main = &wts[0];
        assert!(main.is_main, "first entry is the main worktree");
        assert_eq!(
            Path::new(&main.path),
            repo.workdir().unwrap(),
            "main entry path is the repo workdir"
        );
        assert_eq!(main.branch.as_deref(), Some("main"));
        assert_eq!(main.head.as_deref(), Some(tip.as_str()));
        assert!(!main.detached);
        assert!(!main.locked);
        assert_eq!(main.prunable, None);

        let linked = wts
            .iter()
            .find(|w| w.name == "aaa-linked")
            .expect("linked entry");
        assert!(!linked.is_main, "linked entries are not main");
        assert_eq!(linked.branch.as_deref(), Some("linked"));
    }

    #[test]
    fn worktree_prune_on_healthy_repo_returns_zero() {
        let dir = TempDir::new("prune-healthy");
        let repo = init_repo(dir.path());
        commit_file(&repo, "a.txt", "one\n", "base");

        let pruned = ENGINE.worktree_prune_impl(&repo).expect("prune");
        assert_eq!(pruned, 0, "nothing prunable in a healthy repo");

        // With a linked worktree present and valid: still zero.
        let linked_path = dir.path().join("linked");
        ENGINE
            .worktree_add_impl(&repo, linked_path.to_str().unwrap(), None, Some("linked"))
            .expect("worktree add");
        let pruned = ENGINE.worktree_prune_impl(&repo).expect("prune");
        assert_eq!(pruned, 0);
        assert!(
            ENGINE.worktrees_impl(&repo).unwrap().len() == 2,
            "valid worktree survives the prune"
        );
    }

    #[test]
    fn worktree_prune_counts_and_sweeps_stale_worktrees() {
        let dir = TempDir::new("prune-stale");
        let repo = init_repo(dir.path());
        commit_file(&repo, "a.txt", "one\n", "base");
        let linked_path = dir.path().join("linked");
        ENGINE
            .worktree_add_impl(&repo, linked_path.to_str().unwrap(), None, Some("linked"))
            .expect("worktree add");

        // Simulate a worktree whose directory vanished (git's classic prune
        // case): the admin area remains but the tree is gone.
        std::fs::remove_dir_all(&linked_path).expect("remove worktree dir");

        let pruned = ENGINE.worktree_prune_impl(&repo).expect("prune");
        assert_eq!(pruned, 1, "the stale worktree was prunable");
        assert!(
            repo.find_worktree("linked").is_err(),
            "git worktree prune removed the admin area"
        );
    }
}
