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

// ---------- M2: mutation model ----------

export interface LineRange {
  start: number;
  end: number;
}

export type StageTarget =
  | { file: string }
  | { hunk: { path: string; hunk: number } }
  | { lines: { path: string; hunk: number; ranges: LineRange[] } };

export interface StageRequest {
  targets: StageTarget[];
  unstage: boolean;
}

export interface CommitOptions {
  message: string;
  amend: boolean;
  no_verify: boolean;
  allow_empty: boolean;
  author: GitSignature | null;
}

export interface SigningInfo {
  active: boolean;
  format: string;
  key_id: string | null;
}

export interface HookInfo {
  kind: string;
  present: boolean;
  executable: boolean;
}

export interface FetchOptions {
  remote: string;
  prune: boolean;
  refs: string[];
  depth: number | null;
}

export interface PullOptions {
  remote: string;
  branch: string;
  ff_only: boolean;
  rebase: boolean;
}

export interface PushOptions {
  remote: string;
  branch: string;
  force: boolean;
  set_upstream: boolean;
}

export interface NetStats {
  received_bytes: number;
  objects: number;
  updated_refs: [string, string][];
}

export interface BranchInfo {
  name: string;
  sha: string;
  upstream: string | null;
  ahead: number;
  behind: number;
  gone: boolean;
  is_head: boolean;
}

export interface RemoteInfo {
  name: string;
  url: string;
  push_url: string | null;
}

export interface OpProgress {
  repo_id: string;
  op_id: string;
  kind: string;
  message: string;
  pct: number | null;
  done: boolean;
  error: string | null;
}

export interface AuthRequest {
  op_id: string;
  repo_id: string;
  url: string;
  kind: "https-user" | "https-pass" | "ssh-passphrase";
  prompt: string;
}

// ---------- M3: power + safety ----------

export type ConflictResolution = "ours" | "theirs" | "both";

export interface ConflictFile {
  path: string;
  has_base: boolean;
  has_ours: boolean;
  has_theirs: boolean;
  source: string;
}

export type MergeOutcome = "fast_forward" | "merged" | "conflicted" | "up_to_date";

export interface MergeResult {
  outcome: MergeOutcome;
  conflicts: ConflictFile[];
  new_head: string | null;
}

export type ResetKind = "soft" | "mixed" | "hard";

export interface RebaseStep {
  sha: string;
  action: string;
  new_message: string | null;
}

export interface RebaseState {
  active: boolean;
  plan: RebaseStep[];
  current: number;
  paused_for_edit: boolean;
  rewritten: [string, string][];
}

export interface StashInfo {
  index: number;
  sha: string;
  message: string;
  author: GitSignature;
}

export interface WorktreeInfo {
  path: string;
  name: string;
  branch: string | null;
  head: string | null;
  detached: boolean;
  locked: boolean;
  prunable: string | null;
}

export interface ReflogEntry {
  old_sha: string;
  new_sha: string;
  signature: GitSignature;
  message: string;
}

export interface CheckpointInfo {
  id: string;
  reason: string;
  ref_name: string;
  created_at: number;
  branch: string | null;
  has_worktree_state: boolean;
}

export interface PreviewFile {
  path: string;
  change: string;
}

export interface PreviewInfo {
  summary: string;
  files: PreviewFile[];
  checkpoint_id: string | null;
}
