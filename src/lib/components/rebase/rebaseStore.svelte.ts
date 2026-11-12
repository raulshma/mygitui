/**
 * Rebase monitor (Svelte 5 runes) — watches the backend's interactive-rebase
 * state for one repository and surfaces it to the planner UI (M3 FE lane E2).
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * The {@link rebaseMonitor} singleton is shared by every surface that needs
 * rebase status today (the `RebasePlanner` dialog) and later (E4/M3
 * integration); tests build isolated instances via `new RebaseMonitor()`.
 *
 * Lifecycle: `start(repoId)` polls `rebase_state` immediately and then every
 * {@link REBASE_POLL_MS} ms until a poll observes `active: false` (the
 * "poll until done" contract), at which point the timer stops itself — the
 * final `RebaseState` stays visible for a completion summary. `start` is
 * idempotent per repo (calling it again mid-run never resets the timer) and
 * switching repos resets everything. `stop()` clears the timer and all state.
 *
 * Pause/conflict detection: while the backend reports an active, paused-for-
 * edit rebase the monitor also polls `conflicts()`; the first tick that sees
 * conflicts toasts once ("resolve in the status panel, then Continue") and
 * clears the flag when they go away. `continueRebase()` / `abort()` delegate
 * to the client wrappers and apply/keep the resulting state.
 *
 * All backend access goes through `$lib/ipc/client` (mocked with `vi.mock`
 * in tests). Events surface as toasts (`$lib/toast`); the store never throws.
 */

import { conflicts, rebaseAbort, rebaseContinue, rebaseState } from "$lib/ipc/client";
import type { ConflictFile, RebaseState, RebaseStep } from "$lib/ipc/types";
import { toast } from "$lib/toast";
import { describeError } from "$lib/errors";

/** Poll interval for `rebase_state` (contract: every 500ms). */
export const REBASE_POLL_MS = 500;

/**
 * After this much continuous polling the interval backs off: a rebase only
 * advances through `continueRebase()` (which refreshes state immediately)
 * or an external CLI continue, and the latter needs seconds, not
 * milliseconds. The fast lane covers the typical short rebase; long
 * conflict-resolution sessions stop paying 2 git2 rebase-state reads/s.
 */
const REBASE_BACKOFF_AFTER_MS = 15_000;
const REBASE_BACKOFF_POLL_MS = 2_000;

/**
 * Conflicts are refetched at most this often while paused (and immediately
 * on each pause transition): the list changes as the user resolves files.
 */
const CONFLICTS_REFRESH_MS = 1_000;

function errorMessage(err: unknown): string {
  return describeError(err);
}

export class RebaseMonitor {
  /** Latest polled backend rebase state (`null` before the first poll). */
  state: RebaseState | null = $state(null);
  /** Conflicted files from the latest paused-state poll. */
  conflicts: ConflictFile[] = $state([]);
  /** Whether the latest poll saw an active rebase. */
  active = $state(false);
  /** Whether the polling timer is running. */
  polling = $state(false);
  /** Last poll/continue failure message (`null` when healthy). */
  error: string | null = $state(null);

  #repoId: string | null = null;
  #timer: ReturnType<typeof setInterval> | null = null;
  /** Current poll cadence (re-armed when the backoff kicks in). */
  #timerInterval = 0;
  /** `Date.now()` of monitor start (backoff eligibility). */
  #startedAt = 0;
  /** Last conflicts() fetch time (paused-refresh cadence). */
  #conflictsAt = 0;
  /** Whether the previous poll saw a paused rebase (pause transitions). */
  #wasPaused = false;
  /** Bumped by `#hardStop` — in-flight async work compares and bails. */
  #token = 0;
  /** Whether any poll of this run has seen an active rebase. */
  #engaged = false;
  /** Toast-once flag for the conflicts appearance transition. */
  #hadConflicts = false;
  /** Overlap guard: skip a tick while the previous one is in flight. */
  #inFlight = false;

  // -- derived ----------------------------------------------------------------

  /** Repo this monitor is bound to, or `null` before `start`/after `stop`. */
  get repoId(): string | null {
    return this.#repoId;
  }

  /** Number of conflicted files from the latest paused-state poll. */
  get conflictCount(): number {
    return this.conflicts.length;
  }

  /** Backend is stopped mid-rebase and waiting for the user (edit or
   *  conflict pause). Continue is only meaningful while this is true.
   *  Requires `active`: after a natural completion `#finish` keeps the
   *  final `state` visible while clearing `active` — without this gate
   *  the stale pause flags enabled Continue against a rebase that had
   *  already ended (`rebase_continue` then errored backend-side). */
  get paused(): boolean {
    return this.active && (this.pausedForEdit || this.state?.paused_for_conflict === true);
  }

  /** Backend is paused for `edit` (inspect/amend the commit, then continue). */
  get pausedForEdit(): boolean {
    return this.active && this.state?.paused_for_edit === true;
  }

  /** Active rebase with conflicts to resolve — the Continue gate. */
  get inConflict(): boolean {
    return this.active && this.conflictCount > 0;
  }

  /** Total steps in the running plan (`0` when idle). */
  get stepCount(): number {
    return this.state?.plan.length ?? 0;
  }

  /** Index of the step the backend is stopped at (0-based). */
  get current(): number {
    return this.state?.current ?? 0;
  }

  /** How many commits the rebase has rewritten so far. */
  get rewrittenCount(): number {
    return this.state?.rewritten.length ?? 0;
  }

  /** The step the backend is currently stopped at, if any. */
  get currentStep(): RebaseStep | null {
    return this.state?.plan[this.current] ?? null;
  }

  // -- lifecycle ----------------------------------------------------------------

  /**
   * Starts (or confirms) monitoring `repoId`: immediate poll, then every
   * {@link REBASE_POLL_MS}. Idempotent for the same repo while polling;
   * a different repo (or a stopped monitor) resets and restarts.
   */
  start(repoId: string): void {
    if (this.polling && this.#repoId === repoId) return;
    this.#hardStop();
    this.#repoId = repoId;
    this.polling = true;
    this.#startedAt = Date.now();
    void this.#tick();
    this.#armTimer(REBASE_POLL_MS);
  }

  /** Stops polling and clears all state. Safe to call repeatedly. */
  stop(): void {
    this.#hardStop();
  }

  // -- delegation ----------------------------------------------------------------

  /**
   * Continues the rebase (`rebase_continue`) and applies the returned state;
   * conflicts are refreshed immediately when the backend pauses again.
   * Returns the new state, or `null` on failure (toasted, never thrown).
   */
  async continueRebase(): Promise<RebaseState | null> {
    const repoId = this.#repoId;
    if (repoId === null) return null;
    try {
      const state = await rebaseContinue(repoId);
      this.state = state;
      this.active = state.active;
      this.error = null;
      if (state.active && (state.paused_for_edit || state.paused_for_conflict)) {
        try {
          this.conflicts = await conflicts(repoId);
        } catch {
          // Non-fatal — the next poll refreshes the conflict list.
        }
      } else {
        this.conflicts = [];
        this.#hadConflicts = false;
      }
      return state;
    } catch (err) {
      this.error = errorMessage(err);
      toast(`Rebase continue failed: ${this.error}`, { kind: "error" });
      return null;
    }
  }

  /**
   * Aborts the rebase (`rebase_abort`) and resets the monitor. Returns
   * whether the backend confirmed the abort; failures are toasted.
   */
  async abort(): Promise<boolean> {
    const repoId = this.#repoId;
    if (repoId === null) return false;
    try {
      await rebaseAbort(repoId);
      toast("Rebase aborted — branch restored", { kind: "info" });
      this.stop();
      return true;
    } catch (err) {
      this.error = errorMessage(err);
      toast(`Rebase abort failed: ${this.error}`, { kind: "error" });
      return false;
    }
  }

  // -- internals ----------------------------------------------------------------

  async #tick(): Promise<void> {
    const repoId = this.#repoId;
    if (repoId === null || this.#inFlight) return;
    this.#inFlight = true;
    const token = this.#token;
    try {
      const state = await rebaseState(repoId);
      if (token !== this.#token) return;
      this.error = null;
      this.state = state;
      this.active = state.active;

      const paused = state.active && (state.paused_for_edit || state.paused_for_conflict);
      if (paused) {
        const now = Date.now();
        // Conflicts change as the user resolves files, so refresh while
        // paused — but on a 1s cadence + every pause transition, not on
        // every 500ms tick.
        if (!this.#wasPaused || now - this.#conflictsAt >= CONFLICTS_REFRESH_MS) {
          this.#conflictsAt = now;
          try {
            const files = await conflicts(repoId);
            if (token !== this.#token) return;
            this.conflicts = files;
          } catch (err) {
            if (token !== this.#token) return;
            this.error = errorMessage(err);
          }
        }
        if (this.conflicts.length > 0 && !this.#hadConflicts) {
          this.#hadConflicts = true;
          toast(
            `Rebase conflict in ${this.conflicts.length} file${this.conflicts.length === 1 ? "" : "s"} — resolve in the status panel, then Continue`,
            { kind: "error" },
          );
        }
      } else {
        if (this.conflicts.length > 0) {
          this.conflicts = [];
          this.#hadConflicts = false;
        }
      }
      this.#wasPaused = paused;

      if (!state.active) {
        // Poll-until-done contract: a rebase we were watching finished.
        if (this.#engaged) toast("Rebase complete", { kind: "success" });
        this.#finish();
        return;
      }
      this.#engaged = true;
      // Adaptive cadence: fast lane for the typical short rebase, backed off
      // once polling has run a while (conflict-resolution sessions).
      const interval =
        Date.now() - this.#startedAt < REBASE_BACKOFF_AFTER_MS
          ? REBASE_POLL_MS
          : REBASE_BACKOFF_POLL_MS;
      if (interval !== this.#timerInterval) this.#armTimer(interval);
    } catch (err) {
      // Transient poll failures keep the timer running; surface, don't die.
      if (token !== this.#token) return;
      this.error = errorMessage(err);
    } finally {
      this.#inFlight = false;
    }
  }

  #armTimer(interval: number): void {
    if (this.#timer !== null) clearInterval(this.#timer);
    this.#timerInterval = interval;
    this.#timer = setInterval(() => void this.#tick(), interval);
  }

  /** Natural stop: clear the timer but keep the final state for display. */
  #finish(): void {
    if (this.#timer !== null) {
      clearInterval(this.#timer);
      this.#timer = null;
    }
    this.#timerInterval = 0;
    this.polling = false;
    this.active = false;
    this.#engaged = false;
    this.#hadConflicts = false;
  }

  #hardStop(): void {
    this.#token++;
    if (this.#timer !== null) {
      clearInterval(this.#timer);
      this.#timer = null;
    }
    this.#timerInterval = 0;
    this.#repoId = null;
    this.polling = false;
    this.active = false;
    this.state = null;
    this.conflicts = [];
    this.error = null;
    this.#engaged = false;
    this.#hadConflicts = false;
    this.#inFlight = false;
  }
}

/** Shared monitor — every rebase surface attaches to this one. */
export const rebaseMonitor = new RebaseMonitor();
