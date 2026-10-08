/**
 * Safety/undo store (Svelte 5 runes) — M3 checkpoint state per repo.
 *
 * Caches the `checkpoints` list for the UndoPanel (loaded on mount and
 * after every action — no polling), tracks loading/error, and remembers the
 * last guard checkpoint (`guard_checkpoint`) a dialog created so success
 * toasts and logs can quote its id.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client` (mocked with
 * `vi.mock` in tests).
 */

import { checkpoints, guardCheckpoint } from "$lib/ipc/client";
import type { CheckpointInfo, RepoId } from "$lib/ipc/types";
import { sortNewestFirst } from "$lib/components/safety/safetyModel";
import { untrack } from "svelte";

/** Per-repo undo state (repos with no data yet have no entry). */
export interface SafetyState {
  /** Checkpoints, newest first. */
  checkpoints: CheckpointInfo[];
  loading: boolean;
  error: string | null;
  /** Checkpoint created by the most recent `guardNow` call, if any. */
  lastGuard: CheckpointInfo | null;
}

const EMPTY: SafetyState = {
  checkpoints: [],
  loading: false,
  error: null,
  lastGuard: null,
};

export class SafetyStore {
  /** Undo state keyed by repo id (fresh references per update). */
  states: Record<RepoId, SafetyState> = $state({});

  /** Read-only view of one repo's state (falls back to an empty shape). */
  stateFor(repoId: RepoId): SafetyState {
    return this.states[repoId] ?? EMPTY;
  }

  /**
   * (Re)loads the checkpoints list, newest first. Failures land in
   * `error` — never thrown.
   */
  async load(repoId: RepoId): Promise<void> {
    this.#patch(repoId, { loading: true, error: null });
    try {
      const list = await checkpoints(repoId);
      this.#patch(repoId, {
        checkpoints: sortNewestFirst(list),
        loading: false,
      });
    } catch (err) {
      this.#patch(repoId, {
        loading: false,
        error: err instanceof Error ? err.message : String(err),
      });
    }
  }

  /**
   * Snapshots the current state as a guard checkpoint (`guard_checkpoint`),
   * remembers it as `lastGuard` and returns it. Throws on failure (the
   * caller decides whether the guarded op may proceed).
   */
  async guard(repoId: RepoId, reason: string): Promise<CheckpointInfo> {
    const cp = await guardCheckpoint(repoId, reason);
    this.#patch(repoId, { lastGuard: cp });
    return cp;
  }

  /** Clears `lastGuard` (new dangerous op starting). */
  clearGuard(repoId: RepoId): void {
    this.#patch(repoId, { lastGuard: null });
  }

  /** Drops a repo's state entirely (repo closed / test teardown). */
  drop(repoId: RepoId): void {
    if (!(repoId in this.states)) return;
    const next = { ...this.states };
    delete next[repoId];
    this.states = next;
  }

  #patch(repoId: RepoId, patch: Partial<SafetyState>): void {
    // Read the record untracked: load() runs inside component effects, and a
    // tracked read here would make each patch write re-trigger the effect
    // that called it — an infinite checkpoints IPC loop.
    const prev = untrack(() => this.states);
    this.states = {
      ...prev,
      [repoId]: { ...EMPTY, ...prev[repoId], ...patch },
    };
  }
}

/** The application-wide safety store. */
export const safetyStore = new SafetyStore();

// Standalone function API over the singleton (what components import). --

export function undoState(repoId: RepoId): SafetyState {
  return safetyStore.stateFor(repoId);
}

export function loadUndo(repoId: RepoId): Promise<void> {
  return safetyStore.load(repoId);
}

export function guardNow(repoId: RepoId, reason: string): Promise<CheckpointInfo> {
  return safetyStore.guard(repoId, reason);
}
