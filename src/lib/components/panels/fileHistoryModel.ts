/**
 * Pure selection logic for FileHistoryView's commit list: shift-click
 * ranges and the aggregate-diff spec behind them.
 *
 * The file walk is newest-first, so a selected index range lo..hi spans
 * `commits[lo]` (newest) down to `commits[hi]` (oldest). The aggregate
 * diff of a range is the diff between the OLDEST commit's first parent and
 * the NEWEST commit — `git diff oldest^ newest` — which collapses every
 * intermediate change into one view, exactly like showing a push range.
 * No runes, no DOM.
 */

/** Structural subset of CommitInfo the model needs. */
export interface SelectionCommit {
  sha: string;
  parents: string[];
}

/** Inclusive anchor..clicked index range (order-independent). */
export function rangeIndices(anchor: number, clicked: number): number[] {
  const lo = Math.min(anchor, clicked);
  const hi = Math.max(anchor, clicked);
  const out: number[] = [];
  for (let i = lo; i <= hi; i++) out.push(i);
  return out;
}

export interface AggregateSpec {
  /** Real diff endpoints: the oldest selected commit's first parent → the newest. */
  baseSha: string;
  targetSha: string;
  /** Display forms for the detail header (`abc1234^ → def5678`). */
  baseLabel: string;
  targetLabel: string;
}

export type AggregateRange = AggregateSpec | { error: string };

/**
 * Aggregate-diff endpoints for a consecutive selection (lo = newest index,
 * hi = oldest index). A single-commit range degrades to that commit's own
 * parent0 diff. An empty oldest parent (root commit) has no diff base.
 */
export function aggregateRange(
  commits: readonly SelectionCommit[],
  lo: number,
  hi: number,
): AggregateRange {
  const newest = commits[lo];
  const oldest = commits[hi];
  if (!newest || !oldest) return { error: "Selection is out of range" };
  const parent = oldest.parents[0];
  if (!parent) {
    return { error: "Oldest selected commit is a root commit — no diff base" };
  }
  return {
    baseSha: parent,
    targetSha: newest.sha,
    // git notation: `<oldest>^` IS the base commit (its first parent).
    baseLabel: `${oldest.sha.slice(0, 7)}^`,
    targetLabel: newest.sha.slice(0, 7),
  };
}
