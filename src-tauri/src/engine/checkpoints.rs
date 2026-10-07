//! Checkpoints — the undo system (lane D4): snapshot refs + restore + gc,
//! plus pure previews of dangerous operations.
//!
//! A checkpoint is a snapshot commit whose tree is the FULL WORKDIR STATE
//! (HEAD tree overlaid with every workdir modification, staged change and
//! untracked file — `git add -A` semantics) committed with the current HEAD
//! as first parent (no parent on an unborn HEAD). It is reachable only from
//! a hidden ref `refs/mygitui/checkpoints/<unix_millis>-<reason>`; HEAD and
//! all branch refs stay put, so creating one is invisible to normal git
//! operations and `git gc` only removes it once the ref is gone.
//!
//! Restore semantics (exact-snapshot restore):
//! 1. SAFETY: a `pre-restore` checkpoint is created first, so the restore
//!    itself is undoable.
//! 2. The index is mixed-reset to the snapshot's first parent (HEAD at
//!    snapshot time; cleared entirely for a root snapshot on an unborn HEAD).
//! 3. The snapshot tree is force-checked out over the workdir with
//!    `remove_untracked`, so the workdir matches the snapshot EXACTLY —
//!    files created after the checkpoint are deleted. This is documented
//!    behavior: restore is the big red undo button, not a merge.
//! 4. HEAD, branch refs and the reflog are untouched — restore rewrites only
//!    the index and workdir.
//!
//! GC deletes snapshot refs older than the cutoff but always keeps the most
//! recent checkpoint (safety floor), whatever its age.
//!
//! WIRING NOTE (M2-lane convention): `Libgit2Engine` may carry only one
//! `impl GitEngineM3` (Rust coherence) and lane D1 owns it, so the four
//! checkpoint entry points and the preview factor live here as inherent
//! `*_impl` methods. Integration forwards `GitEngineM3::checkpoint_*` to
//! these; the IPC commands call them directly in the meantime.

use std::time::{SystemTime, UNIX_EPOCH};

use git2::build::CheckoutBuilder;
use git2::{Delta, ErrorCode, IndexAddOption, Repository, Signature, Sort, StatusOptions};

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::{CheckpointInfo, PreviewFile, PreviewInfo};

/// Namespace holding all snapshot refs.
const CHECKPOINT_PREFIX: &str = "refs/mygitui/checkpoints/";

/// Marker line in a snapshot commit message (identifies our commits).
const MESSAGE_MARKER: &str = "mygitui-checkpoint";

/// Upper bound for the sanitized reason embedded in a ref name.
const REASON_MAX_CHARS: usize = 48;

/// Fallback suffix when a reason sanitizes to nothing (e.g. "///").
const REASON_FALLBACK: &str = "checkpoint";

/// Commits listed by the `branch_delete` preview before the cap kicks in.
const PREVIEW_COMMIT_CAP: usize = 100;

impl Libgit2Engine {
    /// Snapshot the full workdir state under a fresh checkpoint ref.
    ///
    /// The snapshot tree is built on a throwaway handle: `add_all` stages
    /// modifications + untracked files, `update_all` drops entries whose
    /// file vanished from the workdir. `write_tree` persists tree/blob
    /// objects only — the index (the user's staging area, on disk or cached
    /// in the shared handle) is never mutated, so taking a checkpoint leaves
    /// `git status` untouched.
    pub(crate) fn checkpoint_create_impl(
        &self,
        repo: &Repository,
        reason: &str,
    ) -> EngineResult<CheckpointInfo> {
        let tree_oid = workdir_snapshot_tree(repo)?;
        let tree = repo.find_tree(tree_oid)?;
        let committer = committer_signature(repo)?;
        let head = head_commit(repo)?;

        let branch = current_branch(repo);
        let branch_label = branch.clone().unwrap_or_else(|| "detached".to_owned());
        let message = format!(
            "checkpoint: {reason}\n\n{MESSAGE_MARKER}\nbranch: {branch_label}\nworktree: yes\n"
        );

        let parents: Vec<&git2::Commit<'_>> = head.iter().collect();
        let oid = repo.commit(None, &committer, &committer, &message, &tree, &parents)?;

        let ref_name = format!(
            "{CHECKPOINT_PREFIX}{}-{}",
            unix_millis(),
            sanitize_reason(reason)
        );
        // force=true: two checkpoints with the same millisecond + reason
        // collide on the ref name; the newer snapshot wins, which is fine.
        repo.reference(&ref_name, oid, true, "checkpoint: create")?;
        let commit = repo.find_commit(oid)?;
        Ok(checkpoint_info_of(&ref_name, &commit))
    }

    /// All checkpoints, newest first (by commit time; tie: ref name desc).
    pub(crate) fn checkpoints_impl(&self, repo: &Repository) -> EngineResult<Vec<CheckpointInfo>> {
        let mut out = Vec::new();
        for reference in repo.references_glob(&format!("{CHECKPOINT_PREFIX}*"))? {
            let reference = reference?;
            let Ok(name) = reference.name() else {
                continue; // non-UTF-8 ref name; skip rather than fail the listing
            };
            let Ok(commit) = reference.peel_to_commit() else {
                continue; // dangling/corrupt snapshot ref; skip
            };
            out.push(checkpoint_info_of(name, &commit));
        }
        out.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.ref_name.cmp(&a.ref_name))
        });
        Ok(out)
    }

    /// Restore the workdir + index to a checkpoint's exact snapshot (see the
    /// module docs for the full semantics). HEAD/branch refs never move.
    pub(crate) fn checkpoint_restore_impl(&self, repo: &Repository, id: &str) -> EngineResult<()> {
        let ref_name = format!("{CHECKPOINT_PREFIX}{id}");
        let reference = repo
            .find_reference(&ref_name)
            .map_err(|_| EngineError::Invalid(format!("checkpoint `{id}` not found")))?;

        // SAFETY FIRST: snapshot the about-to-be-destroyed state so a
        // restore has its own undo button.
        self.checkpoint_create_impl(repo, "pre-restore")?;

        let snapshot = reference.peel_to_commit()?;
        let snapshot_tree = snapshot.tree()?;

        // Index := HEAD-at-snapshot tree (mixed reset); a root snapshot has
        // no parent, so the index is emptied instead (unborn-style).
        match snapshot.parent(0) {
            Ok(parent1) => repo.reset_default(Some(parent1.as_object()), ["*"])?,
            Err(_) => {
                let mut index = repo.index()?;
                index.clear()?;
                index.write()?;
            }
        }

        // Workdir := snapshot, exactly. Force overwrites local
        // modifications; remove_untracked deletes files created after the
        // checkpoint (documented: restore is an exact-snapshot restore).
        // checkout_tree updates the index too (libgit2 default), landing it
        // on the snapshot tree.
        let mut checkout = CheckoutBuilder::new();
        checkout
            .force()
            .recreate_missing(true)
            .remove_untracked(true);
        repo.checkout_tree(snapshot_tree.as_object(), Some(&mut checkout))?;
        Ok(())
    }

    /// Delete checkpoint refs whose commit is older than the cutoff
    /// (`now - older_than_days`), returning how many were deleted. The most
    /// recent checkpoint is always kept (safety floor).
    pub(crate) fn checkpoint_gc_impl(
        &self,
        repo: &Repository,
        older_than_days: u32,
    ) -> EngineResult<u32> {
        let cutoff = unix_secs() - i64::from(older_than_days) * 86_400;

        // (ref name, commit time), newest first — same order as the listing.
        let mut entries: Vec<(String, i64)> = Vec::new();
        for reference in repo.references_glob(&format!("{CHECKPOINT_PREFIX}*"))? {
            let reference = reference?;
            let Ok(name) = reference.name() else {
                continue;
            };
            let Ok(commit) = reference.peel_to_commit() else {
                continue;
            };
            entries.push((name.to_owned(), commit.time().seconds()));
        }
        entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));

        let mut deleted = 0u32;
        for (index, (name, created_at)) in entries.iter().enumerate() {
            // Safety floor: the newest checkpoint survives every GC pass.
            if index == 0 || *created_at >= cutoff {
                continue;
            }
            repo.find_reference(name)?.delete()?;
            deleted += 1;
        }
        Ok(deleted)
    }

    /// Pure what-if preview of a dangerous operation: computes what WOULD
    /// change without touching the workdir, index, or any ref. Kinds:
    /// `reset_hard`, `clean`, `checkout_force`, `branch_delete`.
    pub(crate) fn preview_impl(
        &self,
        repo: &Repository,
        kind: &str,
        params: &serde_json::Value,
    ) -> EngineResult<PreviewInfo> {
        match kind {
            "reset_hard" => preview_reset_hard(repo, params),
            "clean" => preview_clean(repo, params),
            "checkout_force" => preview_checkout_force(repo, params),
            "branch_delete" => preview_branch_delete(repo, params),
            other => Err(EngineError::Invalid(format!(
                "unknown preview kind `{other}`"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// checkpoint helpers
// ---------------------------------------------------------------------------

/// Tree of the full workdir state: HEAD tree overlaid with all workdir
/// modifications + untracked files (`git add -A` semantics).
///
/// The index surgery runs on a FRESH repository handle: `add_all` /
/// `update_all` mutate the handle's cached in-memory index (they never touch
/// the on-disk index file — but the app's long-lived handle would then
/// report everything staged until reopened). The outer handle's index cache
/// revalidates from disk, which stays untouched (same pattern as the
/// `&mut Repository` dance in stash.rs).
fn workdir_snapshot_tree(repo: &Repository) -> EngineResult<git2::Oid> {
    let fresh = Repository::open(repo.path())?;
    let mut index = fresh.index()?;
    // add_all stages new + modified files (untracked included, ignored not).
    index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
    // update_all sweeps tracked entries whose file left the workdir.
    index.update_all(["*"].iter(), None)?;
    // Persists tree/blob objects to the odb; the index file stays as-is.
    Ok(index.write_tree()?)
}

/// Build the `CheckpointInfo` model by parsing a snapshot commit's message
/// trailers (`checkpoint: <reason>` subject + `branch: <name>` body line).
fn checkpoint_info_of(ref_name: &str, commit: &git2::Commit<'_>) -> CheckpointInfo {
    let message = commit.message().unwrap_or_default();
    let reason = message
        .strip_prefix("checkpoint: ")
        .and_then(|rest| rest.lines().next())
        .unwrap_or(REASON_FALLBACK)
        .to_owned();
    let branch = message_trailer(message, "branch: ")
        .filter(|branch| !branch.is_empty() && *branch != "detached")
        .map(str::to_owned);
    CheckpointInfo {
        id: ref_name
            .strip_prefix(CHECKPOINT_PREFIX)
            .unwrap_or(ref_name)
            .to_owned(),
        ref_name: ref_name.to_owned(),
        reason,
        created_at: commit.time().seconds(),
        branch,
        has_worktree_state: message.contains(MESSAGE_MARKER),
    }
}

/// First `key: value` line in a commit message body.
fn message_trailer<'m>(message: &'m str, key: &str) -> Option<&'m str> {
    message.lines().find_map(|line| line.strip_prefix(key))
}

/// Ref-safe suffix for a reason: lowercase, `[a-z0-9-]` only, runs of other
/// characters collapse to one `-`.
fn sanitize_reason(reason: &str) -> String {
    let mut out = String::new();
    for ch in reason.chars().take(REASON_MAX_CHARS * 2) {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        REASON_FALLBACK.to_owned()
    } else {
        trimmed.chars().take(REASON_MAX_CHARS).collect()
    }
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// HEAD peeled to a commit; `None` on an unborn HEAD.
fn head_commit<'r>(repo: &'r Repository) -> EngineResult<Option<git2::Commit<'r>>> {
    match repo.head() {
        Ok(head) => Ok(Some(head.peel_to_commit()?)),
        Err(e) if matches!(e.code(), ErrorCode::NotFound | ErrorCode::UnbornBranch) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Current branch name; still resolved on an unborn HEAD (its name lives in
/// HEAD's symbolic target). `None` when detached.
fn current_branch(repo: &Repository) -> Option<String> {
    match repo.head() {
        Ok(head) => head.shorthand().ok().map(str::to_owned),
        Err(e) if matches!(e.code(), ErrorCode::NotFound | ErrorCode::UnbornBranch) => {
            // Unborn HEAD: read the branch from HEAD's symbolic target.
            let target = repo
                .find_reference("HEAD")
                .ok()
                .and_then(|head| head.symbolic_target().ok().flatten().map(str::to_owned))?;
            Some(
                target
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&target)
                    .to_owned(),
            )
        }
        Err(_) => None,
    }
}

/// The configured identity, with the same friendly error as the M2 commit
/// path (checkpoints are commits and need user.name/user.email).
fn committer_signature(repo: &Repository) -> EngineResult<Signature<'static>> {
    repo.signature().map_err(|e| {
        EngineError::Invalid(format!(
            "committer identity not configured (set user.name / user.email): {e}"
        ))
    })
}

/// Resolve a commit-ish string to a commit (errors as `Invalid`).
fn resolve_commit<'r>(repo: &'r Repository, spec: &str) -> EngineResult<git2::Commit<'r>> {
    let obj = repo
        .revparse_single(spec)
        .map_err(|e| EngineError::Invalid(format!("cannot resolve `{spec}`: {e}")))?;
    obj.peel_to_commit()
        .map_err(|e| EngineError::Invalid(format!("`{spec}` is not a commit: {e}")))
}

// ---------------------------------------------------------------------------
// preview helpers
// ---------------------------------------------------------------------------

/// String parameter from the preview params object.
fn param_str<'a>(params: &'a serde_json::Value, key: &str) -> EngineResult<&'a str> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| EngineError::Invalid(format!("preview requires string param `{key}`")))
}

/// HEAD's tree; the empty tree on an unborn HEAD (everything is "added").
fn head_tree(repo: &Repository) -> EngineResult<git2::Tree<'_>> {
    match repo.head() {
        Ok(head) => Ok(head.peel_to_commit()?.tree()?),
        Err(e) if matches!(e.code(), ErrorCode::NotFound | ErrorCode::UnbornBranch) => {
            let empty = repo.treebuilder(None)?.write()?;
            Ok(repo.find_tree(empty)?)
        }
        Err(e) => Err(e.into()),
    }
}

/// Paths changed between two trees, with "added"/"deleted"/"modified".
fn tree_changed_files(
    repo: &Repository,
    old: &git2::Tree<'_>,
    new: &git2::Tree<'_>,
) -> EngineResult<Vec<PreviewFile>> {
    let diff = repo.diff_tree_to_tree(Some(old), Some(new), None)?;
    let mut files = Vec::new();
    for delta in diff.deltas() {
        let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) else {
            continue;
        };
        let change = match delta.status() {
            Delta::Added => "added",
            Delta::Deleted => "deleted",
            _ => "modified",
        };
        files.push(PreviewFile {
            path: path.to_string_lossy().into_owned(),
            change: change.to_owned(),
        });
    }
    Ok(files)
}

/// Number of dirty status entries (staged, unstaged, untracked, conflicted).
fn dirty_count(repo: &Repository) -> EngineResult<usize> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .exclude_submodules(true);
    Ok(repo.statuses(Some(&mut opts))?.len())
}

/// Shared body of the two tree-switch previews (`reset_hard`,
/// `checkout_force`): diff HEAD tree -> target tree + dirty count.
fn preview_tree_switch(
    repo: &Repository,
    kind: &str,
    summary_head: String,
    target: &git2::Commit<'_>,
) -> EngineResult<PreviewInfo> {
    let old = head_tree(repo)?;
    let files = tree_changed_files(repo, &old, &target.tree()?)?;
    let dirty = dirty_count(repo)?;
    Ok(PreviewInfo {
        kind: kind.to_owned(),
        summary: format!(
            "{summary_head}: {} files change, {dirty} uncommitted changes are discarded",
            files.len()
        ),
        files,
    })
}

fn preview_reset_hard(repo: &Repository, params: &serde_json::Value) -> EngineResult<PreviewInfo> {
    let to = param_str(params, "to")?;
    let target = resolve_commit(repo, to)?;
    let short = &target.id().to_string()[..7];
    preview_tree_switch(
        repo,
        "reset_hard",
        format!("Hard reset to {short}"),
        &target,
    )
}

fn preview_checkout_force(
    repo: &Repository,
    params: &serde_json::Value,
) -> EngineResult<PreviewInfo> {
    let name = param_str(params, "name")?;
    let target = resolve_commit(repo, name)?;
    let short = &target.id().to_string()[..7];
    preview_tree_switch(
        repo,
        "checkout_force",
        format!("Force checkout to {name} ({short})"),
        &target,
    )
}

fn preview_clean(repo: &Repository, params: &serde_json::Value) -> EngineResult<PreviewInfo> {
    // dirs=true recurses into untracked directories (git clean -d): every
    // file inside is listed; dirs=false collapses a directory to "dir/".
    let dirs = params
        .get("dirs")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(dirs);
    let statuses = repo.statuses(Some(&mut opts))?;
    let files: Vec<PreviewFile> = statuses
        .iter()
        .filter(|entry| entry.status().is_wt_new())
        .filter_map(|entry| {
            entry.path().ok().map(|path| PreviewFile {
                path: path.to_owned(),
                change: "deleted".to_owned(),
            })
        })
        .collect();
    Ok(PreviewInfo {
        kind: "clean".to_owned(),
        summary: format!("Clean removes {} untracked paths", files.len()),
        files,
    })
}

fn preview_branch_delete(
    repo: &Repository,
    params: &serde_json::Value,
) -> EngineResult<PreviewInfo> {
    let name = param_str(params, "name")?;
    let into = params
        .get("into")
        .and_then(|v| v.as_str())
        .unwrap_or("HEAD");
    let tip = resolve_commit(repo, name)?;
    let into_tip = resolve_commit(repo, into)?;

    // Fully merged = the branch tip is an ancestor of `into` (merge base
    // equals the tip); deleting it orphans nothing.
    let merged = repo
        .merge_base(tip.id(), into_tip.id())
        .map(|base| base == tip.id())
        .unwrap_or(false);
    if merged {
        return Ok(PreviewInfo {
            kind: "branch_delete".to_owned(),
            summary: format!("merged into {into}"),
            files: Vec::new(),
        });
    }

    // Unmerged: list the commits that would become unreachable, capped.
    let mut walk = repo.revwalk()?;
    walk.push(tip.id())?;
    walk.hide(into_tip.id())?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    let mut files = Vec::new();
    for oid in walk.take(PREVIEW_COMMIT_CAP) {
        let commit = repo.find_commit(oid?)?;
        files.push(PreviewFile {
            path: commit.id().to_string(),
            change: commit
                .summary()
                .ok()
                .flatten()
                .unwrap_or("(no summary)")
                .to_owned(),
        });
    }
    Ok(PreviewInfo {
        kind: "branch_delete".to_owned(),
        summary: format!("UNMERGED: {} commits would be unreachable", files.len()),
        files,
    })
}
