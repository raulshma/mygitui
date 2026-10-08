/**
 * Pure logic for the interactive rebase planner (M3 FE lane E2).
 *
 * Everything here is plain, synchronous and side-effect free (no runes, no
 * IPC) so it can be unit tested directly — the reactive monitor lives in
 * `./rebaseStore.svelte` and the UI in `./RebasePlanner.svelte`.
 *
 * Model:
 *   - The flat history (`HistoryStore.flat.commits`, also `streamLog` pages)
 *     is newest-first. A rebase todo is oldest-first (top row is applied
 *     first), so `buildPlan` reverses the HEAD..base slice.
 *   - A {@link PlanRow} is one editable todo line in the planner UI. The
 *     wire shape sent to the backend is the {@link RebaseStep} triple
 *     `{sha, action, new_message}`; `toRebaseSteps` is the exact
 *     serialization (only `reword` carries `new_message`).
 *   - `orderForCherryPick` turns a (possibly shift-range) selection into the
 *     order cherry-pick must apply commits in: oldest first.
 */

import type { CommitInfo, RebaseStep } from "$lib/ipc/types";

// ---------------------------------------------------------------------------
// Actions + plan rows
// ---------------------------------------------------------------------------

/** Actions understood by the backend rebase todo (git-rebase semantics). */
export type RebaseAction =
  | "pick"
  | "squash"
  | "fixup"
  | "drop"
  | "edit"
  | "reword";

/** All actions, in select-display order. */
export const REBASE_ACTIONS: readonly RebaseAction[] = [
  "pick",
  "squash",
  "fixup",
  "drop",
  "edit",
  "reword",
];

/**
 * One editable row of the planner todo (oldest-first display order).
 * `summary` is display-only; `message` is the reword editor buffer
 * (prefilled with the original commit message, honored only for `reword`).
 */
export interface PlanRow {
  sha: string;
  summary: string;
  action: RebaseAction;
  message: string;
}

/** Structural subset `validatePlan` needs (also satisfied by RebaseStep). */
export interface PlanStepLike {
  sha: string;
  action: string;
  new_message: string | null;
}

// ---------------------------------------------------------------------------
// Range extraction + plan construction
// ---------------------------------------------------------------------------

/**
 * Slices the newest-first commit list down to `HEAD..baseSha` (exclusive of
 * `baseSha`). `baseSha === null` means every commit reachable from HEAD. If
 * `baseSha` is not in the list (not loaded / not an ancestor) the whole list
 * is returned — the caller asked for a rebase down to that commit and the
 * backend will refuse invalid plans anyway.
 */
export function rangeForRebase(
  commits: CommitInfo[],
  baseSha: string | null,
): CommitInfo[] {
  if (baseSha === null) return commits.slice();
  const idx = commits.findIndex((c) => c.sha === baseSha);
  if (idx < 0) return commits.slice();
  return commits.slice(0, idx);
}

/**
 * Streaming variant: given the commits accumulated so far (HEAD-first) and
 * the stop sha, returns the truncated `HEAD..baseSha` slice — or `null`
 * when `baseSha` has not been seen yet (keep accumulating pages).
 */
export function cutAtBase(
  commits: CommitInfo[],
  baseSha: string,
): CommitInfo[] | null {
  const idx = commits.findIndex((c) => c.sha === baseSha);
  if (idx < 0) return null;
  return commits.slice(0, idx);
}

/**
 * Builds the initial planner todo from a HEAD-first commit range: rows in
 * oldest-first (todo) order, action `pick`, reword buffer prefilled with the
 * full original message.
 */
export function buildPlan(commits: CommitInfo[]): PlanRow[] {
  return commits
    .slice()
    .reverse()
    .map((commit) => ({
      sha: commit.sha,
      summary: commit.summary,
      action: "pick" as const,
      message: commit.message,
    }));
}

/**
 * Serializes planner rows into the exact backend `RebaseStep` shape.
 * Only `reword` carries `new_message` (raw editor buffer when it has
 * non-whitespace content); every other action serializes `new_message: null`.
 */
export function toRebaseSteps(rows: PlanRow[]): RebaseStep[] {
  return rows.map((row) => ({
    sha: row.sha,
    action: row.action,
    new_message:
      row.action === "reword" && row.message.trim() !== "" ? row.message : null,
  }));
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/**
 * Returns the first validation problem with the plan, or `null` when the
 * plan is runnable. Rules: at least one row, no unknown actions, squash /
 * fixup cannot be the first step (nothing to squash into), reword needs a
 * non-blank message.
 */
export function validatePlan(steps: PlanStepLike[]): string | null {
  if (steps.length === 0) {
    return "The plan is empty — there is nothing to rebase.";
  }
  for (let i = 0; i < steps.length; i++) {
    const step = steps[i] as PlanStepLike;
    const short = step.sha.slice(0, 7);
    if (!REBASE_ACTIONS.includes(step.action as RebaseAction)) {
      return `Unknown action "${step.action}" for ${short}.`;
    }
    if ((step.action === "squash" || step.action === "fixup") && i === 0) {
      return `Squash/fixup cannot be the first step (${short}) — nothing to squash into.`;
    }
    if (step.action === "reword" && (step.new_message ?? "").trim() === "") {
      return `Reword needs a message for ${short}.`;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Reordering (drag-and-drop + keyboard buttons)
// ---------------------------------------------------------------------------

/**
 * Moves the row at `from` to index `to` (the moved row's new index), returning
 * a new array; the input is never mutated. Out-of-range indices and
 * `from === to` return the input array unchanged.
 */
export function moveRow<T>(rows: T[], from: number, to: number): T[] {
  if (from === to) return rows;
  if (from < 0 || from >= rows.length) return rows;
  if (to < 0 || to >= rows.length) return rows;
  const next = rows.slice();
  const [row] = next.splice(from, 1);
  next.splice(to, 0, row as T);
  return next;
}

// ---------------------------------------------------------------------------
// Cherry-pick ordering
// ---------------------------------------------------------------------------

/**
 * Orders selected shas for `cherry_pick`: oldest first (the flat commit list
 * is newest-first, so descending flat index), deduplicated, stable for equal
 * keys. Shas that are not in `commits` are dropped (their order is
 * undefinable and the backend would reject them).
 */
export function orderForCherryPick(
  shas: string[],
  commits: CommitInfo[],
): string[] {
  const indexOf = new Map<string, number>();
  commits.forEach((commit, i) => {
    if (!indexOf.has(commit.sha)) indexOf.set(commit.sha, i);
  });
  const unique = [...new Set(shas)];
  const known = unique.filter((sha) => indexOf.has(sha));
  known.sort((a, b) => (indexOf.get(b) as number) - (indexOf.get(a) as number));
  return known;
}
