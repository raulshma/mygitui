/**
 * Pure model for the Agent-mode changes list: working-copy grouping
 * (Staged / Changed / Untracked) and the row-label helpers the list and
 * the "git info" card share. No IPC, no Svelte — fully unit tested in
 * `changesModel.test.ts`.
 */
import type { ChangeKind, StatusEntry } from "$lib/ipc/types";

/** The three Agent-mode sections of the working copy. */
export interface StatusGroups {
  staged: StatusEntry[];
  changed: StatusEntry[];
  untracked: StatusEntry[];
}

/**
 * Groups status entries into Staged / Changed / Untracked, preserving the
 * backend's entry order inside each group. One entry lands in exactly one
 * group: an untracked worktree file always goes to `untracked` (it cannot
 * also be meaningfully staged); otherwise an index-side change is `staged`
 * — even when the worktree is dirty too (this minimal list stages the
 * worktree side separately, so "both" shows under Staged); otherwise a
 * dirty worktree (including `conflicted`) is `changed`.
 */
export function groupStatusEntries(entries: StatusEntry[]): StatusGroups {
  const groups: StatusGroups = { staged: [], changed: [], untracked: [] };
  for (const entry of entries) {
    if (entry.worktree === "untracked") {
      groups.untracked.push(entry);
    } else if (entry.index !== "unmodified") {
      groups.staged.push(entry);
    } else if (entry.worktree !== "unmodified") {
      groups.changed.push(entry);
    }
  }
  return groups;
}

/** One-letter kind badge for a row (`M`/`A`/`D`/`R`/`C`/`U`, `!`, `·`). */
export function kindLetter(kind: ChangeKind): string {
  switch (kind) {
    case "added":
      return "A";
    case "deleted":
      return "D";
    case "modified":
      return "M";
    case "renamed":
      return "R";
    case "copied":
      return "C";
    case "untracked":
      return "U";
    case "conflicted":
      return "!";
    default:
      return "·";
  }
}

export interface SplitPath {
  /** File name including extension. */
  base: string;
  /** Parent directory with separators normalized away (`""` at root). */
  dir: string;
}

/** Splits `a/b/c.txt` into base `c.txt` + dir `a/b` (rows show both). */
export function splitPath(path: string): SplitPath {
  const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (idx === -1) return { base: path, dir: "" };
  return { base: path.slice(idx + 1), dir: path.slice(0, idx) };
}

/**
 * Which diff side a row opens: a purely staged entry diffs HEAD→index,
 * everything else diffs index→worktree (mirrors StatusPanel's scope rule).
 */
export function openSide(entry: StatusEntry): "worktree" | "index" {
  return entry.index !== "unmodified" && entry.worktree === "unmodified"
    ? "index"
    : "worktree";
}

/** The worktree side can be staged from this row (never conflicts). */
export function isStageable(entry: StatusEntry): boolean {
  return entry.worktree !== "unmodified" && entry.worktree !== "conflicted";
}

/**
 * Discard applies to worktree-only changes (Changed + Untracked rows):
 * staged-side rows have nothing to reset and conflicted rows need an
 * explicit resolution.
 */
export function isDiscardable(entry: StatusEntry): boolean {
  return (
    entry.index === "unmodified" &&
    entry.worktree !== "unmodified" &&
    entry.worktree !== "conflicted"
  );
}
