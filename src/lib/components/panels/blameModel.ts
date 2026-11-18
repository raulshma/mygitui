/**
 * Pure logic for BlameView's age heat (M12, lane C): maps each authoring
 * commit's signature time to a 0..1 "freshness" ratio (1 = newest commit in
 * the blame result) and renders it as a background tint — newest rows get
 * the strongest accent, the oldest fade to nothing.
 *
 * Computed ONCE per blame result (per unique sha), never per rendered row;
 * the color strings are static `color-mix` derivations of the theme's
 * `--primary`, so dark/light themes adapt for free. No runes, no DOM.
 */

import type { BlameQuery } from "$lib/ipc/types";

/** Blame age scale: accent share at ratio 1 (newest). */
export const AGE_HEAT_MAX_PCT = 28;

/** Minimum input for the ratio map (structural subset of BlameLine). */
export interface ShaTime {
  sha: string;
  signature: { time: number };
}

/**
 * Per-sha freshness ratio in [0, 1] — 1 for the newest commit, 0 for the
 * oldest. When every commit shares one timestamp all rows count as newest
 * (ratio 1) rather than dividing by zero. Later duplicates of a sha are
 * ignored (first wins).
 */
export function ageRatios(lines: readonly ShaTime[]): Map<string, number> {
  const times = new Map<string, number>();
  for (const line of lines) {
    if (!times.has(line.sha)) times.set(line.sha, line.signature.time);
  }
  if (times.size === 0) return times;
  let min = Infinity;
  let max = -Infinity;
  for (const t of times.values()) {
    if (t < min) min = t;
    if (t > max) max = t;
  }
  const ratios = new Map<string, number>();
  if (max === min) {
    for (const [sha] of times) ratios.set(sha, 1);
    return ratios;
  }
  for (const [sha, t] of times) {
    ratios.set(sha, (t - min) / (max - min));
  }
  return ratios;
}

/**
 * Background for a freshness ratio: the newest lines carry the strongest
 * primary accent, older ones fade to transparent. Ratios outside [0, 1]
 * clamp.
 */
export function ageBackground(ratio: number): string {
  const clamped = Math.min(1, Math.max(0, ratio));
  const pct = Math.round(clamped * AGE_HEAT_MAX_PCT);
  if (pct <= 0) return "transparent";
  return `color-mix(in srgb, var(--primary) ${pct}%, transparent)`;
}

// ---------------------------------------------------------------------------
// Extended blame query (repo_blame's optional BlameQuery)
// ---------------------------------------------------------------------------

/** FE-side blame option inputs (BlameView toolbar toggles + oldest bound). */
export interface BlameQueryInputs {
  /** Revision to blame at (`None`/empty = HEAD; rides the query's `from`). */
  from?: string | null;
  /** Oldest bound (`git blame <oldest>..<from>`); trimmed, empty = none. */
  oldest?: string | null;
  ignoreWhitespace?: boolean;
  firstParent?: boolean;
  ignoreRevs?: boolean;
}

/**
 * Builds the backend `BlameQuery` from the toolbar inputs, or `null` when
 * every option is at its default — a null query keeps the IPC request to
 * the legacy shape (the arg stays off the wire) instead of sending a
 * structurally-identical no-op object.
 */
export function buildBlameQuery(
  inputs: BlameQueryInputs,
): BlameQuery | null {
  const ignoreWhitespace = inputs.ignoreWhitespace ?? false;
  const firstParent = inputs.firstParent ?? false;
  const ignoreRevs = inputs.ignoreRevs ?? false;
  const oldest = inputs.oldest?.trim() || null;
  if (!ignoreWhitespace && !firstParent && !ignoreRevs && oldest === null) {
    return null;
  }
  return {
    from: inputs.from ?? null,
    oldest,
    ignore_whitespace: ignoreWhitespace,
    first_parent: firstParent,
    ignore_revs: ignoreRevs,
  };
}
