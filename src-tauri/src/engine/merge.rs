//! Merge, conflicts, cherry-pick, revert, reset — lane D1.
//!
//! SEMANTICS (the FE contract):
//!
//! * `merge_branch` — `UpToDate` (nothing to do), `FastForward` (safe checkout
//!   of the target tree + branch ref update; `no_ff` forces the real merge
//!   instead), `Merged` (2-parent commit, message `Merge branch '<name>'` or
//!   `Merge remote-tracking branch '<name>'` for refs under `refs/remotes/`),
//!   or `Conflicted` (libgit2's merge state stays: MERGE_HEAD + 3-stage index
//!   entries + conflict markers in the workdir; the FE drives resolution via
//!   `conflict_resolve`, then the user commits normally). The finishing
//!   commit (mutations.rs) takes MERGE_HEAD as its second parent and consumes
//!   the merge state.
//! * `conflict_resolve` — preset `Ours`/`Theirs` write that side's bytes to
//!   the workdir file and clear the index conflict (the path becomes staged);
//!   `Both` materializes a 2-way marker preview into the workdir and KEEPS
//!   the index conflicted:
//!
//!   ```text
//!   <<<<<<< <path>
//!   <stage-2 "ours" bytes>
//!   =======
//!   <stage-3 "theirs" bytes>
//!   >>>>>>> <path>
//!   ```
//!
//!   Sections are newline-terminated (a missing trailing newline is added so
//!   separator lines always start at column 0); the FE editor edits the file
//!   and finishes by calling again with `custom_content` set (editor bytes
//!   take precedence over the preset), which writes + stages + resolves.
//! * `cherry_pick`/`revert` — replay each commit's parent diff (forward /
//!   reversed) via libgit2's sequencer machinery onto workdir+index, then
//!   commit: cherry-pick keeps the original message + author (committer =
//!   configured identity); revert writes `Revert "<summary>"` plus a
//!   `This reverts commit <sha>.` body. A conflicting step reports success
//!   but leaves real git state behind: 3-stage index entries, conflict
//!   markers in the workdir and the `.git/CHERRY_PICK_HEAD` /
//!   `.git/REVERT_HEAD` plus `MERGE_MSG` files, so `RepoStatus::sequencer`
//!   and `conflicts().source` light up. Picks before the conflict are
//!   committed (reported in `new_head`); the remaining shas are NOT applied
//!   — the FE resolves + commits the conflict, then re-drives with the shas
//!   AFTER the conflicted one. A clean run drops libgit2's sequencer markers
//!   when it finishes. `sequencer_abort` cancels an interrupted pick/revert:
//!   workdir + index return to HEAD (already-committed picks of this leg
//!   stay) and the marker files are dropped.
//! * `reset` — `Soft` moves the branch/HEAD ref only; `Mixed` also resets the
//!   index to the target tree; `Hard` also checks the target tree out into
//!   the workdir (force). Works on unborn and detached HEAD.

use std::path::Path;

use git2::build::CheckoutBuilder;
use git2::{Index, Repository, RepositoryState, Signature, Time};

use super::git_engine::{EngineError, EngineResult, GitEngineM3};
use super::libgit2::Libgit2Engine;
use super::types::{
    BisectMark, BisectState, ConflictFile, ConflictResolution, MergeFavor, MergeOptions,
    MergeOutcome, MergeResult, ResetKind,
};
use crate::engine::types::{
    CheckpointInfo, RebaseState, RebaseStep, ReflogEntry, StashInfo, WorktreeInfo,
};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Resolve a commit-ish string to a commit (propagates errors as `Invalid`).
fn resolve_commit<'r>(repo: &'r Repository, spec: &str) -> EngineResult<git2::Commit<'r>> {
    let obj = repo
        .revparse_single(spec)
        .map_err(|e| EngineError::Invalid(format!("cannot resolve `{spec}`: {e}")))?;
    obj.peel_to_commit()
        .map_err(|e| EngineError::Invalid(format!("`{spec}` is not a commit: {e}")))
}

/// HEAD peeled to a commit; `None` on an unborn HEAD.
fn head_commit<'r>(repo: &'r Repository) -> EngineResult<Option<git2::Commit<'r>>> {
    match repo.head() {
        Ok(head) => Ok(Some(head.peel_to_commit()?)),
        Err(e)
            if matches!(
                e.code(),
                git2::ErrorCode::NotFound | git2::ErrorCode::UnbornBranch
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// Is a merge in progress (MERGE_HEAD written by `git merge` / `repo.merge`)?
pub(crate) fn merge_in_progress(repo: &Repository) -> bool {
    repo.path().join("MERGE_HEAD").exists() || repo.state() == RepositoryState::Merge
}

/// Which sequencer owns the current conflicts. Precedence mirrors the
/// `in_progress_state` helper in libgit2.rs: rebase beats cherry-pick beats
/// revert; anything else reads as a merge.
fn sequencer_source(repo: &Repository) -> String {
    let gitdir = repo.path();
    let state = repo.state();
    let rebasing = gitdir.join("rebase-merge").is_dir()
        || gitdir.join("rebase-apply").is_dir()
        // mygitui's custom rebase sequencer state (engine/rebase.rs).
        || gitdir.join("mygitui").join("rebase.json").exists()
        || matches!(
            state,
            RepositoryState::Rebase
                | RepositoryState::RebaseInteractive
                | RepositoryState::RebaseMerge
                | RepositoryState::ApplyMailbox
                | RepositoryState::ApplyMailboxOrRebase
        );
    if rebasing {
        return "rebase".into();
    }
    if gitdir.join("CHERRY_PICK_HEAD").exists()
        || matches!(
            state,
            RepositoryState::CherryPick | RepositoryState::CherryPickSequence
        )
    {
        return "cherry-pick".into();
    }
    if gitdir.join("REVERT_HEAD").exists()
        || matches!(
            state,
            RepositoryState::Revert | RepositoryState::RevertSequence
        )
    {
        return "revert".into();
    }
    "merge".into()
}

/// All conflicted paths in the index with their stage availability.
fn conflicts_impl(repo: &Repository) -> EngineResult<Vec<ConflictFile>> {
    let source = sequencer_source(repo);
    let index = repo.index()?;
    let mut out = Vec::new();
    for conflict in index.conflicts()? {
        let conflict = conflict?;
        let path_of = |e: &git2::IndexEntry| String::from_utf8_lossy(&e.path).into_owned();
        let path = conflict
            .ancestor
            .as_ref()
            .or(conflict.our.as_ref())
            .or(conflict.their.as_ref())
            .map(path_of)
            .unwrap_or_default();
        out.push(ConflictFile {
            path,
            has_base: conflict.ancestor.is_some(),
            has_ours: conflict.our.is_some(),
            has_theirs: conflict.their.is_some(),
            source: source.clone(),
        });
    }
    Ok(out)
}

/// Move the ref HEAD points at to `oid`: the branch ref when attached (also
/// creating it on an unborn branch), HEAD itself when detached.
fn move_head_ref(repo: &Repository, oid: git2::Oid, log_message: &str) -> EngineResult<()> {
    let head = repo.find_reference("HEAD")?;
    match head.symbolic_target().ok().flatten() {
        Some(branch_ref) => {
            repo.reference(branch_ref, oid, true, log_message)?;
        }
        None => repo.set_head_detached(oid)?,
    }
    Ok(())
}

/// The configured identity (`repo.signature`), with the same friendly error
/// as the M2 commit path.
fn committer_signature(repo: &Repository) -> EngineResult<Signature<'static>> {
    repo.signature().map_err(|e| {
        EngineError::Invalid(format!(
            "committer identity not configured (set user.name / user.email): {e}"
        ))
    })
}

/// The original author signature rebuilt as owned (cherry-pick keeps the
/// author line verbatim; the committer becomes the local identity).
fn preserved_author(commit: &git2::Commit<'_>) -> EngineResult<Signature<'static>> {
    let author = commit.author();
    Signature::new(
        author.name().unwrap_or_default(),
        author.email().unwrap_or_default(),
        &Time::new(author.when().seconds(), author.when().offset_minutes()),
    )
    .map_err(|e| EngineError::Invalid(format!("invalid author identity: {e}")))
}

/// `git merge`'s default subject: remote-tracking refs (resolved against
/// `refs/remotes/`, full or shorthand) get the remote-tracking wording.
fn merge_message(repo: &Repository, ref_name: &str) -> String {
    let is_remote = ref_name.starts_with("refs/remotes/")
        || repo
            .find_reference(&format!("refs/remotes/{ref_name}"))
            .is_ok();
    let short = ref_name.strip_prefix("refs/remotes/").unwrap_or(ref_name);
    if is_remote {
        format!("Merge remote-tracking branch '{short}'")
    } else {
        let short = ref_name.strip_prefix("refs/heads/").unwrap_or(ref_name);
        format!("Merge branch '{short}'")
    }
}

/// Blob content of one stage of a conflicted path.
fn stage_content(
    repo: &Repository,
    index: &Index,
    path: &str,
    stage: u16,
) -> EngineResult<Vec<u8>> {
    let entry = index
        .get_path(Path::new(path), i32::from(stage))
        .ok_or_else(|| EngineError::Invalid(format!("`{path}` has no stage {stage} entry")))?;
    Ok(repo.find_blob(entry.id)?.content().to_vec())
}

fn write_workdir_file(repo: &Repository, path: &str, content: &[u8]) -> EngineResult<()> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| EngineError::Invalid("bare repository has no workdir".into()))?;
    let file = workdir.join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(file, content)?;
    Ok(())
}

/// Append `content` to `out`, newline-terminated, so a following separator
/// line always starts at column 0.
fn push_newline_terminated(out: &mut Vec<u8>, content: &[u8]) {
    out.extend_from_slice(content);
    if !content.ends_with(b"\n") {
        out.push(b'\n');
    }
}

/// 2-way conflict marker preview for the 3-way editor (format in module docs).
fn render_conflict_markers(path: &str, ours: &[u8], theirs: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ours.len() + theirs.len() + 2 * path.len() + 32);
    out.extend_from_slice(format!("<<<<<<< {path}\n").as_bytes());
    push_newline_terminated(&mut out, ours);
    out.extend_from_slice(b"=======\n");
    push_newline_terminated(&mut out, theirs);
    out.extend_from_slice(format!(">>>>>>> {path}\n").as_bytes());
    out
}

/// Best-effort removal of our sequencer marker files. Only safe when no
/// merge is running (an active merge owns MERGE_MSG).
pub(crate) fn clear_sequencer_files(repo: &Repository) {
    if merge_in_progress(repo) {
        return;
    }
    for name in ["CHERRY_PICK_HEAD", "REVERT_HEAD", "MERGE_MSG"] {
        let _ = std::fs::remove_file(repo.path().join(name));
    }
}

/// True while a cherry-pick or revert is interrupted (marker files or the
/// matching libgit2 repository state) and `sequencer_abort` may run.
fn sequencer_in_progress(repo: &Repository) -> bool {
    let gitdir = repo.path();
    matches!(
        repo.state(),
        RepositoryState::CherryPick
            | RepositoryState::CherryPickSequence
            | RepositoryState::Revert
            | RepositoryState::RevertSequence
    ) || gitdir.join("CHERRY_PICK_HEAD").exists()
        || gitdir.join("REVERT_HEAD").exists()
}

/// Undo the workdir side of an interrupted operation: every path it staged
/// (auto-merged or conflicted) goes back to its HEAD content, files it added
/// are removed — `git merge --abort` semantics for exactly the files the
/// operation touched. Unrelated local changes are kept. Shared by
/// `merge_abort` and `sequencer_abort`.
fn restore_workdir_to_head(repo: &Repository, head: &git2::Commit<'_>) -> EngineResult<()> {
    let head_tree = head.tree()?;
    let index = repo.index()?;
    let diff = repo.diff_tree_to_index(Some(&head_tree), Some(&index), None)?;
    let mut touched: Vec<std::path::PathBuf> = Vec::new();
    for delta in diff.deltas() {
        if let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) {
            touched.push(path.to_path_buf());
        }
    }
    drop(diff);
    drop(index);
    for path in &touched {
        match head_tree.get_path(path) {
            Ok(entry) => {
                let content = repo.find_blob(entry.id())?.content().to_vec();
                write_workdir_file(repo, path.to_string_lossy().as_ref(), &content)?;
            }
            // Added by the operation, absent from HEAD: drop the workdir file.
            Err(_) => {
                if let Some(workdir) = repo.workdir() {
                    let _ = std::fs::remove_file(workdir.join(path));
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// sequencer (cherry-pick / revert)
// ---------------------------------------------------------------------------

/// The two sequencer operations share apply/commit/conflict machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sequencer {
    CherryPick,
    Revert,
}

fn verb(kind: Sequencer) -> &'static str {
    match kind {
        Sequencer::CherryPick => "cherry-pick",
        Sequencer::Revert => "revert",
    }
}

/// Commit message for a revert (git format).
fn revert_message(sha: &str, commit: &git2::Commit<'_>) -> String {
    let fallback = || {
        commit
            .message()
            .unwrap_or("commit")
            .lines()
            .next()
            .unwrap_or("commit")
            .to_owned()
    };
    let summary = commit
        .summary()
        .ok()
        .flatten()
        .map(str::to_owned)
        .unwrap_or_else(fallback);
    format!("Revert \"{summary}\"\n\nThis reverts commit {sha}.\n")
}

impl Libgit2Engine {
    /// Abort an interrupted cherry-pick/revert: the workdir and index go back
    /// to HEAD (the state before the conflicted step — picks that already
    /// committed in this leg stay), then the marker files
    /// (CHERRY_PICK_HEAD / REVERT_HEAD / MERGE_MSG) are removed.
    /// `git cherry-pick --abort` also rewinds picks that already landed; that
    /// needs the CLI sequencer's persisted pre-run HEAD, which our apply loop
    /// doesn't keep — each FE re-drive is a fresh run, so the leg is the
    /// abortable unit here.
    pub(crate) fn sequencer_abort_impl(&self, repo: &Repository) -> EngineResult<()> {
        if merge_in_progress(repo) {
            return Err(EngineError::Invalid(
                "a merge is in progress (abort the merge first)".into(),
            ));
        }
        if !sequencer_in_progress(repo) {
            return Err(EngineError::Invalid(
                "no cherry-pick or revert in progress".into(),
            ));
        }
        let head = head_commit(repo)?
            .ok_or_else(|| EngineError::Invalid("cannot abort: HEAD is unborn".into()))?;

        restore_workdir_to_head(repo, &head)?;

        // Index := HEAD, then drop the sequencer markers (safe: no merge runs).
        repo.reset_default(Some(head.as_object()), ["*"])?;
        clear_sequencer_files(repo);
        Ok(())
    }
}

fn sequencer_apply(
    repo: &Repository,
    shas: &[String],
    kind: Sequencer,
) -> EngineResult<MergeResult> {
    if merge_in_progress(repo) {
        return Err(EngineError::Invalid(
            "cannot run the sequencer while a merge is in progress".into(),
        ));
    }
    if shas.is_empty() {
        return Ok(MergeResult {
            outcome: MergeOutcome::UpToDate,
            conflicts: Vec::new(),
            new_head: head_commit(repo)?.map(|c| c.id().to_string()),
        });
    }
    // A fresh run owns the sequencer state; leftovers from a previous
    // conflicted pick (the FE re-drive path) are stale.
    clear_sequencer_files(repo);

    let mut created: Option<String> = None;
    for sha in shas {
        let commit = resolve_commit(repo, sha)?;
        let head = head_commit(repo)?.ok_or_else(|| {
            EngineError::Invalid(format!("cannot {} onto an unborn HEAD", verb(kind)))
        })?;

        match kind {
            Sequencer::CherryPick => repo.cherrypick(&commit, None),
            Sequencer::Revert => repo.revert(&commit, None),
        }
        .map_err(|e| {
            EngineError::Invalid(format!("cannot {} `{}`: {e}", verb(kind), commit.id()))
        })?;

        // libgit2's cherrypick/revert reports success even when the 3-way
        // merge conflicted: it then leaves the full git state behind
        // (3-stage index entries, marker files in the workdir,
        // CHERRY_PICK_HEAD / REVERT_HEAD + MERGE_MSG) for the FE to resolve.
        let mut index = repo.index()?;
        if index.has_conflicts() {
            return Ok(MergeResult {
                outcome: MergeOutcome::Conflicted,
                conflicts: conflicts_impl(repo)?,
                new_head: created,
            });
        }

        let tree_oid = index.write_tree()?;
        let tree = repo.find_tree(tree_oid)?;
        let committer = committer_signature(repo)?;
        let (message, author) = match kind {
            Sequencer::CherryPick => (
                commit.message().unwrap_or_default().to_owned(),
                preserved_author(&commit)?,
            ),
            Sequencer::Revert => (
                revert_message(&commit.id().to_string(), &commit),
                committer.clone(),
            ),
        };
        let oid = repo.commit(Some("HEAD"), &author, &committer, &message, &tree, &[&head])?;
        created = Some(oid.to_string());
    }
    // The clean run consumed every step; libgit2 leaves its sequencer
    // markers behind on the success path too, so drop them.
    clear_sequencer_files(repo);
    Ok(MergeResult {
        outcome: MergeOutcome::Merged,
        conflicts: Vec::new(),
        new_head: created,
    })
}

// ---------------------------------------------------------------------------
// GitEngineM3
// ---------------------------------------------------------------------------

impl GitEngineM3 for Libgit2Engine {
    fn merge_branch(
        &self,
        repo: &Repository,
        ref_name: &str,
        opts: &MergeOptions,
    ) -> EngineResult<MergeResult> {
        if merge_in_progress(repo) {
            return Err(EngineError::Invalid(
                "a merge is already in progress (resolve or abort it first)".into(),
            ));
        }
        let ours = head_commit(repo)?
            .ok_or_else(|| EngineError::Invalid("cannot merge: HEAD is unborn".into()))?;
        let theirs = resolve_commit(repo, ref_name)?;
        let annotated = repo.find_annotated_commit(theirs.id())?;
        let (analysis, _) = repo.merge_analysis(&[&annotated])?;

        if analysis.is_up_to_date() {
            return Ok(MergeResult {
                outcome: MergeOutcome::UpToDate,
                conflicts: Vec::new(),
                new_head: Some(ours.id().to_string()),
            });
        }

        // `git merge --squash` of a fast-forwardable branch stages the diff
        // instead of moving the ref — the real-merge path below handles it
        // (base == ours, so the 3-way result is theirs' tree).
        if analysis.is_fast_forward() && !opts.no_ff && !opts.squash {
            // Workdir first, ref last (checkout_branch_target convention: a
            // failed checkout leaves the ref untouched).
            let mut checkout = CheckoutBuilder::new();
            checkout.safe();
            repo.checkout_tree(theirs.tree()?.as_object(), Some(&mut checkout))?;
            move_head_ref(repo, theirs.id(), "merge: fast-forward")?;
            return Ok(MergeResult {
                outcome: MergeOutcome::FastForward,
                conflicts: Vec::new(),
                new_head: Some(theirs.id().to_string()),
            });
        }

        let mut checkout = CheckoutBuilder::new();
        checkout.safe();
        let mut merge_opts = git2::MergeOptions::new();
        match opts.favor {
            MergeFavor::Ours => {
                merge_opts.file_favor(git2::FileFavor::Ours);
            }
            MergeFavor::Theirs => {
                merge_opts.file_favor(git2::FileFavor::Theirs);
            }
            MergeFavor::None => {}
        }
        repo.merge(&[&annotated], Some(&mut merge_opts), Some(&mut checkout))?;
        let mut index = repo.index()?;
        if index.has_conflicts() {
            if opts.squash {
                // `git merge --squash` conflicts carry no MERGE_HEAD: the
                // finishing commit must be a normal 1-parent commit. Keep the
                // conflicted index + workdir markers, drop the merge state.
                repo.cleanup_state()?;
            }
            return Ok(MergeResult {
                outcome: MergeOutcome::Conflicted,
                conflicts: conflicts_impl(repo)?,
                new_head: None,
            });
        }

        if opts.squash {
            // Stage-only result: index holds the merged entries (repo.merge
            // staged them), workdir has the content, HEAD did not move.
            // SQUASH_MSG seeds the commit bar; no merge state remains.
            std::fs::write(
                repo.path().join("SQUASH_MSG"),
                merge_message(repo, ref_name),
            )?;
            repo.cleanup_state()?;
            return Ok(MergeResult {
                outcome: MergeOutcome::Squashed,
                conflicts: Vec::new(),
                new_head: Some(ours.id().to_string()),
            });
        }

        if opts.no_commit {
            // MERGE_HEAD stays live: the FE resolve/commit flow finishes the
            // merge commit through the normal commit path.
            return Ok(MergeResult {
                outcome: MergeOutcome::NoCommit,
                conflicts: Vec::new(),
                new_head: Some(ours.id().to_string()),
            });
        }

        let tree_oid = index.write_tree()?;
        let tree = repo.find_tree(tree_oid)?;
        let committer = committer_signature(repo)?;
        let message = merge_message(repo, ref_name);
        let oid = repo.commit(
            Some("HEAD"),
            &committer,
            &committer,
            &message,
            &tree,
            &[&ours, &theirs],
        )?;
        // git_merge leaves MERGE_HEAD/MERGE_MSG/MERGE_MODE behind even on the
        // clean path — the merge commit consumes them, exactly like
        // `git commit` would.
        repo.cleanup_state()?;
        Ok(MergeResult {
            outcome: MergeOutcome::Merged,
            conflicts: Vec::new(),
            new_head: Some(oid.to_string()),
        })
    }

    fn merge_abort(&self, repo: &Repository) -> EngineResult<()> {
        if !merge_in_progress(repo) {
            return Err(EngineError::Invalid("no merge in progress".into()));
        }
        let head = repo.head()?.peel_to_commit()?;

        restore_workdir_to_head(repo, &head)?;

        // Index := HEAD (mixed), then drop MERGE_HEAD/MERGE_MSG/MERGE_MODE.
        repo.reset_default(Some(head.as_object()), ["*"])?;
        repo.cleanup_state()?;
        Ok(())
    }

    fn conflicts(&self, repo: &Repository) -> EngineResult<Vec<ConflictFile>> {
        conflicts_impl(repo)
    }

    fn conflict_resolve(
        &self,
        repo: &Repository,
        path: &str,
        res: ConflictResolution,
        custom_content: Option<&[u8]>,
    ) -> EngineResult<()> {
        let mut index = repo.index()?;
        // Editor bytes win over the preset: the FE hands back the merged
        // content of its 3-way editor (typically after a `Both` preview) and
        // the path resolves + stages in one step.
        if let Some(content) = custom_content {
            write_workdir_file(repo, path, content)?;
            index.conflict_remove(Path::new(path))?;
            index.add_path(Path::new(path))?;
            index.write()?;
            return Ok(());
        }
        match res {
            ConflictResolution::Both => {
                // Marker preview for the 3-way editor; the index stays
                // conflicted so a later custom resolution finishes the file.
                let ours = stage_content(repo, &index, path, 2)?;
                let theirs = stage_content(repo, &index, path, 3)?;
                let content = render_conflict_markers(path, &ours, &theirs);
                write_workdir_file(repo, path, &content)?;
                return Ok(());
            }
            ConflictResolution::Ours | ConflictResolution::Theirs => {
                let stage = if res == ConflictResolution::Ours {
                    2
                } else {
                    3
                };
                let content = stage_content(repo, &index, path, stage)?;
                write_workdir_file(repo, path, &content)?;
            }
        }
        index.conflict_remove(Path::new(path))?;
        index.add_path(Path::new(path))?;
        index.write()?;
        Ok(())
    }

    fn cherry_pick(&self, repo: &Repository, shas: &[String]) -> EngineResult<MergeResult> {
        sequencer_apply(repo, shas, Sequencer::CherryPick)
    }

    fn sequencer_abort(&self, repo: &Repository) -> EngineResult<()> {
        self.sequencer_abort_impl(repo)
    }

    fn revert(&self, repo: &Repository, shas: &[String]) -> EngineResult<MergeResult> {
        sequencer_apply(repo, shas, Sequencer::Revert)
    }

    fn reset(&self, repo: &Repository, kind: ResetKind, to: &str) -> EngineResult<()> {
        let target = resolve_commit(repo, to)?;
        // Workdir/index first, ref last: a failed checkout leaves the ref
        // (and HEAD) untouched.
        match kind {
            ResetKind::Soft => {}
            ResetKind::Mixed => {
                // Index := target tree, workdir untouched.
                repo.reset_default(Some(target.as_object()), ["*"])?;
            }
            ResetKind::Hard => {
                // checkout_tree also updates the index (libgit2 default).
                let mut checkout = CheckoutBuilder::new();
                checkout.force();
                repo.checkout_tree(target.tree()?.as_object(), Some(&mut checkout))?;
            }
        }
        move_head_ref(repo, target.id(), &format!("reset: moving to {to}"))?;
        Ok(())
    }

    // ---------- D2 rebase forwarding ----------
    fn rebase_start(
        &self,
        repo: &Repository,
        plan: &[RebaseStep],
        onto: Option<&str>,
    ) -> EngineResult<RebaseState> {
        self.rebase_start_impl(repo, plan, onto)
    }
    fn rebase_state(&self, repo: &Repository) -> EngineResult<RebaseState> {
        self.rebase_state_impl(repo)
    }
    fn rebase_continue(&self, repo: &Repository) -> EngineResult<RebaseState> {
        self.rebase_continue_impl(repo)
    }
    fn rebase_abort(&self, repo: &Repository) -> EngineResult<()> {
        self.rebase_abort_impl(repo)
    }

    // ---------- D3 stash/worktree/reflog forwarding ----------
    fn stash_list(&self, repo: &Repository) -> EngineResult<Vec<StashInfo>> {
        self.stash_list_impl(repo)
    }
    fn stash_push(
        &self,
        repo: &Repository,
        message: Option<&str>,
        keep_index: bool,
        include_untracked: bool,
    ) -> EngineResult<()> {
        self.stash_push_impl(repo, message, keep_index, include_untracked)
    }
    fn stash_apply(&self, repo: &Repository, index: u32, pop: bool) -> EngineResult<()> {
        self.stash_apply_impl(repo, index, pop)
    }
    fn stash_drop(&self, repo: &Repository, index: u32) -> EngineResult<()> {
        self.stash_drop_impl(repo, index)
    }
    fn stash_branch(&self, repo: &Repository, name: &str, index: u32) -> EngineResult<()> {
        self.stash_branch_impl(repo, name, index)
    }
    fn worktrees(&self, repo: &Repository) -> EngineResult<Vec<WorktreeInfo>> {
        self.worktrees_impl(repo)
    }
    fn worktree_add(
        &self,
        repo: &Repository,
        path: &str,
        branch: Option<&str>,
        new_branch: Option<&str>,
    ) -> EngineResult<()> {
        self.worktree_add_impl(repo, path, branch, new_branch)
    }
    fn worktree_remove(&self, repo: &Repository, name: &str, force: bool) -> EngineResult<()> {
        self.worktree_remove_impl(repo, name, force)
    }
    fn reflog(&self, repo: &Repository, name: Option<&str>) -> EngineResult<Vec<ReflogEntry>> {
        self.reflog_impl(repo, name)
    }

    // ---------- D4 checkpoint forwarding ----------
    fn checkpoint_create(&self, repo: &Repository, reason: &str) -> EngineResult<CheckpointInfo> {
        self.checkpoint_create_impl(repo, reason)
    }
    fn checkpoints(&self, repo: &Repository) -> EngineResult<Vec<CheckpointInfo>> {
        self.checkpoints_impl(repo)
    }
    fn checkpoint_restore(&self, repo: &Repository, id: &str) -> EngineResult<()> {
        self.checkpoint_restore_impl(repo, id)
    }
    fn checkpoint_gc(&self, repo: &Repository, older_than_days: u32) -> EngineResult<u32> {
        self.checkpoint_gc_impl(repo, older_than_days)
    }

    // ---------- M10: bisect / describe / autosquash forwarding ----------
    fn bisect_start(
        &self,
        repo: &Repository,
        bad: Option<&str>,
        good: Option<&str>,
    ) -> EngineResult<BisectState> {
        self.bisect_start_impl(repo, bad, good)
    }
    fn bisect_state(&self, repo: &Repository) -> EngineResult<BisectState> {
        self.bisect_state_impl(repo)
    }
    fn bisect_mark(&self, repo: &Repository, mark: BisectMark) -> EngineResult<BisectState> {
        self.bisect_mark_impl(repo, mark)
    }
    fn bisect_reset(&self, repo: &Repository) -> EngineResult<()> {
        self.bisect_reset_impl(repo)
    }
    fn describe(&self, repo: &Repository, spec: &str) -> EngineResult<String> {
        self.describe_impl(repo, spec)
    }
    fn autosquash_plan(&self, repo: &Repository, base: &str) -> EngineResult<Vec<RebaseStep>> {
        self.autosquash_plan_impl(repo, base)
    }
}
