/**
 * TS mirror of `src-tauri/src/engine/types.rs`.
 * CONTRACT: additive changes only; keep in lockstep with the Rust side.
 */

export type RepoId = string;

export type ChangeKind =
  | "unmodified"
  | "added"
  | "deleted"
  | "modified"
  | "renamed"
  | "copied"
  | "untracked"
  | "conflicted"
  | "ignored";

export interface StatusEntry {
  path: string;
  old_path: string | null;
  index: ChangeKind;
  worktree: ChangeKind;
}

export interface RepoStatus {
  branch: string | null;
  head: string | null;
  detached: boolean;
  ahead: number;
  behind: number;
  merging: boolean;
  rebasing: boolean;
  sequencer: boolean;
  entries: StatusEntry[];
}

/** Externally tagged: bare strings for unit sides, `{"commit": "abc"}` otherwise. */
export type DiffSide = "worktree" | "index" | "head" | { commit: string };

export interface DiffLine {
  old_no: number | null;
  new_no: number | null;
  /** '+', '-', ' ', '=' */
  origin: string;
  text: string;
  highlights: [number, number][];
}

export interface DiffHunk {
  old_start: number;
  new_start: number;
  lines: DiffLine[];
}

export interface FileDiff {
  path: string;
  old_path: string | null;
  binary: boolean;
  is_image: boolean;
  additions: number;
  deletions: number;
  hunks: DiffHunk[];
}

export interface GitSignature {
  name: string;
  email: string;
  /** Unix seconds. */
  time: number;
  /** Minutes east of UTC. */
  offset_minutes: number;
}

export interface CommitInfo {
  sha: string;
  parents: string[];
  author: GitSignature;
  committer: GitSignature;
  message: string;
  summary: string;
  refs: string[];
}

export interface LogFilter {
  text?: string | null;
  regex: boolean;
  author?: string | null;
  path?: string | null;
  after_unix?: number | null;
  before_unix?: number | null;
  refs: string[];
  follow: boolean;
}

export interface BlameLine {
  line_no: number;
  sha: string;
  signature: GitSignature;
  final_sha: string;
  final_signature: GitSignature;
  text: string;
}

export interface RepoInfo {
  repo_id: string;
  root: string;
  name: string;
  bare: boolean;
  git_dir: string;
}

export interface GraphEdge {
  from: number;
  to: number;
}

export interface GraphRow {
  sha: string;
  lane: number;
  edges: GraphEdge[];
  lane_count: number;
}

export interface LogPage {
  commits: CommitInfo[];
  rows: GraphRow[];
  next_cursor: string | null;
  generation: number;
}
