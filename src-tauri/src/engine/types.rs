//! Core data model shared across the git engine, IPC boundary, and UI.
//!
//! CONTRACT: these types are mirrored in `src/lib/ipc/types.ts`. Changes here
//! MUST be mirrored there in the same patch — additive changes only within a
//! milestone; breaking changes go through the orchestrator.

use serde::{Deserialize, Serialize};

/// Stable identifier for an open repo (one per app tab).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Unmodified,
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    Untracked,
    Conflicted,
    Ignored,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusEntry {
    pub path: String,
    /// Previous path for renames.
    pub old_path: Option<String>,
    /// Staged (index vs HEAD) change kind; `Unmodified` when nothing staged.
    pub index: ChangeKind,
    /// Worktree (worktree vs index) change kind; `Unmodified` when clean.
    pub worktree: ChangeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatus {
    /// Current branch name; `None` when detached.
    pub branch: Option<String>,
    pub head: Option<String>,
    pub detached: bool,
    pub ahead: u32,
    pub behind: u32,
    /// True while MERGE_HEAD exists.
    pub merging: bool,
    /// True while a rebase is in progress (any phase).
    pub rebasing: bool,
    /// True while a cherry-pick or revert is in progress.
    pub sequencer: bool,
    pub entries: Vec<StatusEntry>,
}

/// One side of a diff comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    /// Working directory.
    Worktree,
    /// The index (staged content).
    Index,
    /// HEAD commit.
    Head,
    /// Any commit-ish string (sha, branch, tag, ref).
    Commit(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// '+', '-', ' ' (context), or '=' for hunk metadata lines.
    pub origin: char,
    pub text: String,
    /// Word-level highlight ranges (byte start, byte end) within `text`,
    /// filled by the imara-diff word pass; may be empty.
    pub highlights: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub binary: bool,
    pub is_image: bool,
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitSignature {
    pub name: String,
    pub email: String,
    /// Unix seconds.
    pub time: i64,
    /// Timezone offset in minutes east of UTC.
    pub offset_minutes: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: GitSignature,
    pub committer: GitSignature,
    /// Full commit message.
    pub message: String,
    /// First line of the message.
    pub summary: String,
    /// Ref decorations (branches/tags/HEAD) pointing at this commit.
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogFilter {
    pub text: Option<String>,
    /// When set, `text` is treated as a regex.
    pub regex: bool,
    pub author: Option<String>,
    pub path: Option<String>,
    pub after_unix: Option<i64>,
    pub before_unix: Option<i64>,
    /// Restrict the walk to these commit-ish roots; empty = all refs + HEAD.
    pub refs: Vec<String>,
    /// Follow renames for `path` (single-path walks only).
    pub follow: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlameLine {
    pub line_no: u32,
    pub sha: String,
    pub signature: GitSignature,
    /// Final (latest) commit that touched this line.
    pub final_sha: String,
    pub final_signature: GitSignature,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoInfo {
    pub repo_id: RepoId,
    /// Absolute workdir path.
    pub root: String,
    /// Directory name (tab title).
    pub name: String,
    /// True when the repo is bare (no workdir).
    pub bare: bool,
    pub git_dir: String,
}
