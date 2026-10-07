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
    LogFilter, NetStats, PullOptions, PushOptions, RemoteInfo, RepoStatus, SigningInfo,
    StageRequest,
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

    fn branch_switch(
        &self,
        _repo: &Repository,
        _name: &str,
        _force: bool,
    ) -> EngineResult<()> {
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
