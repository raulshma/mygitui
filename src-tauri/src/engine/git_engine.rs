//! The `GitEngine` trait: every git read/mutation the app can perform.
//!
//! CONTRACT: `Libgit2Engine` (git2-rs) is the primary implementation; `CliEngine`
//! shells out to system git for capability-table fallbacks (partial clone,
//! sparse-checkout edges, signed commits, exotic rebase states). UI-facing code
//! only ever talks to this trait.
//!
//! M1 scope: read operations. Mutations arrive in M2 and extend this trait —
//! additive only.

use git2::Repository;

use super::types::{
    BlameLine, BranchInfo, CommitInfo, CommitOptions, DiffSide, FetchOptions, FileDiff, HookInfo,
    LogFilter, NetStats, PullOptions, PushOptions, RemoteBranchInfo, RemoteInfo, RepoStatus,
    SigningInfo, StageRequest, StageTarget, TagInfo,
};

pub type EngineResult<T> = Result<T, EngineError>;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("invalid path or ref: {0}")]
    Invalid(String),
    #[error("operation not supported by this engine: {0}")]
    Unsupported(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Read operations for one open repository.
///
/// All methods are synchronous: git2 is not async. IPC commands wrap them in
/// `tauri::async_runtime::spawn_blocking`.
pub trait GitEngine: Send + Sync {
    fn status(&self, repo: &Repository) -> EngineResult<RepoStatus>;

    /// Diff two sides, optionally restricted to `paths` (prefix-matched).
    fn diff(
        &self,
        repo: &Repository,
        old: &DiffSide,
        new: &DiffSide,
        paths: Option<&[String]>,
    ) -> EngineResult<Vec<FileDiff>>;

    /// One page of history in topological/date order. `after` is the last sha
    /// of the previous page (None = first page). Returns the page plus a
    /// `next_cursor` (None when the walk is exhausted).
    fn log(
        &self,
        repo: &Repository,
        filter: &LogFilter,
        limit: usize,
        after: Option<&str>,
    ) -> EngineResult<(Vec<CommitInfo>, Option<String>)>;

    fn blame(
        &self,
        repo: &Repository,
        path: &str,
        from: Option<&str>,
    ) -> EngineResult<Vec<BlameLine>>;

    /// List local + remote branches and tags with their target shas
    /// (decorations model used by the log walker and branch panel).
    fn refs(&self, repo: &Repository) -> EngineResult<Vec<(String, String)>>;

    // ---------- M2: mutations (default = unsupported until engines land them) ----------

    fn stage(&self, _repo: &Repository, _req: &StageRequest) -> EngineResult<()> {
        Err(EngineError::Unsupported("stage".into()))
    }

    fn stage_all(&self, _repo: &Repository, _unstage: bool) -> EngineResult<()> {
        Err(EngineError::Unsupported("stage_all".into()))
    }

    /// Throw away local changes for paths/hunks (`git checkout --`).
    /// Callers snapshot a checkpoint first.
    fn discard(&self, _repo: &Repository, _targets: &[StageTarget]) -> EngineResult<()> {
        Err(EngineError::Unsupported("discard".into()))
    }

    fn commit(&self, _repo: &Repository, _opts: &CommitOptions) -> EngineResult<String> {
        Err(EngineError::Unsupported("commit".into()))
    }

    fn signing_info(&self, _repo: &Repository) -> EngineResult<SigningInfo> {
        Err(EngineError::Unsupported("signing_info".into()))
    }

    fn hooks(&self, _repo: &Repository) -> EngineResult<Vec<HookInfo>> {
        Err(EngineError::Unsupported("hooks".into()))
    }

    fn branches(&self, _repo: &Repository) -> EngineResult<Vec<BranchInfo>> {
        Err(EngineError::Unsupported("branches".into()))
    }

    fn branch_create(
        &self,
        _repo: &Repository,
        _name: &str,
        _from: Option<&str>,
        _checkout: bool,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("branch_create".into()))
    }

    fn branch_switch(&self, _repo: &Repository, _name: &str, _force: bool) -> EngineResult<()> {
        Err(EngineError::Unsupported("branch_switch".into()))
    }

    /// Merged-into check used by delete guards and the cleanup wizard.
    fn branch_is_merged(&self, _repo: &Repository, _name: &str, _into: &str) -> EngineResult<bool> {
        Err(EngineError::Unsupported("branch_is_merged".into()))
    }

    fn branch_delete(&self, _repo: &Repository, _name: &str, _force: bool) -> EngineResult<()> {
        Err(EngineError::Unsupported("branch_delete".into()))
    }

    fn branch_rename(&self, _repo: &Repository, _old: &str, _new: &str) -> EngineResult<()> {
        Err(EngineError::Unsupported("branch_rename".into()))
    }

    fn tag_create(
        &self,
        _repo: &Repository,
        _name: &str,
        _target: Option<&str>,
        _message: Option<&str>,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("tag_create".into()))
    }

    fn tag_delete(&self, _repo: &Repository, _name: &str) -> EngineResult<()> {
        Err(EngineError::Unsupported("tag_delete".into()))
    }

    /// Tags with metadata (annotated: tagger + message).
    fn tag_list(&self, _repo: &Repository) -> EngineResult<Vec<TagInfo>> {
        Err(EngineError::Unsupported("tag_list".into()))
    }

    /// Remote-tracking branches across all remotes.
    fn remote_branches(&self, _repo: &Repository) -> EngineResult<Vec<RemoteBranchInfo>> {
        Err(EngineError::Unsupported("remote_branches".into()))
    }

    /// Create + switch to a local branch tracking `<remote>/<name>`
    /// (`git switch <name>` from a remote branch). Returns the local name.
    fn branch_checkout_remote(
        &self,
        _repo: &Repository,
        _remote: &str,
        _name: &str,
        _new_local: Option<&str>,
    ) -> EngineResult<String> {
        Err(EngineError::Unsupported("branch_checkout_remote".into()))
    }

    fn remotes(&self, _repo: &Repository) -> EngineResult<Vec<RemoteInfo>> {
        Err(EngineError::Unsupported("remotes".into()))
    }

    fn remote_add(&self, _repo: &Repository, _name: &str, _url: &str) -> EngineResult<()> {
        Err(EngineError::Unsupported("remote_add".into()))
    }

    fn remote_remove(&self, _repo: &Repository, _name: &str) -> EngineResult<()> {
        Err(EngineError::Unsupported("remote_remove".into()))
    }

    fn remote_set_url(
        &self,
        _repo: &Repository,
        _name: &str,
        _url: &str,
        _push: bool,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("remote_set_url".into()))
    }

    /// Network ops report progress through the callback (already on a
    /// blocking thread; keep it cheap — throttle upstream).
    fn fetch(
        &self,
        _repo: &Repository,
        _opts: &FetchOptions,
        _progress: &mut dyn FnMut(FetchProgress),
    ) -> EngineResult<NetStats> {
        Err(EngineError::Unsupported("fetch".into()))
    }

    fn pull(
        &self,
        _repo: &Repository,
        _opts: &PullOptions,
        _progress: &mut dyn FnMut(FetchProgress),
    ) -> EngineResult<NetStats> {
        Err(EngineError::Unsupported("pull".into()))
    }

    fn push(
        &self,
        _repo: &Repository,
        _opts: &PushOptions,
        _progress: &mut dyn FnMut(PushProgress),
    ) -> EngineResult<NetStats> {
        Err(EngineError::Unsupported("push".into()))
    }
}

/// libgit2 fetch/clone transfer progress.
#[derive(Debug, Clone, Copy, Default)]
pub struct FetchProgress {
    pub bytes: u64,
    pub objects_total: u32,
    pub objects_received: u32,
    pub done: bool,
}

/// Push-specific progress (per-ref).
#[derive(Debug, Clone, Default)]
pub struct PushProgress {
    pub current: u32,
    pub total: u32,
    pub bytes: u64,
    pub message: String,
}

// ---------- M3: power + safety (default Unsupported until lanes land) ----------

use super::types::{
    BisectMark, BisectState, CheckpointInfo, ConflictFile, ConflictResolution, MergeOptions,
    MergeResult, RebaseState, RebaseStep, ReflogEntry, ResetKind, StashInfo, WorktreeInfo,
};

/// Additional trait items live in an extension impl to keep M2 diff small.
pub trait GitEngineM3: Send + Sync {
    fn merge_branch(
        &self,
        _repo: &Repository,
        _ref_name: &str,
        _opts: &MergeOptions,
    ) -> EngineResult<MergeResult> {
        Err(EngineError::Unsupported("merge_branch".into()))
    }

    fn merge_abort(&self, _repo: &Repository) -> EngineResult<()> {
        Err(EngineError::Unsupported("merge_abort".into()))
    }

    fn conflicts(&self, _repo: &Repository) -> EngineResult<Vec<ConflictFile>> {
        Err(EngineError::Unsupported("conflicts".into()))
    }

    fn conflict_resolve(
        &self,
        _repo: &Repository,
        _path: &str,
        _res: ConflictResolution,
        _custom_content: Option<&[u8]>,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("conflict_resolve".into()))
    }

    fn cherry_pick(&self, _repo: &Repository, _shas: &[String]) -> EngineResult<MergeResult> {
        Err(EngineError::Unsupported("cherry_pick".into()))
    }

    fn revert(&self, _repo: &Repository, _shas: &[String]) -> EngineResult<MergeResult> {
        Err(EngineError::Unsupported("revert".into()))
    }

    /// Abort an interrupted cherry-pick/revert (`git cherry-pick --abort`).
    fn sequencer_abort(&self, _repo: &Repository) -> EngineResult<()> {
        Err(EngineError::Unsupported("sequencer_abort".into()))
    }

    fn reset(&self, _repo: &Repository, _kind: ResetKind, _to: &str) -> EngineResult<()> {
        Err(EngineError::Unsupported("reset".into()))
    }

    fn rebase_start(
        &self,
        _repo: &Repository,
        _plan: &[RebaseStep],
        _onto: Option<&str>,
    ) -> EngineResult<RebaseState> {
        Err(EngineError::Unsupported("rebase_start".into()))
    }

    fn rebase_state(&self, _repo: &Repository) -> EngineResult<RebaseState> {
        Err(EngineError::Unsupported("rebase_state".into()))
    }

    fn rebase_continue(&self, _repo: &Repository) -> EngineResult<RebaseState> {
        Err(EngineError::Unsupported("rebase_continue".into()))
    }

    fn rebase_abort(&self, _repo: &Repository) -> EngineResult<()> {
        Err(EngineError::Unsupported("rebase_abort".into()))
    }

    fn stash_list(&self, _repo: &Repository) -> EngineResult<Vec<StashInfo>> {
        Err(EngineError::Unsupported("stash_list".into()))
    }

    fn stash_push(
        &self,
        _repo: &Repository,
        _message: Option<&str>,
        _keep_index: bool,
        _include_untracked: bool,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("stash_push".into()))
    }

    fn stash_apply(&self, _repo: &Repository, _index: u32, _pop: bool) -> EngineResult<()> {
        Err(EngineError::Unsupported("stash_apply".into()))
    }

    fn stash_drop(&self, _repo: &Repository, _index: u32) -> EngineResult<()> {
        Err(EngineError::Unsupported("stash_drop".into()))
    }

    fn stash_branch(&self, _repo: &Repository, _name: &str, _index: u32) -> EngineResult<()> {
        Err(EngineError::Unsupported("stash_branch".into()))
    }

    fn worktrees(&self, _repo: &Repository) -> EngineResult<Vec<WorktreeInfo>> {
        Err(EngineError::Unsupported("worktrees".into()))
    }

    fn worktree_add(
        &self,
        _repo: &Repository,
        _path: &str,
        _branch: Option<&str>,
        _new_branch: Option<&str>,
    ) -> EngineResult<()> {
        Err(EngineError::Unsupported("worktree_add".into()))
    }

    fn worktree_remove(&self, _repo: &Repository, _name: &str, _force: bool) -> EngineResult<()> {
        Err(EngineError::Unsupported("worktree_remove".into()))
    }

    fn reflog(&self, _repo: &Repository, _name: Option<&str>) -> EngineResult<Vec<ReflogEntry>> {
        Err(EngineError::Unsupported("reflog".into()))
    }

    fn checkpoint_create(&self, _repo: &Repository, _reason: &str) -> EngineResult<CheckpointInfo> {
        Err(EngineError::Unsupported("checkpoint_create".into()))
    }

    fn checkpoints(&self, _repo: &Repository) -> EngineResult<Vec<CheckpointInfo>> {
        Err(EngineError::Unsupported("checkpoints".into()))
    }

    fn checkpoint_restore(&self, _repo: &Repository, _id: &str) -> EngineResult<()> {
        Err(EngineError::Unsupported("checkpoint_restore".into()))
    }

    fn checkpoint_gc(&self, _repo: &Repository, _older_than_days: u32) -> EngineResult<u32> {
        Err(EngineError::Unsupported("checkpoint_gc".into()))
    }

    // ---------- M10: bisect / describe / autosquash ----------

    fn bisect_start(
        &self,
        _repo: &Repository,
        _bad: Option<&str>,
        _good: Option<&str>,
    ) -> EngineResult<BisectState> {
        Err(EngineError::Unsupported("bisect_start".into()))
    }

    fn bisect_state(&self, _repo: &Repository) -> EngineResult<BisectState> {
        Err(EngineError::Unsupported("bisect_state".into()))
    }

    fn bisect_mark(&self, _repo: &Repository, _mark: BisectMark) -> EngineResult<BisectState> {
        Err(EngineError::Unsupported("bisect_mark".into()))
    }

    fn bisect_reset(&self, _repo: &Repository) -> EngineResult<()> {
        Err(EngineError::Unsupported("bisect_reset".into()))
    }

    /// `git describe --tags` for a commit-ish (falls back to the short sha).
    fn describe(&self, _repo: &Repository, _spec: &str) -> EngineResult<String> {
        Err(EngineError::Unsupported("describe".into()))
    }

    /// Builds a rebase plan from `fixup!` / `squash!` commits in
    /// `base..HEAD` (`git rebase --autosquash` semantics, expressed in our
    /// plan format for `rebase_start`).
    fn autosquash_plan(&self, _repo: &Repository, _base: &str) -> EngineResult<Vec<RebaseStep>> {
        Err(EngineError::Unsupported("autosquash_plan".into()))
    }
}

// ---------- M12: signature verify / branch trash / worktree prune ----------

use super::types::{BisectLogEntry, BranchTrashEntry, CommitSignature};

/// M12 extension trait. Impls live in the owning modules (`signing.rs`,
/// `trash.rs`, `stash.rs`) so lanes never share an impl block.
pub trait GitEngineM12: Send + Sync {
    /// Verification status of a commit's signature via the git CLI
    /// (`verify-commit`), so gpg/ssh-agent match terminal behavior.
    fn commit_signature(&self, _repo: &Repository, _sha: &str) -> EngineResult<CommitSignature> {
        Err(EngineError::Unsupported("commit_signature".into()))
    }

    /// Branches preserved under `refs/mygitui/trash/*`, newest first.
    fn branch_trash_list(&self, _repo: &Repository) -> EngineResult<Vec<BranchTrashEntry>> {
        Err(EngineError::Unsupported("branch_trash_list".into()))
    }

    /// Restore a trashed branch (optionally under a new name). Returns the
    /// branch name. Refuses when the name already exists unless renamed.
    fn branch_trash_restore(
        &self,
        _repo: &Repository,
        _id: &str,
        _new_name: Option<&str>,
    ) -> EngineResult<String> {
        Err(EngineError::Unsupported("branch_trash_restore".into()))
    }

    /// `git worktree prune` — drop stale administrative files. Returns the
    /// number of pruned worktrees reported by git.
    fn worktree_prune(&self, _repo: &Repository) -> EngineResult<u32> {
        Err(EngineError::Unsupported("worktree_prune".into()))
    }

    /// Recorded mark history for a live bisect (empty when none).
    fn bisect_log(&self, _repo: &Repository) -> EngineResult<Vec<BisectLogEntry>> {
        Err(EngineError::Unsupported("bisect_log".into()))
    }
}
