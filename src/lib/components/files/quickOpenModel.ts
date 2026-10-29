/**
 * Pure quick-open fuzzy model — no runes, no DOM, no IPC (unit-testable,
 * perf-gated like the other model modules).
 *
 * The index is a prebuilt lowercase copy of the backend's file index
 * (built once per fetch, never per keystroke); scoring is a single-pass
 * subsequence matcher with path-boundary and basename bonuses. 50k paths
 * score in well under a frame (gated in the tests).
 */

import type { FileIndexEntry } from "$lib/ipc/types";

/** Prebuilt search index: original + lowercase copies, one array each. */
export interface PathIndex {
  /** Repository-relative paths (backend order: natural-sorted). */
  paths: string[];
  /** Same length, lowercased — the scoring input. */
  lower: string[];
}

/** Build the lowercase scoring index over the backend walk result. */
export function buildIndex(entries: FileIndexEntry[]): PathIndex {
  const paths: string[] = [];
  const lower: string[] = [];
  for (const entry of entries) {
    if (entry.is_dir) continue; // quick-open finds files
    paths.push(entry.path);
    lower.push(entry.path.toLowerCase());
  }
  return { paths, lower };
}

/** Score module state: kept tiny and allocation-free per candidate. */
const BOUNDARY_CHARS = "/._- ";

/**
 * Subsequence score of `query` (LOWERCASED) within `lowerPath`, or null
 * when the query isn't a subsequence. Higher = better; bonuses:
 * consecutive streaks, path-boundary matches (after `/._- `), basename
 * matches, and shorter paths win ties.
 */
export function fuzzyScore(query: string, lowerPath: string): number | null {
  if (query === "") return 0;
  let qi = 0;
  let score = 0;
  let streak = 0;
  let boundary = true; // path start is a boundary
  const basenameStart = lowerPath.lastIndexOf("/") + 1;
  for (let pi = 0; pi < lowerPath.length && qi < query.length; pi++) {
    if (lowerPath[pi] === query[qi]) {
      streak++;
      score += 6 * streak;
      if (boundary) score += 10;
      if (pi >= basenameStart) score += 12; // basename hit
      if (pi === basenameStart) score += 6; // basename leading hit
      qi++;
      boundary = false;
    } else {
      streak = 0;
      if (BOUNDARY_CHARS.includes(lowerPath[pi]!)) boundary = true;
    }
  }
  if (qi < query.length) return null;
  // Shorter paths (closer to the query) rank slightly higher.
  score -= Math.floor(lowerPath.length / 8);
  return score;
}

/** One scored result. */
export interface QuickOpenHit {
  path: string;
  score: number;
}

/**
 * Search the index: every candidate scored, best-first, capped at
 * `limit`. Empty query returns the natural (backend) order.
 */
export function searchPaths(
  index: PathIndex,
  query: string,
  limit = 100,
): QuickOpenHit[] {
  const needle = query.toLowerCase();
  if (needle === "") {
    const hits: QuickOpenHit[] = [];
    for (let i = 0; i < index.paths.length && i < limit; i++) {
      hits.push({ path: index.paths[i]!, score: 0 });
    }
    return hits;
  }
  const scored: QuickOpenHit[] = [];
  for (let i = 0; i < index.paths.length; i++) {
    const score = fuzzyScore(needle, index.lower[i]!);
    if (score !== null) scored.push({ path: index.paths[i]!, score });
  }
  scored.sort(
    (a, b) => b.score - a.score || a.path.length - b.path.length || (a.path < b.path ? -1 : 1),
  );
  return scored.length > limit ? scored.slice(0, limit) : scored;
}

/** Highlight ranges (match character indexes in the ORIGINAL path) for a
 * hit, so the UI can underline the matched characters without re-matching
 * against the original-case path. */
export function matchRanges(
  query: string,
  path: string,
): Array<[number, number]> {
  const needle = query.toLowerCase();
  const lowerPath = path.toLowerCase();
  const ranges: Array<[number, number]> = [];
  let qi = 0;
  for (let pi = 0; pi < lowerPath.length && qi < needle.length; pi++) {
    if (lowerPath[pi] === needle[qi]) {
      ranges.push([pi, pi + 1]);
      qi++;
    }
  }
  return ranges;
}

/** One render token of a highlighted path. */
export interface PathToken {
  text: string;
  marked: boolean;
  separator: boolean;
}

/**
 * Path → render tokens with per-character match marks (the UI maps these
 * to `<mark>` spans; separators are flagged for dimming). Visible rows
 * only — a dozen paths × path length per keystroke.
 */
export function pathTokens(path: string, query: string): PathToken[] {
  const ranges = matchRanges(query, path);
  const marked = new Set(ranges.map(([start]) => start));
  const tokens: PathToken[] = [];
  let offset = 0;
  const segments = path.split("/");
  for (let si = 0; si < segments.length; si++) {
    if (si > 0) {
      tokens.push({ text: "/", marked: false, separator: true });
      offset++;
    }
    const segment = segments[si]!;
    for (const ch of segment) {
      tokens.push({ text: ch, marked: marked.has(offset), separator: false });
      offset++;
    }
    offset++; // separator consumed above for the NEXT segment
  }
  return tokens;
}
