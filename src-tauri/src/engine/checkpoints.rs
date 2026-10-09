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

use super::git_engine::{EngineError, EngineResult};
use super::libgit2::Libgit2Engine;
use super::types::{CheckpointInfo, PreviewFile, PreviewInfo};
use git2::build::CheckoutBuilder;
use git2::{Delta, ErrorCode, IndexAddOption, Repository, Signature, Sort, StatusOptions};

/// Namespace holding all snapshot refs.
const CHECKPOINT_PREFIX: &str = "refs/mygitui/checkpoints/";

/// Marker file (under `<gitdir>/mygitui/`) holding the unix millis of the
/// last automatic checkpoint GC.
const AUTO_GC_MARKER: &str = "last_ckpt_gc";

/// Auto-GC cadence (days between runs) and the age cutoff it passes to the
/// real GC.
const AUTO_GC_DAYS: u32 = 30;

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

/// Best-effort automatic GC at repo open (M12): when the marker file
/// `<gitdir>/mygitui/last_ckpt_gc` is missing or older than
/// [`AUTO_GC_DAYS`] AND at least one checkpoint ref exists, run the regular
/// GC (`checkpoint_gc_impl`, same 30-day cutoff) and write the marker.
/// Every error is swallowed — this must never block or fail a repo open.
/// Callers on the open path may run it on a spawned thread (see repo.rs).
pub(crate) fn maybe_auto_gc(repo: &Repository) {
    let result = auto_gc_inner(repo);
    if let Err(err) = result {
        tracing::debug!(
            error = %err,
            "checkpoint auto-gc skipped (best-effort)"
        );
    }
}

fn auto_gc_inner(repo: &Repository) -> EngineResult<()> {
    let marker = repo.path().join("mygitui").join(AUTO_GC_MARKER);
    if marker_is_fresh(&marker) {
        return Ok(());
    }
    // No checkpoints → nothing to collect (and no marker: the first real
    // checkpoint still gets a GC pass at the next open).
    let mut any = repo.references_glob(&format!("{CHECKPOINT_PREFIX}*"))?;
    if any.next().is_none() {
        return Ok(());
    }
    let engine = Libgit2Engine;
    engine.checkpoint_gc_impl(repo, AUTO_GC_DAYS)?;
    // Marker written only after a run that actually happened.
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&marker, unix_millis().to_string())?;
    Ok(())
}

/// True when the marker exists and is younger than [`AUTO_GC_DAYS`].
fn marker_is_fresh(marker: &std::path::Path) -> bool {
    let Ok(text) = std::fs::read_to_string(marker) else {
        return false;
    };
    let Ok(last) = text.trim().parse::<i64>() else {
        return false;
    };
    let now = unix_millis() as i64;
    now - last < i64::from(AUTO_GC_DAYS) * 86_400_000
}

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

#[cfg(test)]
mod tests {
    //! M12: automatic checkpoint GC at repo open.

    use std::path::{Path, PathBuf};

    use git2::{IndexAddOption, Repository, Signature, Time};

    use super::super::libgit2::Libgit2Engine;
    use super::maybe_auto_gc;
    use super::{AUTO_GC_MARKER, CHECKPOINT_PREFIX};

    const ENGINE: Libgit2Engine = Libgit2Engine;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("mygitui-ckptgc-{}-{name}", std::process::id()));
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
        let repo = Repository::init(path).expect("init temp repo");
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Gc Test")?;
                c.set_str("user.email", "gc@test.local")
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

    /// Forge a checkpoint whose commit is backdated `age_days`, with a ref
    /// name that sorts as older than anything created "now".
    fn forge_aged_checkpoint(
        repo: &Repository,
        parent_sha: &str,
        tag: &str,
        age_days: i64,
    ) -> String {
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
        let when = now_secs() - age_days * 86_400;
        let sig = Signature::new("Old Test", "old@test.local", &Time::new(when, 0))
            .expect("backdated signature");
        let message =
            format!("checkpoint: aged-{tag}\n\nmygitui-checkpoint\nbranch: main\nworktree: yes\n");
        let oid = repo
            .commit(None, &sig, &sig, &message, &tree, &[&parent])
            .expect("backdated commit");
        let ref_name = format!("{CHECKPOINT_PREFIX}{}-aged-{tag}", now_millis());
        repo.reference(&ref_name, oid, true, "checkpoint: create")
            .expect("forge ref");
        ref_name
    }

    fn marker_path(repo: &Repository) -> PathBuf {
        repo.path().join("mygitui").join(AUTO_GC_MARKER)
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

    #[test]
    fn auto_gc_prunes_aged_checkpoints_and_writes_marker() {
        let dir = TempDir::new("prunes");
        let repo = init_repo(dir.path());
        let base = commit_file(&repo, "a.txt", "one\n", "base");

        // One aged checkpoint (40 days) + one fresh (the safety floor).
        let aged = forge_aged_checkpoint(&repo, &base, "old", 40);
        let fresh = ENGINE
            .checkpoint_create_impl(&repo, "fresh")
            .expect("fresh checkpoint");

        // No marker (never gc'd): the call must collect the aged ref.
        assert!(!marker_path(&repo).exists());
        maybe_auto_gc(&repo);

        assert!(
            repo.find_reference(&aged).is_err(),
            "40-day-old checkpoint pruned"
        );
        assert!(
            repo.find_reference(&fresh.ref_name).is_ok(),
            "fresh checkpoint survives"
        );
        assert!(marker_path(&repo).exists(), "marker written after the run");
    }

    #[test]
    fn auto_gc_second_call_within_window_is_a_no_op() {
        let dir = TempDir::new("no-op");
        let repo = init_repo(dir.path());
        let base = commit_file(&repo, "a.txt", "one\n", "base");

        let aged = forge_aged_checkpoint(&repo, &base, "one", 40);
        ENGINE
            .checkpoint_create_impl(&repo, "fresh")
            .expect("fresh checkpoint");
        maybe_auto_gc(&repo);
        assert!(repo.find_reference(&aged).is_err());

        // A NEW aged checkpoint appears, but the marker is now fresh: the
        // next call must not collect anything.
        let aged2 = forge_aged_checkpoint(&repo, &base, "two", 40);
        maybe_auto_gc(&repo);
        assert!(
            repo.find_reference(&aged2).is_ok(),
            "fresh marker ⇒ gc skipped entirely"
        );

        // Deleting the marker re-arms the gc.
        std::fs::remove_file(marker_path(&repo)).expect("remove marker");
        maybe_auto_gc(&repo);
        assert!(repo.find_reference(&aged2).is_err(), "gc ran again");
    }

    #[test]
    fn auto_gc_without_checkpoints_writes_no_marker() {
        let dir = TempDir::new("empty");
        let repo = init_repo(dir.path());
        commit_file(&repo, "a.txt", "one\n", "base");
        maybe_auto_gc(&repo);
        assert!(
            !marker_path(&repo).exists(),
            "no checkpoints ⇒ no run ⇒ no marker"
        );
    }
}
