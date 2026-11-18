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
//! - `worktree_lock`/`worktree_unlock` (Phase 5) run through libgit2's
//!   `Worktree::lock/unlock` — the same lock state the listing reads back,
//!   so no CLI round-trip. git's refusal of the main worktree is mirrored
//!   up front ("the main worktree cannot be locked/unlocked"). The spec
//!   argument accepts a worktree NAME (the `worktree_remove` convention)
//!   or its PATH (what `git worktree lock` takes).
//! - `worktree_add(from_remote)` (Phase 5): `<remote>/<branch>` shorthand
//!   creates the local branch at the tracking tip with upstream configured
//!   (shared `create_branch_at_tracking` helper from branches.rs, the same
//!   logic `branch_checkout_remote` uses) and the worktree is added on it;
//!   `branch`/`new_branch` stay mutually exclusive with it. The branch name
//!   may itself contain `/` — only the FIRST slash splits the remote off.

use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{
    BranchType, ErrorCode, Repository, StashApplyOptions, StashFlags, StashSaveOptions,
    StatusOptions, Worktree, WorktreeAddOptions, WorktreeLockStatus, WorktreePruneOptions,
};

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::{diff_to_files, Libgit2Engine};
use super::types::{DiffSide, FileDiff, GitSignature, ReflogEntry, StashInfo, WorktreeInfo};

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

/// Error mapper for the stash save calls: libgit2 reports "no local
/// changes" as NotFound (matches `git stash` exiting "No local changes").
fn nothing_to_stash(e: git2::Error) -> EngineError {
    if e.code() == ErrorCode::NotFound {
        EngineError::Invalid("nothing to stash".into())
    } else {
        EngineError::Git(e)
    }
}

/// After a selective stash (`git stash push -- <paths>`): reset every
/// pathspec-matching path with local changes back to HEAD, exactly like the
/// git CLI does once the stash captured them. HEAD-tracked paths get their
/// index entry and workdir bytes restored from HEAD; staged-new paths leave
/// the index and the workdir (nothing at HEAD to restore); untracked paths
/// leave the workdir only when they were actually stashed
/// (`--include-untracked`).
fn reset_stashed_paths(
    repo: &Repository,
    pathspec: &git2::Pathspec,
    include_untracked: bool,
) -> EngineResult<()> {
    let mut status_opts = StatusOptions::new();
    status_opts
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut status_opts))?;

    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let head_tree = match &head {
        Some(commit) => Some(commit.tree()?),
        None => None,
    };

    for entry in statuses.iter().filter(|e| {
        let status = e.status();
        status.intersects(
            git2::Status::WT_NEW
                | git2::Status::WT_MODIFIED
                | git2::Status::WT_DELETED
                | git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED,
        )
    }) {
        let path = entry.path().unwrap_or_default().to_owned();
        let status = entry.status();
        if !pathspec.matches_path(Path::new(&path), git2::PathspecFlags::default()) {
            continue;
        }
        // Restore from HEAD when the path is tracked there; otherwise it
        // leaves index + workdir entirely (a fresh staged file was stashed
        // whole; an untracked one only when include_untracked swept it).
        let in_head = head_tree
            .as_ref()
            .map(|t| t.get_path(Path::new(&path)).is_ok())
            .unwrap_or(false);
        let untracked_only = status == git2::Status::WT_NEW;
        if untracked_only && !include_untracked {
            continue; // not part of the stash: keep it on disk
        }
        if in_head {
            let commit = head.as_ref().expect("head checked above");
            repo.reset_default(Some(commit.as_object()), [&path])?;
            let blob = repo.find_blob(commit.tree()?.get_path(Path::new(&path))?.id())?;
            let full = repo
                .workdir()
                .ok_or_else(|| EngineError::Invalid("bare repository has no workdir".into()))?
                .join(&path);
            // checkout-style smudge (autocrlf/eol attributes) — see eol.rs
            let content = super::eol::convert_to_workdir(repo, &path, blob.content());
            std::fs::write(full, content)?;
        } else {
            repo.reset_default(None, [&path])?;
            let full = repo
                .workdir()
                .ok_or_else(|| EngineError::Invalid("bare repository has no workdir".into()))?
                .join(&path);
            if let Err(e) = std::fs::remove_file(&full) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(e.into());
                }
            }
        }
    }
    Ok(())
}

/// Selective stash (`git stash push [-u] [-k] -- <paths>`), git-CLI
/// accurate. Three libgit2 divergences are worked around:
/// - `stash_save_with_opts` force-resets the WHOLE workdir/index after
///   building the stash, which would destroy non-matching edits — git
///   resets only the matching paths. The stash is therefore built with
///   `StashFlags::KEEP_ALL` (libgit2 touches nothing) and the matching
///   paths are reset to HEAD afterwards (`reset_stashed_paths`).
/// - its pathspec mode copies UNTRACKED matches into the stash tree too,
///   where their double presence (stash tree + untracked tree) makes a
///   later apply conflict double-writing the file. libgit2 therefore only
///   ever sees the TRACKED matching paths; untracked matches are swept into
///   the untracked parent by its (pathspec-blind) workdir sweep.
/// - that sweep would also capture every NON-matching untracked file, so
///   those are held aside in the gitdir for the duration of the call.
///
/// With no tracked matches at all, the stash is built directly
/// (`stash_untracked_only`) since libgit2 cannot express the shape.
fn selective_stash(
    fresh: &mut Repository,
    stasher: git2::Signature<'static>,
    message: Option<&str>,
    keep_index: bool,
    include_untracked: bool,
    pathspecs: &[String],
) -> EngineResult<()> {
    let pathspec = git2::Pathspec::new(pathspecs.iter().map(String::as_str))?;
    let (tracked, untracked) = selective_changes(fresh, &pathspec)?;
    if tracked.is_empty() && untracked.is_empty() {
        return Err(EngineError::Invalid("nothing to stash".into()));
    }
    let holding = if include_untracked {
        let holding_paths = non_matching_untracked(fresh, &pathspec)?;
        if holding_paths.is_empty() {
            None
        } else {
            let workdir = fresh
                .workdir()
                .ok_or_else(|| EngineError::Invalid("bare repository has no workdir".into()))?
                .to_path_buf();
            Some(UntrackedHolding::create(
                &workdir,
                fresh.path(),
                holding_paths,
            )?)
        }
    } else {
        None
    };
    // Restore the held-aside files on every path below: a refused stash
    // ("nothing to stash") must never lose them.
    if tracked.is_empty() {
        let result = stash_untracked_only(fresh, &stasher, message, &untracked);
        if let Some(holding) = holding {
            holding.restore();
        }
        result?;
        reset_stashed_paths(fresh, &pathspec, include_untracked)?;
        return Ok(());
    }
    let mut flags = StashFlags::KEEP_ALL;
    if keep_index {
        flags |= StashFlags::KEEP_INDEX;
    }
    let mut opts = StashSaveOptions::new(stasher);
    opts.flags(Some(flags));
    for path in &tracked {
        opts.pathspec(path.as_str());
    }
    let stash = fresh.stash_save_ext(Some(&mut opts));
    if let Some(holding) = holding {
        holding.restore();
    }
    stash.map_err(nothing_to_stash)?;
    if !keep_index {
        reset_stashed_paths(fresh, &pathspec, include_untracked)?;
    }
    Ok(())
}

/// Split the pathspec-matching local changes into tracked ones (index or
/// worktree state of HEAD-tracked / staged paths) and untracked ones
/// (`WT_NEW` only — never part of the stash tree).
fn selective_changes(
    repo: &Repository,
    pathspec: &git2::Pathspec,
) -> EngineResult<(Vec<String>, Vec<String>)> {
    let mut status_opts = StatusOptions::new();
    status_opts
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .exclude_submodules(true);
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    for entry in repo.statuses(Some(&mut status_opts))?.iter() {
        let status = entry.status();
        if !status.intersects(
            git2::Status::WT_NEW
                | git2::Status::WT_MODIFIED
                | git2::Status::WT_DELETED
                | git2::Status::WT_RENAMED
                | git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED
                | git2::Status::INDEX_RENAMED,
        ) {
            continue;
        }
        let Some(path) = entry.path().ok().map(str::to_owned) else {
            continue;
        };
        if !pathspec.matches_path(Path::new(&path), git2::PathspecFlags::default()) {
            continue;
        }
        if status == git2::Status::WT_NEW {
            untracked.push(path);
        } else {
            tracked.push(path);
        }
    }
    Ok((tracked, untracked))
}

/// Build the stash for `git stash push -u -- <paths>` when ONLY untracked
/// files match: the stash tree equals the base tree (no tracked changes)
/// and the files live only in the untracked parent — the shape git itself
/// produces, which libgit2's pathspec mode cannot express. Message formats
/// mirror libgit2's (`On <branch>: <msg>` / `WIP on <branch>: <sha> <subj>`).
fn stash_untracked_only(
    repo: &Repository,
    stasher: &git2::Signature<'static>,
    message: Option<&str>,
    untracked_paths: &[String],
) -> EngineResult<()> {
    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let branch = repo
        .find_reference("HEAD")
        .ok()
        .and_then(|h| h.symbolic_target().ok().flatten().map(str::to_owned))
        .map(|t| t.trim_start_matches("refs/heads/").to_owned())
        .unwrap_or_else(|| "(no branch)".to_owned());
    let desc = match &head {
        Some(commit) => format!(
            "{branch}: {:.7} {}",
            commit.id(),
            commit.summary().ok().flatten().unwrap_or_default()
        ),
        None => format!("{branch}:"),
    };

    // 1. index snapshot (the `--index` restore source).
    let index_tree_oid = repo.index()?.write_tree()?;
    let index_tree = repo.find_tree(index_tree_oid)?;
    let index_parents: Vec<git2::Commit<'_>> = head.iter().cloned().collect();
    let index_parent_refs: Vec<&git2::Commit<'_>> = index_parents.iter().collect();
    let index_oid = repo.commit(
        None,
        stasher,
        stasher,
        &format!("index on {desc}"),
        &index_tree,
        &index_parent_refs,
    )?;
    let index_commit = repo.find_commit(index_oid)?;

    // 2. untracked parent with exactly the matching files.
    let untracked_tree_oid = write_files_tree(repo, untracked_paths)?;
    let untracked_tree = repo.find_tree(untracked_tree_oid)?;
    let untracked_parents = [&index_commit];
    let untracked_oid = repo.commit(
        None,
        stasher,
        stasher,
        &format!("untracked files on {desc}"),
        &untracked_tree,
        &untracked_parents,
    )?;
    let untracked_commit = repo.find_commit(untracked_oid)?;

    // 3. stash commit: base tree, stash message, parents in libgit2/git
    // order [base, index, untracked] (stash_apply reads base from parent 0
    // and the index parent from 1).
    let base_tree = match &head {
        Some(commit) => commit.tree()?,
        None => repo.find_tree(repo.treebuilder(None)?.write()?)?,
    };
    let mut parents: Vec<&git2::Commit<'_>> = head.iter().collect();
    parents.push(&index_commit);
    parents.push(&untracked_commit);
    let worktree_message = match message {
        Some(msg) => format!("On {branch}: {msg}"),
        None => format!("WIP on {desc}"),
    };
    let stash_oid = repo.commit(
        None,
        stasher,
        stasher,
        &worktree_message,
        &base_tree,
        &parents,
    )?;

    // 4. move refs/stash (creating its reflog first — refs/stash is not in
    // libgit2's default reflog globs, so a first stash needs the explicit
    // ensure-log or the entry would be lost).
    repo.reference_ensure_log(STASH_REF)?;
    repo.reference(STASH_REF, stash_oid, true, &worktree_message)?;
    Ok(())
}

/// Build a tree containing exactly `paths` (blobs read from the workdir),
/// with nested subtrees for directories.
fn write_files_tree(repo: &Repository, paths: &[String]) -> EngineResult<git2::Oid> {
    use std::collections::BTreeMap;

    fn build(repo: &Repository, entries: &BTreeMap<String, Node>) -> EngineResult<git2::Oid> {
        let mut builder = repo.treebuilder(None)?;
        for (name, node) in entries {
            match node {
                Node::File(oid) => {
                    builder.insert(name, *oid, 0o100644)?;
                }
                Node::Dir(sub) => {
                    let oid = build(repo, sub)?;
                    builder.insert(name, oid, 0o040000)?;
                }
            }
        }
        Ok(builder.write()?)
    }

    enum Node {
        File(git2::Oid),
        Dir(BTreeMap<String, Node>),
    }

    fn dir_map(node: &mut Node) -> &mut BTreeMap<String, Node> {
        match node {
            Node::Dir(map) => map,
            Node::File(_) => unreachable!("insert descends only through Dir nodes"),
        }
    }

    let mut root = BTreeMap::new();
    let workdir = repo
        .workdir()
        .ok_or_else(|| EngineError::Invalid("bare repository has no workdir".into()))?;
    for rel in paths {
        let bytes = std::fs::read(workdir.join(rel))?;
        let oid = repo.blob(&bytes)?;
        let mut map = &mut root;
        let parts: Vec<&str> = rel.split('/').collect();
        for segment in &parts[..parts.len() - 1] {
            let node = map
                .entry((*segment).to_owned())
                .or_insert_with(|| Node::Dir(BTreeMap::new()));
            map = dir_map(node);
        }
        map.insert(parts[parts.len() - 1].to_owned(), Node::File(oid));
    }
    build(repo, &root)
}

/// Untracked files (status `WT_NEW`, recursive) that the pathspec does not
/// match — the files a selective stash must NOT capture.
fn non_matching_untracked(
    repo: &Repository,
    pathspec: &git2::Pathspec,
) -> EngineResult<Vec<String>> {
    let mut status_opts = StatusOptions::new();
    status_opts
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .exclude_submodules(true);
    Ok(repo
        .statuses(Some(&mut status_opts))?
        .iter()
        .filter(|e| e.status() == git2::Status::WT_NEW)
        .filter_map(|e| e.path().ok().map(str::to_owned))
        .filter(|p| !pathspec.matches_path(Path::new(p), git2::PathspecFlags::default()))
        .collect())
}

/// Holding area for non-matching untracked files during a selective stash
/// push: libgit2's untracked-commit sweep ignores the pathspec, so the files
/// move into `<gitdir>/mygitui/stash-keep-<...>/` for the duration of the
/// call and move right back. `rename` inside the same gitdir filesystem
/// never copies bytes, so even large untracked trees cost nothing; on the
/// rare cross-device gitdir the rename falls back to copy + remove.
struct UntrackedHolding {
    dir: PathBuf,
    workdir: PathBuf,
    moved: Vec<String>,
}

impl UntrackedHolding {
    fn create(workdir: &Path, gitdir: &Path, paths: Vec<String>) -> EngineResult<UntrackedHolding> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = gitdir
            .join("mygitui")
            .join(format!("stash-keep-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        for rel in &paths {
            let from = workdir.join(rel);
            let to = dir.join(rel);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            move_file(&from, &to)?;
        }
        Ok(UntrackedHolding {
            dir,
            workdir: workdir.to_path_buf(),
            moved: paths,
        })
    }

    /// Best-effort restore (runs on success AND after a failed stash — the
    /// workdir must never lose files to a refused stash).
    fn restore(self) {
        for rel in &self.moved {
            let to = self.workdir.join(rel);
            if let Some(parent) = to.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = move_file(&self.dir.join(rel), &to);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `rename`, falling back to copy + remove when source and target sit on
/// different filesystems.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
        Err(e) => Err(e),
    }
}

/// libgit2's selective `--include-untracked` stash puts the swept untracked
/// files into BOTH the stash tree (`stash_update_index_from_paths` adds
/// every pathspec match) and the untracked tree. Applying such a stash
/// double-writes the file: the untracked checkout creates it, then the safe
/// modified checkout conflicts on the now-existing file ("1 conflict
/// prevents checkout"). Drop the duplicated workdir copies first — the
/// apply's own untracked checkout recreates them — but only when the
/// workdir bytes still match the stashed blob, so a genuinely edited file
/// keeps its right to surface as a conflict.
fn drop_duplicated_untracked(repo: &Repository, stash_oid: git2::Oid) -> EngineResult<()> {
    let Ok(commit) = repo.find_commit(stash_oid) else {
        return Ok(());
    };
    if commit.parent_count() != 3 {
        return Ok(()); // no untracked parent: nothing to deduplicate
    }
    let Some(workdir) = repo.workdir() else {
        return Ok(());
    };
    let stash_tree = commit.tree()?;
    let mut dropped: Vec<(String, git2::Oid)> = Vec::new();
    tree_files(
        repo,
        &commit.parent(2)?.tree()?,
        String::new(),
        &mut dropped,
    );
    for (path, blob_oid) in dropped {
        if stash_tree.get_path(Path::new(&path)).is_err() {
            continue; // only duplicated entries are the double-write hazard
        }
        let Ok(blob) = repo.find_blob(blob_oid) else {
            continue;
        };
        let file = workdir.join(&path);
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        if bytes == blob.content() {
            std::fs::remove_file(&file)?;
        }
    }
    Ok(())
}

/// Collect the blob entries of a tree into `(path, oid)` pairs, recursing
/// into subtrees.
fn tree_files(
    repo: &Repository,
    tree: &git2::Tree<'_>,
    prefix: String,
    out: &mut Vec<(String, git2::Oid)>,
) {
    for entry in tree.iter() {
        let Ok(name) = entry.name() else { continue };
        match entry.kind() {
            Some(git2::ObjectType::Blob) => out.push((format!("{prefix}{name}"), entry.id())),
            Some(git2::ObjectType::Tree) => {
                if let Ok(sub) = repo.find_tree(entry.id()) {
                    tree_files(repo, &sub, format!("{prefix}{name}/"), out);
                }
            }
            _ => {}
        }
    }
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

    /// `git stash push [-m msg] [--keep-index] [--include-untracked] [-- paths]`.
    /// `paths` (`None` or empty = all changes) limits the stash to those
    /// pathspecs; the untracked / keep-index flags behave identically with
    /// and without a pathspec.
    pub(crate) fn stash_push_impl(
        &self,
        repo: &Repository,
        message: Option<&str>,
        keep_index: bool,
        include_untracked: bool,
        paths: Option<&[String]>,
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
        let pathspecs = paths.filter(|p| !p.is_empty());
        if let Some(pathspecs) = pathspecs {
            selective_stash(
                &mut fresh,
                stasher,
                message,
                keep_index,
                include_untracked,
                pathspecs,
            )
        } else {
            fresh
                .stash_save2(&stasher, message, Some(flags))
                .map_err(nothing_to_stash)?;
            Ok(())
        }
    }

    /// `git stash show <stash>`: the file diffs between the stash's base
    /// (first parent) and the stash commit, reusing the engine's diff
    /// pipeline (rename detection, binary/image marking, patch building).
    /// Like `git stash show`, the (optional) third parent's untracked-tree
    /// state is NOT included.
    pub(crate) fn stash_show_impl(
        &self,
        repo: &Repository,
        index: u32,
    ) -> EngineResult<Vec<FileDiff>> {
        let stash = stash_oid_at(repo, index)?;
        let commit = repo.find_commit(stash)?;
        let base = commit
            .parent(0)
            .map_err(|_| EngineError::Invalid(format!("stash entry {index} has no base commit")))?;
        let diff = self
            .diff_build(
                repo,
                &DiffSide::Commit(base.id().to_string()),
                &DiffSide::Commit(stash.to_string()),
                false,
            )?
            .ok_or_else(|| EngineError::Invalid("stash diff produced no sides".into()))?;
        diff_to_files(repo, &diff, None)
    }

    /// `git stash apply [--index] <stash>`: `restore_index` re-instates the
    /// stashed index state (staged changes come back staged, `--index`
    /// semantics) instead of staging everything as unstaged.
    pub(crate) fn stash_apply_impl(
        &self,
        repo: &Repository,
        index: u32,
        pop: bool,
        restore_index: bool,
    ) -> EngineResult<()> {
        // Pre-resolve so a bad index fails before the worktree is touched.
        let stash_oid = stash_oid_at(repo, index)?;
        let mut fresh = Repository::open(repo.path())?;
        drop_duplicated_untracked(&fresh, stash_oid)?;
        let mut opts = StashApplyOptions::new();
        if restore_index {
            opts.reinstantiate_index();
        }
        fresh
            .stash_apply(index as usize, Some(&mut opts))
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

    /// Rename the stash entry's message. git has no native rename, so the
    /// stash commit is re-created (same tree, parents, author) with the new
    /// message and `refs/stash` + its reflog move onto it. Only the newest
    /// entry (`stash@{0}`) is supported: renaming deeper entries would need a
    /// commit-chain/reflog rebuild of the whole stack.
    pub(crate) fn stash_rename_impl(
        &self,
        repo: &Repository,
        index: u32,
        message: &str,
    ) -> EngineResult<()> {
        if index != 0 {
            return Err(EngineError::Invalid(
                "only the newest stash can be renamed (stash@{0})".into(),
            ));
        }
        if message.trim().is_empty() {
            return Err(EngineError::Invalid("stash message cannot be empty".into()));
        }
        let old_oid = stash_oid_at(repo, index)?;
        let commit = repo.find_commit(old_oid)?;
        let committer = repo.signature().map_err(|e| {
            EngineError::Invalid(format!(
                "committer identity not configured (set user.name / user.email): {e}"
            ))
        })?;
        let parents: Vec<git2::Commit<'_>> = commit.parents().collect();
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        let new_oid = repo.commit(
            None, // no ref move here; refs/stash is updated with its own reflog entry below
            &commit.author(),
            &committer,
            message,
            &commit.tree()?,
            &parent_refs,
        )?;

        // Move the ref (force: it keeps pointing at the old commit). This
        // appends a reflog entry (old -> renamed) whose message is the NEW
        // stash message — `stash_list` renders the reflog message, so it
        // must read as the renamed entry. The superseded entry that still
        // points at the old commit is dropped below, keeping the stack
        // length unchanged.
        repo.reference(STASH_REF, new_oid, true, message)?;

        // Reflog fix-up. `reference` may or may not append an entry
        // (reflog-on-update is config dependent), so handle both shapes.
        let mut reflog = repo.reflog(STASH_REF)?;
        let newest = reflog.get(0).map(|e| e.id_new());
        if newest == Some(new_oid) {
            // Expected shape: [renamed, (old), ...]. Remove the stale entry
            // pointing at the old commit; `rewrite_previous_entry` splices
            // the chain (the renamed entry's old-id becomes the previous
            // stash, matching what the original push wrote).
            for i in 1..reflog.len() {
                if reflog.get(i).map(|e| e.id_new()) == Some(old_oid) {
                    reflog.remove(i, true)?;
                    break;
                }
            }
        } else {
            // No reflog entry was appended: rewrite the newest entry in
            // place via remove + append (its old-id re-reads the current
            // ref target, so it reports the rename, not the original base).
            reflog.remove(0, false)?;
            reflog.append(new_oid, &committer, Some(message))?;
        }
        reflog.write()?;
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
        from_remote: Option<&str>,
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

        // M3 scope: a worktree always starts on a branch — existing, fresh,
        // or (Phase 5) created at a remote-tracking tip. (libgit2 rejects an
        // already-checked-out branch with "reference ... is already checked
        // out".)
        let reference = match (branch, new_branch, from_remote) {
            (Some(_), Some(_), _) => {
                return Err(EngineError::Invalid(
                    "pass either `branch` or `new_branch`, not both".into(),
                ));
            }
            (Some(_), _, Some(_)) | (None, Some(_), Some(_)) => {
                return Err(EngineError::Invalid(
                    "`from_remote` cannot be combined with `branch` or `new_branch`".into(),
                ));
            }
            (Some(name), None, None) => repo
                .find_branch(name, BranchType::Local)
                .map_err(|_| EngineError::Invalid(format!("branch `{name}` not found")))?
                .into_reference(),
            (None, Some(name), None) => {
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
            (None, None, Some(spec)) => {
                // `<remote>/<branch>` shorthand: create the local branch at
                // the tracking tip (upstream configured, no checkout) via
                // the shared `branch_checkout_remote` helper, then put the
                // worktree on it. A branch name of its own may contain
                // `/`; only the FIRST slash splits the remote off.
                let (remote, name) = spec.split_once('/').ok_or_else(|| {
                    EngineError::Invalid(format!(
                        "`{spec}` is not a remote branch (`<remote>/<branch>` expected)"
                    ))
                })?;
                let local_name =
                    super::branches::create_branch_at_tracking(repo, remote, name, name)?;
                repo.find_branch(&local_name, BranchType::Local)?
                    .into_reference()
            }
            (None, None, None) => {
                return Err(EngineError::Invalid(
                    "worktree_add requires `branch`, `new_branch`, or `from_remote` (detached adds are not supported)".into(),
                ));
            }
        };

        let mut opts = WorktreeAddOptions::new();
        opts.reference(Some(&reference));
        repo.worktree(name, path, Some(&opts))?;
        Ok(())
    }

    /// `git worktree lock [--reason <r>] <path|name>` (Phase 5). The same
    /// libgit2 lock state the `worktrees` listing reads back
    /// (`WorktreeLockStatus::Locked(reason)`), so the FE lock flag and the
    /// lock reason are authoritative without a CLI round-trip. git refuses
    /// the main worktree; we mirror that up front (before the spec lookup,
    /// which only sees linked worktrees).
    pub(crate) fn worktree_lock_impl(
        &self,
        repo: &Repository,
        path: &str,
        reason: Option<&str>,
    ) -> EngineResult<()> {
        if is_main_worktree_path(repo, path) {
            return Err(EngineError::Invalid(
                "the main worktree cannot be locked".into(),
            ));
        }
        let wt = find_worktree_by_spec(repo, path)?;
        wt.lock(reason.map(str::trim).filter(|r| !r.is_empty()))?;
        Ok(())
    }

    /// `git worktree unlock <path|name>` (Phase 5). Same main-worktree
    /// refusal as git (`The main working tree cannot be locked or unlocked`).
    pub(crate) fn worktree_unlock_impl(&self, repo: &Repository, path: &str) -> EngineResult<()> {
        if is_main_worktree_path(repo, path) {
            return Err(EngineError::Invalid(
                "the main worktree cannot be unlocked".into(),
            ));
        }
        let wt = find_worktree_by_spec(repo, path)?;
        wt.unlock()?;
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
        //
        // Like git, a linked worktree with local changes is only removable
        // with `force`: pruning would silently discard edits that no ref
        // points at. Ignored files are not local changes (status excludes
        // them unless asked), which matches `git worktree remove`.
        if !force {
            if let Ok(linked) = Repository::open(wt.path()) {
                let mut statuses = StatusOptions::new();
                statuses
                    .include_untracked(true)
                    .recurse_untracked_dirs(true)
                    .exclude_submodules(true);
                if !linked.statuses(Some(&mut statuses))?.is_empty() {
                    return Err(EngineError::Invalid(format!(
                        "worktree `{name}` has local changes; commit or stash them, or remove it with force"
                    )));
                }
            }
        }
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

/// Resolve a worktree by NAME first (the `worktree_remove` convention),
/// then by PATH (`git worktree lock/unlock` take paths). Linked worktrees
/// only — the lock/unlock impls refuse the main worktree themselves.
fn is_main_worktree_path(repo: &Repository, spec: &str) -> bool {
    // PathBuf equality compares components, so a trailing separator
    // (`/tmp/repo/`) still matches the bare workdir path.
    repo.workdir()
        .map(|w| w == Path::new(spec))
        .unwrap_or(false)
}

/// Resolve a worktree by NAME first (the `worktree_remove` convention),
/// then by PATH (`git worktree lock/unlock` take paths). Linked worktrees
/// only — the lock/unlock impls refuse the main worktree themselves.
fn find_worktree_by_spec(repo: &Repository, spec: &str) -> EngineResult<Worktree> {
    if let Ok(wt) = repo.find_worktree(spec) {
        return Ok(wt);
    }
    for entry in repo.worktrees()?.iter() {
        let Some(name) = entry.ok().flatten() else {
            continue; // non-UTF-8 worktree name
        };
        if let Ok(wt) = repo.find_worktree(name) {
            if wt.path().to_string_lossy() == spec {
                return Ok(wt);
            }
        }
    }
    Err(EngineError::Invalid(format!("worktree `{spec}` not found")))
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

    static ENGINE: Libgit2Engine = Libgit2Engine::new();

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
            .worktree_add_impl(
                &repo,
                linked_path.to_str().unwrap(),
                None,
                Some("linked"),
                None,
            )
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
            .worktree_add_impl(
                &repo,
                linked_path.to_str().unwrap(),
                None,
                Some("linked"),
                None,
            )
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
            .worktree_add_impl(
                &repo,
                linked_path.to_str().unwrap(),
                None,
                Some("linked"),
                None,
            )
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
