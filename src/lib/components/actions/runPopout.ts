/**
 * Action-run popout handoff.
 *
 * Popout windows are separate JS contexts, so their actions store starts
 * empty and cannot replay output that already streamed. The main window
 * stashes the current run (output included) in localStorage right before
 * opening the window; the popout hydrates from that snapshot and then
 * follows live `action-output` events like any other window.
 *
 * Snapshots are best-effort (storage may be unavailable) and pruned to the
 * newest few runs so a long session cannot grow the store unboundedly.
 */

import type { ActionRun } from "$lib/stores/actions.svelte";

/** localStorage key prefix: `<prefix><run_id>` → serialized {@link ActionRun}. */
export const ACTION_RUN_POPOUT_PREFIX = "mygitui.action-run-popout.";

/** Snapshots kept before the oldest are pruned. */
const MAX_STASHED_RUNS = 20;

function defaultStorage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

function isRun(value: unknown): value is ActionRun {
  if (typeof value !== "object" || value === null) return false;
  const run = value as Record<string, unknown>;
  return (
    typeof run.run_id === "string" &&
    typeof run.repo_id === "string" &&
    typeof run.name === "string" &&
    (run.command === undefined || typeof run.command === "string") &&
    Array.isArray(run.lines) &&
    run.lines.every((line) => typeof line === "string") &&
    typeof run.dropped === "number" &&
    typeof run.done === "boolean" &&
    (run.exit_code === null || typeof run.exit_code === "number") &&
    typeof run.order === "number" &&
    (run.status === "running" ||
      run.status === "success" ||
      run.status === "failed")
  );
}

function prune(store: Storage): void {
  const keys: string[] = [];
  for (let i = 0; i < store.length; i += 1) {
    const key = store.key(i);
    if (key?.startsWith(ACTION_RUN_POPOUT_PREFIX)) keys.push(key);
  }
  for (const key of keys.slice(0, Math.max(0, keys.length - MAX_STASHED_RUNS))) {
    store.removeItem(key);
  }
}

/** Saves a snapshot of `run` for the popout window about to open. */
export function stashActionRun(run: ActionRun): void {
  const store = defaultStorage();
  if (!store) return;
  try {
    store.setItem(
      `${ACTION_RUN_POPOUT_PREFIX}${run.run_id}`,
      JSON.stringify(run),
    );
    prune(store);
  } catch {
    // Storage full/unavailable: the popout still follows live events.
  }
}

/** Reads a previously stashed snapshot; `null` when absent or malformed. */
export function readStashedActionRun(runId: string): ActionRun | null {
  const store = defaultStorage();
  if (!store) return null;
  try {
    const raw = store.getItem(`${ACTION_RUN_POPOUT_PREFIX}${runId}`);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    return isRun(parsed) ? parsed : null;
  } catch {
    return null;
  }
}
