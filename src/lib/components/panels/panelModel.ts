/**
 * Pure logic for the M3 panels (Stash / Worktree / Reflog) and the RepoView
 * conflict banner — no runes, no DOM, fully unit-testable.
 *
 * Conventions mirror `statusModel.ts`: small functions over IPC types, one
 * formatting/bucketing rule each, so the Svelte components stay declarative.
 */

import type {
  BranchInfo,
  ConflictFile,
  ReflogEntry,
  RepoStatus,
  StashInfo,
  WorktreeInfo,
} from "$lib/ipc/types";

// ---------------------------------------------------------------------------
// Shared formatting
// ---------------------------------------------------------------------------

/** Short sha for display (first 7 chars; passes through short/empty input). */
export function shortSha(sha: string): string {
  return sha.slice(0, 7);
}

/** Final path segment on `/` or `\` (worktree / repo display names). */
export function baseName(path: string): string {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return cut === -1 ? path : path.slice(cut + 1);
}

/**
 * Coarse relative age for a unix-seconds timestamp ("just now", "5m ago",
 * "3h ago", "2d ago", "3w ago", "4mo ago", "1y ago"). Future timestamps
 * (clock skew) read as "just now". `now` is injectable for tests.
 */
export function relativeAge(unixSeconds: number, now: number = Date.now()): string {
  const deltaMs = now - unixSeconds * 1000;
  if (deltaMs < 45_000) return "just now";
  const minutes = Math.floor(deltaMs / 60_000);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  const weeks = Math.floor(days / 7);
  if (days < 30) return `${weeks}w ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;
  const years = Math.floor(days / 365);
  return `${years}y ago`;
}

// ---------------------------------------------------------------------------
// Conflict banner (RepoView)
// ---------------------------------------------------------------------------

/** Which in-progress operation a conflict situation belongs to. */
export type ConflictSource = "rebase" | "merge" | "sequencer";

/**
 * Reads the in-progress op off a status snapshot (`null` when the working
 * copy is clean of any in-progress operation). Rebase wins over the
 * sequencer flag (a rebase drives the sequencer, and only rebase can be
 * aborted with `rebase_abort`).
 */
export function conflictSource(status: RepoStatus): ConflictSource | null {
  if (status.rebasing) return "rebase";
  if (status.merging) return "merge";
  if (status.sequencer) return "sequencer";
  return null;
}

/** Human label for the banner ("<label> in progress — N conflicted files"). */
export function conflictSourceLabel(source: ConflictSource): string {
  switch (source) {
    case "rebase":
      return "Rebase";
    case "merge":
      return "Merge";
    case "sequencer":
      return "Cherry-pick/revert";
  }
}

/** Which backend command aborts the in-progress op (`rebase` → rebase_abort). */
export function conflictAbortCommand(
  source: ConflictSource,
): "rebase_abort" | "merge_abort" {
  return source === "rebase" ? "rebase_abort" : "merge_abort";
}

// ---------------------------------------------------------------------------
// StashPanel
// ---------------------------------------------------------------------------

/** `stash@{n}` ref label for a stash entry. */
export function stashRef(info: Pick<StashInfo, "index">): string {
  return `stash@{${info.index}}`;
}

// ---------------------------------------------------------------------------
// WorktreePanel
// ---------------------------------------------------------------------------

/** Badge descriptor for a worktree chip row. */
export interface WorktreeBadge {
  label: string;
  /** Dangerous badges (locked / prunable) render in the error color. */
  danger: boolean;
  /** Hover/explanation text (e.g. the prunable reason). */
  title: string;
}

/**
 * Classifies a worktree into display badges: `detached` (informational),
 * `locked` and `prunable` (dangerous; the latter carries the reason).
 */
export function worktreeBadges(
  w: Pick<WorktreeInfo, "locked" | "prunable" | "detached">,
): WorktreeBadge[] {
  const badges: WorktreeBadge[] = [];
  if (w.detached) {
    badges.push({ label: "detached", danger: false, title: "Detached HEAD" });
  }
  if (w.locked) {
    badges.push({ label: "locked", danger: true, title: "Worktree is locked" });
  }
  if (w.prunable !== null && w.prunable !== "") {
    badges.push({
      label: "prunable",
      danger: true,
      title: `Prunable: ${w.prunable}`,
    });
  }
  return badges;
}

/** Removing a locked or prunable worktree needs the `force` flag. */
export function worktreeNeedsForce(
  w: Pick<WorktreeInfo, "locked" | "prunable">,
): boolean {
  return w.locked || (w.prunable !== null && w.prunable !== "");
}

/** Branch label for a worktree row (`branch` or `detached @ <sha7>`). */
export function worktreeRefLabel(
  w: Pick<WorktreeInfo, "branch" | "detached" | "head">,
): string {
  if (w.branch) return w.branch;
  if (w.detached && w.head) return `detached @ ${shortSha(w.head)}`;
  return w.detached ? "detached" : "unknown";
}

/**
 * Branches eligible for a new worktree: everything the repo reports minus
 * branches already checked out somewhere this client can see (`is_head`).
 */
export function worktreeCandidateBranches(
  all: readonly BranchInfo[],
): BranchInfo[] {
  return all.filter((b) => !b.is_head);
}

// ---------------------------------------------------------------------------
// ReflogPanel
// ---------------------------------------------------------------------------

/** Display cap — the full list is kept, only rendering stops here. */
export const REFLOG_DISPLAY_CAP = 500;

/** One calendar-day bucket of reflog entries (newest-first within the day). */
export interface ReflogDayGroup {
  /** "Today" / "Yesterday" / locale date of the bucket's day. */
  label: string;
  entries: ReflogEntry[];
}

/** Local calendar-day key (`Y-M-D` numbers) for a unix-seconds timestamp. */
function dayKey(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  // Plain local Y/M/D comparison is enough for grouping (DST shifts the
  // hour, never the calendar date at the usual reflog granularity).
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

/** Human label for a local calendar day relative to `now`. */
export function reflogDayLabel(unixSeconds: number, now: number = Date.now()): string {
  const d = new Date(unixSeconds * 1000);
  const today = new Date(now);
  const startOfToday = new Date(
    today.getFullYear(),
    today.getMonth(),
    today.getDate(),
  );
  const day = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const daysAgo = Math.round((startOfToday.getTime() - day.getTime()) / 86_400_000);
  if (daysAgo === 0) return "Today";
  if (daysAgo === 1) return "Yesterday";
  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  }).format(day);
}

/**
 * Groups reflog entries (newest-first, as the backend returns them) into
 * consecutive calendar-day buckets. Grouping is stable: entries whose day
 * differs from the previous one start a new group, so the output order
 * matches the input order exactly.
 */
export function groupReflogByDay(
  entries: readonly ReflogEntry[],
  now: number = Date.now(),
): ReflogDayGroup[] {
  const groups: ReflogDayGroup[] = [];
  let current: ReflogDayGroup | null = null;
  let currentKey = "";
  for (const entry of entries) {
    const key = dayKey(entry.signature.time);
    if (current === null || key !== currentKey) {
      current = {
        label: reflogDayLabel(entry.signature.time, now),
        entries: [entry],
      };
      groups.push(current);
      currentKey = key;
    } else {
      current.entries.push(entry);
    }
  }
  return groups;
}
