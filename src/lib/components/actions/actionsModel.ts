/**
 * Pure model for the M4 actions UI (no Svelte, no IPC — fully tested).
 *
 * Everything the ActionsPanel and CleanDialog render is computed here:
 * run status classification, output-line truncation, run ordering/grouping
 * and the clean-preview path grouping.
 */

import type { PreviewFile } from "$lib/ipc/types";

/** Max runs kept in the actions store (oldest dropped). */
export const MAX_RUNS = 200;

/** Max output lines kept per run (most recent win; dropped count reported). */
export const RUN_LINE_CAP = 5000;

/** A clean of more than this many paths asks for type-to-confirm. */
export const CLEAN_CONFIRM_THRESHOLD = 10;

/** Clean dialog's type-to-confirm keyword. */
export const CLEAN_CONFIRM_WORD = "clean";

// ---------------------------------------------------------------------------
// Run status
// ---------------------------------------------------------------------------

/** Lifecycle of one action run. */
export type RunStatus = "running" | "success" | "failed";

/**
 * Classifies a run: running until the done event, then success on exit
 * code 0 and failed otherwise (including a done run without a code, e.g.
 * a cancelled run — non-zero semantics).
 */
export function classifyRun(done: boolean, exitCode: number | null): RunStatus {
  if (!done) return "running";
  return exitCode === 0 ? "success" : "failed";
}

/** Chip text for a run status: "running" / "exit 0" / "exit 3". */
export function runStatusLabel(status: RunStatus, exitCode: number | null): string {
  if (status === "running") return "running";
  return `exit ${exitCode ?? "?"}`;
}

// ---------------------------------------------------------------------------
// Output lines
// ---------------------------------------------------------------------------

export interface LineTruncation {
  /** The kept lines, in arrival order. */
  lines: string[];
  /** How many earlier lines were dropped to fit the cap (0 when none). */
  dropped: number;
}

/**
 * Caps an output buffer at `cap` lines, keeping the most recent ones (the
 * terminal view auto-scrolls to the bottom, so the tail is what matters).
 * Pure: returns a new array.
 */
export function truncateLines(lines: string[], cap: number): LineTruncation {
  if (lines.length <= cap) return { lines: [...lines], dropped: 0 };
  return {
    lines: lines.slice(lines.length - cap),
    dropped: lines.length - cap,
  };
}

/** Full display lines for a run's output: truncation notice first, then lines. */
export function outputDisplayLines(lines: string[], dropped: number, cap: number): string[] {
  const head =
    dropped > 0 ? [`… ${dropped} earlier lines truncated (kept ${cap})`] : [];
  return [...head, ...lines];
}

// ---------------------------------------------------------------------------
// Run ordering + grouping
// ---------------------------------------------------------------------------

/**
 * Sorts runs newest-first (by start order index; higher = newer). Pure.
 */
export function sortRunsNewestFirst<T extends { order: number }>(runs: T[]): T[] {
  return [...runs].sort((a, b) => b.order - a.order);
}

/** Runs of one repository, newest first. Pure. */
export function runsForRepo<T extends { repo_id: string; order: number }>(
  runs: T[],
  repoId: string,
): T[] {
  return sortRunsNewestFirst(runs.filter((run) => run.repo_id === repoId));
}

// ---------------------------------------------------------------------------
// Clean preview grouping (CleanDialog)
// ---------------------------------------------------------------------------

/** One directory group of the clean preview file list. */
export interface PathGroup {
  /** Directory prefix ("" = repo root); display "(root)" for "". */
  dir: string;
  files: PreviewFile[];
}

/** Directory prefix of a preview path ("" = top level); "/" or "\\" aware. */
export function dirOf(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return slash === -1 ? "" : path.slice(0, slash);
}

/**
 * Groups clean-preview files by directory. Root files come first, other
 * directories alphabetically; within a group the original order is kept.
 * Pure.
 */
export function groupByDir(files: PreviewFile[]): PathGroup[] {
  const groups = new Map<string, PreviewFile[]>();
  for (const file of files) {
    const dir = dirOf(file.path);
    const bucket = groups.get(dir);
    if (bucket) bucket.push(file);
    else groups.set(dir, [file]);
  }
  return [...groups.entries()]
    .sort(([a], [b]) => {
      if (a === "") return -1; // root first
      if (b === "") return 1;
      return a.localeCompare(b);
    })
    .map(([dir, groupFiles]) => ({ dir, files: groupFiles }));
}

/** Display label for a group header. */
export function groupLabel(group: PathGroup): string {
  return group.dir === "" ? "(root)" : `${group.dir}/`;
}

// ---------------------------------------------------------------------------
// Clean confirmation
// ---------------------------------------------------------------------------

/** True when the clean is big enough to require typing the confirm word. */
export function needsTypeConfirm(selectedCount: number): boolean {
  return selectedCount > CLEAN_CONFIRM_THRESHOLD;
}

/** Summary line under the file list: "N of M paths selected". */
export function cleanSelectionSummary(selected: number, total: number): string {
  return `${selected} of ${total} path${total === 1 ? "" : "s"} selected`;
}
