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

use super::types::{BlameLine, CommitInfo, DiffSide, FileDiff, LogFilter, RepoStatus};

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
}
