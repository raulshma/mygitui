/**
 * Tab algebra for the center editor's multi-diff model (pure: no runes,
 * no DOM). The center pane holds ONE list of open working-copy diffs plus
 * the active path — same shape as the file workspace's tab strip, so the
 * interactions match too: opening an already-open path focuses its tab,
 * closing activates the neighbor, and a diff stream request covers every
 * open path with the visible one filtered out per tab.
 */

import type { FileDiff, StatusEntry } from "$lib/ipc/types";
import type { DiffScope } from "./statusModel";

/** One open working-copy diff: the entry plus the comparison it shows. */
export interface DiffTab {
  entry: StatusEntry;
  scope: DiffScope;
}

/** The tab whose path is active, falling back to the first tab. */
export function activeDiffTab(
  tabs: readonly DiffTab[],
  activePath: string,
): DiffTab | null {
  return (
    tabs.find((t) => t.entry.path === activePath) ?? tabs[0] ?? null
  );
}

/**
 * Adds `entry` as a new tab, or refreshes it in place when a tab for the
 * path is already open (path is the tab identity — a refresh updates the
 * entry kinds and the shown scope without moving the tab).
 */
export function upsertDiffTab(
  tabs: readonly DiffTab[],
  entry: StatusEntry,
  scope: DiffScope,
): DiffTab[] {
  const at = tabs.findIndex((t) => t.entry.path === entry.path);
  if (at === -1) return [...tabs, { entry, scope }];
  const next = [...tabs];
  next[at] = { entry, scope };
  return next;
}

/** Removes the tab for `path` (no-op when absent). */
export function withoutDiffTab(
  tabs: readonly DiffTab[],
  path: string,
): DiffTab[] {
  return tabs.filter((t) => t.entry.path !== path);
}

/**
 * Neighbor activation after a close (same rule as the file workspace's
 * `closeWorkdirFile`): removing the ACTIVE tab activates its neighbor at
 * the same index (clamped to the new last), removing any other tab keeps
 * the active one, and an empty remainder has none. `removedPath` must be
 * present in `tabs`.
 */
export function neighborDiffPath(
  tabs: readonly DiffTab[],
  activePath: string,
  removedPath: string,
): string | null {
  const at = tabs.findIndex((t) => t.entry.path === removedPath);
  const remaining = tabs.filter((t) => t.entry.path !== removedPath);
  if (remaining.length === 0) return null;
  if (activePath !== removedPath) return activePath;
  return (
    remaining[Math.min(Math.max(at, 0), remaining.length - 1)]?.entry.path ??
    remaining[0]!.entry.path
  );
}

/**
 * The pages of a multi-path diff stream keep every OPEN path — one stream
 * serves all tabs, each tab filters its own files out of the result
 * (`fileForDiffTab`). Rename-tolerant like every entry lookup: a page file
 * matches by its post-rename path or its old path.
 */
export function diffPageForOpenPaths(
  page: readonly FileDiff[],
  openPaths: readonly string[],
): FileDiff[] {
  return page.filter((f) =>
    openPaths.some((p) => f.path === p || f.old_path === p),
  );
}

/** The streamed files shown by one tab (its own FileDiff list only). */
export function fileForDiffTab(
  files: readonly FileDiff[],
  path: string,
): FileDiff[] {
  return files.filter((f) => f.path === path || f.old_path === path);
}
