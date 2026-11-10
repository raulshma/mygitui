//! One precedence function for "which git operation owns the working
//! tree" (state-machines registry #3). The rule — rebase > cherry-pick >
//! revert > merge, bisect reported independently — used to be duplicated
//! by hand in `libgit2.rs` and `merge.rs`, and the remote engine's probe
//! had drifted (it checked `MERGE_HEAD` first, so a remote could label a
//! rebase's conflicts "merge"). Everything detector-shaped now goes
//! through [`Probe::in_progress`].
//!
//! The probe is detector-agnostic on purpose: the local engine supplies
//! git-dir paths plus libgit2's [`RepositoryState`]; the remote engine
//! supplies the marker names its one-shot `state_probe` exec found. An
//! active bisect is surfaced too — it used to be invisible to
//! `RepoStatus`, so the UI could show "clean" mid-search.

use std::path::Path;

use git2::RepositoryState;
use serde::Serialize;
use ts_rs::TS;

/// Which operation owns the working tree (also the `Conflicts.source`
/// label via [`InProgress::label`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InProgress {
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
}

impl InProgress {
    /// Label used by `Conflicts.source` (matches the historical strings).
    pub fn label(self) -> &'static str {
        match self {
            InProgress::Merging => "merge",
            InProgress::Rebasing => "rebase",
            InProgress::CherryPicking => "cherry-pick",
            InProgress::Reverting => "revert",
        }
    }
}

/// Everything a detector knows, in detector-agnostic form: which marker
/// files/dirs exist under the git dir and, locally, libgit2's repository
/// state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Probe {
    pub merge_head: bool,
    /// `rebase-merge/` or `rebase-apply/` exists.
    pub rebase_dirs: bool,
    /// mygitui's custom rebase state (`mygitui/rebase.json`,
    /// engine/rebase.rs).
    pub custom_rebase: bool,
    /// `sequencer/` exists (between steps of a pick/revert sequence the
    /// HEAD marker files may be absent).
    pub sequencer_dir: bool,
    pub cherry_pick_head: bool,
    pub revert_head: bool,
    /// `BISECT_LOG` exists (written when `git bisect start` begins).
    pub bisect_log: bool,
    /// libgit2's state (local only; the remote probe has no equivalent).
    pub state: Option<RepositoryState>,
}

impl Probe {
    /// Builds the probe from a local git dir + libgit2's state.
    pub(crate) fn from_gitdir(gitdir: &Path, state: RepositoryState) -> Self {
        Self {
            merge_head: gitdir.join("MERGE_HEAD").exists(),
            rebase_dirs: gitdir.join("rebase-merge").is_dir()
                || gitdir.join("rebase-apply").is_dir(),
            custom_rebase: gitdir.join("mygitui").join("rebase.json").exists(),
            sequencer_dir: gitdir.join("sequencer").is_dir(),
            cherry_pick_head: gitdir.join("CHERRY_PICK_HEAD").exists(),
            revert_head: gitdir.join("REVERT_HEAD").exists(),
            bisect_log: gitdir.join("BISECT_LOG").exists(),
            state: Some(state),
        }
    }

    fn rebasing(&self) -> bool {
        self.rebase_dirs
            || self.custom_rebase
            || matches!(
                self.state,
                Some(
                    RepositoryState::Rebase
                        | RepositoryState::RebaseInteractive
                        | RepositoryState::RebaseMerge
                        | RepositoryState::ApplyMailbox
                        | RepositoryState::ApplyMailboxOrRebase
                )
            )
    }

    /// The operation in progress, in `git status` precedence: rebase >
    /// cherry-pick > revert > merge. During a rebase the sequencer
    /// marker files may also be present; only the rebase is reported.
    /// `None` = clean of any in-progress operation.
    pub(crate) fn in_progress(&self) -> Option<InProgress> {
        if self.rebasing() {
            return Some(InProgress::Rebasing);
        }
        if self.cherry_pick_head
            || matches!(
                self.state,
                Some(RepositoryState::CherryPick | RepositoryState::CherryPickSequence)
            )
        {
            return Some(InProgress::CherryPicking);
        }
        if self.revert_head
            || matches!(
                self.state,
                Some(RepositoryState::Revert | RepositoryState::RevertSequence)
            )
        {
            return Some(InProgress::Reverting);
        }
        // Mid-sequence (sequencer dir without a HEAD marker): a pick
        // sequence keeps CHERRY_PICK_HEAD absent between steps, but the
        // dir is there — report the sequence rather than "clean".
        if self.sequencer_dir {
            return Some(if self.revert_head {
                InProgress::Reverting
            } else {
                InProgress::CherryPicking
            });
        }
        if self.merge_head || self.state == Some(RepositoryState::Merge) {
            return Some(InProgress::Merging);
        }
        None
    }

    /// A bisect search is active. Reported independently of the
    /// sequencer (git runs them one at a time, so nothing here overrides
    /// it — but a bisect alone must not read as "clean").
    pub(crate) fn bisecting(&self) -> bool {
        self.bisect_log
    }
}
