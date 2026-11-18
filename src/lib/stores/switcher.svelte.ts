/**
 * Quick-switcher store (cmd/ctrl+K, B4 lane).
 *
 * Lives in a `.svelte.ts` module because the open/closed flag is a `$state`
 * rune. Also hosts the switcher's pure pieces so they are unit-testable
 * without a DOM: item building (open tabs first, then favorites, then
 * recents — open roots excluded), the tiny fuzzy scorer (subsequence +
 * consecutive/word-start bonuses) and activation (openTab for recents and
 * favorites, setActive for tabs).
 */

import {
  openTab,
  recentRepos,
  setActive,
  tabStore,
  type Tab,
} from "$lib/stores/tabs.svelte";
import type { RecentRepo } from "$lib/entry/recentRepos";
import { favoriteRepos } from "$lib/stores/favoriteRepos.svelte";

/** One switcher row: an open tab, a favorite, or a recent repository. */
export interface SwitcherItem {
  /** Unique row id (`tab:<id>` / `favorite:<path>` / `recent:<path>`). */
  id: string;
  kind: "tab" | "favorite" | "recent";
  /** Repo name (bold line). */
  label: string;
  /** Full path (dim line) — also the favorite-set key. */
  sub: string;
  /** Tab id (`kind === "tab"`) or repo path (`kind !== "tab"`). */
  target: string;
}

/** A fuzzy-scored item ready to render (positions index `label + " " + sub`). */
export interface RankedItem extends SwitcherItem {
  score: number;
  positions: number[];
}

let open = $state(false);

export function isSwitcherOpen(): boolean {
  return open;
}

export function openSwitcher(): void {
  open = true;
}

export function closeSwitcher(): void {
  open = false;
}

export function toggleSwitcher(): void {
  open = !open;
}

/**
 * Test seam: current store state plus the items the switcher would show
 * right now (snapshot of open tabs + favorites + recents).
 */
export function switcherSnapshot(): { open: boolean; items: SwitcherItem[] } {
  return {
    open,
    items: buildSwitcherItems(recentRepos.list(), tabStore.tabs, favoriteRepos.favorites),
  };
}

/** Normalizes a path for open-vs-recent deduping (separators + case). */
export function normalizeRepoPath(path: string): string {
  return path
    .replace(/[\\/]+/g, "/")
    .toLowerCase()
    .replace(/\/+$/, "");
}

/** Final path segment (`C:\x\repo` and `repo/` both → `repo`). */
export function repoBasename(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1]! : path;
}

/**
 * Builds the switcher list: open tabs first (switching is instant), then
 * favorite repositories, then recents — favorites and recents skip roots
 * that already have an open tab (path-normalized compare), and a favorite
 * that is also a recent appears only once, in the favorites block.
 */
export function buildSwitcherItems(
  recents: readonly RecentRepo[],
  tabs: readonly Tab[],
  favorites: readonly string[] = [],
): SwitcherItem[] {
  const items: SwitcherItem[] = tabs.map((tab) => ({
    id: `tab:${tab.id}`,
    kind: "tab",
    label: tab.name,
    sub: tab.root,
    target: tab.id,
  }));
  const openRoots = new Set(tabs.map((tab) => normalizeRepoPath(tab.root)));
  for (const favorite of favorites) {
    if (openRoots.has(normalizeRepoPath(favorite))) continue;
    openRoots.add(normalizeRepoPath(favorite));
    items.push({
      id: `favorite:${favorite}`,
      kind: "favorite",
      label: repoBasename(favorite),
      sub: favorite,
      target: favorite,
    });
  }
  for (const recent of recents) {
    if (openRoots.has(normalizeRepoPath(recent.path))) continue;
    items.push({
      id: `recent:${recent.path}`,
      kind: "recent",
      label: repoBasename(recent.path),
      sub: recent.path,
      target: recent.path,
    });
  }
  return items;
}

/**
 * Flips `item`'s repository in the favorite set (the row's star toggle).
 * Open tabs favorite their root; favorites/recents favorite their path.
 * Returns the new favorite state (true = now favorited).
 */
export function toggleItemFavorite(item: SwitcherItem): boolean {
  return favoriteRepos.toggle(item.sub);
}

/** True when `haystack[i]` starts a "word" (separator boundary or camelCase). */
function isWordStart(haystack: string, i: number): boolean {
  if (i === 0) return true;
  const prev = haystack[i - 1]!;
  if (/[\s/\\._\-]/.test(prev)) return true;
  const cur = haystack[i]!;
  return prev === prev.toLowerCase() && cur !== cur.toLowerCase();
}

/**
 * Tiny fuzzy scorer: case-insensitive greedy subsequence match with bonuses
 * for consecutive characters and word starts, and a small gap penalty so
 * tight matches outrank scattered ones. Returns `null` when `needle` is not
 * a subsequence of `haystack`. `positions` are indexes into `haystack`.
 */
export function fuzzyMatch(
  needle: string,
  haystack: string,
): { score: number; positions: number[] } | null {
  if (needle.length === 0) return { score: 0, positions: [] };
  const n = needle.toLowerCase();
  const h = haystack.toLowerCase();
  let score = 0;
  let hi = 0;
  let prev = -2;
  const positions: number[] = [];
  for (let ni = 0; ni < n.length; ni++) {
    const found = h.indexOf(n[ni]!, hi);
    if (found === -1) return null;
    positions.push(found);
    score += 1; // base: one point per matched character
    if (found === prev + 1) score += 3; // consecutive run
    if (isWordStart(haystack, found)) score += 2; // word-start hit
    score -= Math.min(found - hi, 8) * 0.25; // gap penalty (capped)
    hi = found + 1;
    prev = found;
  }
  return { score, positions };
}

/**
 * Ranks items against `query`: scores `label + " " + sub` (so both the repo
 * name and path fragments match), drops non-matches, sorts by score
 * descending. The sort is stable, so ties keep build order (tabs first,
 * then recents by recency); an empty query keeps every item (score 0).
 */
export function rankSwitcherItems(
  items: readonly SwitcherItem[],
  query: string,
): RankedItem[] {
  const ranked: RankedItem[] = [];
  for (const item of items) {
    const match = fuzzyMatch(query, `${item.label} ${item.sub}`);
    if (match === null) continue;
    ranked.push({ ...item, score: match.score, positions: match.positions });
  }
  ranked.sort((a, b) => b.score - a.score);
  return ranked;
}

/**
 * Activates an item: closes the switcher, then focuses the tab (tabs) or
 * opens the repository (recents, via the tab store — failures propagate to
 * the caller, which toasts).
 */
export async function activateSwitcherItem(item: SwitcherItem): Promise<void> {
  closeSwitcher();
  if (item.kind === "tab") {
    setActive(item.target);
    return;
  }
  await openTab(item.target);
}
