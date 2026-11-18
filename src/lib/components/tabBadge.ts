/**
 * Per-tab strip badges (TabStrip's dirty dot + ahead/behind chip), derived
 * from a tab's `RepoStatus` snapshot — no stores, no DOM.
 *
 * The derivations mirror the status bar (StatusBar.svelte): the changed
 * count is unique paths (one path can appear as staged AND worktree rows),
 * and the ahead/behind numbers come straight off the status snapshot the
 * tab store already refreshes on activation and watcher events, so the
 * strip never needs its own fetch or the refs cache.
 */

import type { RepoStatus } from "$lib/ipc/types";

/** The badge inputs for one tab (all zero while `status` is still null). */
export interface TabBadge {
  /** Unique changed paths (staged + worktree rows); 0 = clean. */
  changed: number;
  /** Commits the upstream is missing (to push). */
  ahead: number;
  /** Commits the local branch is missing (to pull). */
  behind: number;
}

/** Derives the strip badge inputs from a tab's status (null-safe). */
export function tabBadge(status: RepoStatus | null): TabBadge {
  if (status === null) return { changed: 0, ahead: 0, behind: 0 };
  return {
    changed: new Set(status.entries.map((entry) => entry.path)).size,
    ahead: status.ahead,
    behind: status.behind,
  };
}

/** Tooltip text for the dirty dot: "N changed". */
export function changedLabel(count: number): string {
  return `${count} changed`;
}

/**
 * Tooltip / accessible name for the ahead/behind chip: only the non-zero
 * halves, "/"-joined ("2 ahead", "1 ahead / 3 behind").
 */
export function aheadBehindLabel(ahead: number, behind: number): string {
  const parts: string[] = [];
  if (ahead > 0) parts.push(`${ahead} ahead`);
  if (behind > 0) parts.push(`${behind} behind`);
  return parts.join(" / ");
}

/**
 * Chip number cap (the status bar's rule): a four-digit count would widen
 * the tab; past 99 the chip reads "99+" and the tooltip carries the truth.
 */
export function badgeCount(count: number): string {
  return count > 99 ? "99+" : String(count);
}

/**
 * Extra fragment for the tab button's accessible name ("repo — /root, 2
 * changed, 1 ahead / 3 behind"): the dot/chip are aria-named images, but
 * the button's own aria-label overrides its content, so the same facts
 * ride here to stay announced with the repository name.
 */
export function tabAriaSuffix(badge: TabBadge): string {
  const parts: string[] = [];
  if (badge.changed > 0) parts.push(changedLabel(badge.changed));
  const ab = aheadBehindLabel(badge.ahead, badge.behind);
  if (ab !== "") parts.push(ab);
  return parts.length === 0 ? "" : `, ${parts.join(", ")}`;
}
