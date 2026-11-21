/**
 * PollScheduler (plan §4.3) — the ONE owner of background polling lanes.
 * Workstream E1 migrates the forge polling (ForgePanel checks/runs watches,
 * WorkflowRunView log poll) and retrofits `autofetch.svelte.ts` onto it;
 * nothing adopts it before then (this module ships the primitive + tests
 * only).
 *
 * Model: a lane is a registered task — `{ intervalMs, work, enabled? }` —
 * keyed by a stable string (`repo:<id>:checks`, `repo:<id>:logs`, …).
 * Each lane runs its work serially on its own cadence; the next tick is
 * scheduled only after the previous run settles, so runs never overlap.
 *
 * Behavior contract (§9.1–9.3):
 * - **Cadence**: per-lane `intervalMs`, clamped up to the seconds floor
 *   (`MIN_INTERVAL_MS` — the scheduler has no sub-second lanes; §9.3's
 *   30s floor for user-configurable cadences lives with the settings UI,
 *   not here — the fixed 3.5s log lane must keep working).
 * - **Hidden pause**: while `document.hidden` (injectable), ticks skip
 *   their work and reschedule untouched — a hidden window spawns zero
 *   gh calls; the next visible tick lands within one interval (plus any
 *   backoff already in effect).
 * - **Exponential backoff**: a failed run (thrown/rejected — e.g. a 403
 *   from `gh`) doubles the lane's next delay, capped at `BACKOFF_MAX_MS`;
 *   a successful run resets the lane to its plain cadence. The per-lane
 *   state (`lanes`) is reactive so chips/UI can show a lane is degraded
 *   and how long the next retry is out ("retry in Xm", §9.2).
 * - **Manual refresh**: `refresh(key)` runs the work immediately (bypassing
 *   the hidden pause — it is a user-initiated action), restarts the lane's
 *   cadence from that point, and no-ops while a run is in flight.
 *
 * Registration schedules the FIRST tick one interval out — callers own
 * their on-open fetch (§9.3: "panel lists on-open + 60s"); `refresh` covers
 * the "and forces one refresh" half of the tab-switch promotion.
 *
 * Browser surface is injectable (`Clock` + `VisibilitySource`) so unit
 * tests run on a fake clock with a fake visibility flag — nothing touches
 * timers or `document` outside the injected surface. Lane state is
 * in-memory only: cadence persistence belongs to the settings UI (E1), not
 * to the scheduler.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

import { untrack } from "svelte";

/** Injectable timer surface (tests drive a fake clock through these). */
export interface Clock {
  setTimeout(handler: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

const defaultClock: Clock = {
  setTimeout: (handler, ms) => setTimeout(handler, ms),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** Injectable visibility source (`document.hidden` in production). */
export interface VisibilitySource {
  isHidden(): boolean;
}

const defaultVisibility: VisibilitySource = {
  isHidden: () =>
    typeof document === "undefined" ? false : document.visibilityState === "hidden",
};

/** Registration options for one lane. */
export interface PollTaskOptions {
  /** Cadence in ms; clamped up to {@link MIN_INTERVAL_MS} (seconds granularity). */
  intervalMs: number;
  /** The unit of work; a throw/rejection (e.g. gh 403) triggers backoff. */
  work: () => Promise<void> | void;
  /** `false` registers the lane without scheduling it (default `true`). */
  enabled?: boolean;
}

/** Reactive per-lane state (UI reads this to render degraded lanes). */
export interface PollLaneState {
  /** A run of this lane's work is in flight right now. */
  running: boolean;
  /** The last attempt failed (a success resets this). */
  erroring: boolean;
  /** Failed runs since the last success (0 = healthy). */
  consecutiveErrors: number;
  /** Extra delay the next tick waits (0 = plain cadence). */
  backoffMs: number;
}

/** Seconds granularity: the scheduler has no sub-second lanes. */
export const MIN_INTERVAL_MS = 1_000;

/** Backoff ceiling — a failing lane never waits longer than this (§9.3). */
export const BACKOFF_MAX_MS = 10 * 60_000;

/** Backoff growth: each failure doubles the delay (2×, 4×, 8×…). */
export const BACKOFF_BASE = 2;

export class PollScheduler {
  /** Reactive lane states, keyed by the registration key. */
  lanes: Record<string, PollLaneState> = $state({});

  #clock: Clock;
  #visibility: VisibilitySource;
  /** Pending timers per lane (one in-flight timeout per registered lane). */
  #timers = new Map<string, unknown>();
  /** The live options per registered lane (re-read on every reschedule). */
  #tasks = new Map<string, PollTaskOptions>();
  /** Runs in flight, per lane (a lane never runs two works at once). */
  #running = new Set<string>();

  constructor(
    inject: { clock?: Clock; visibility?: VisibilitySource } = {},
  ) {
    this.#clock = inject.clock ?? defaultClock;
    this.#visibility = inject.visibility ?? defaultVisibility;
  }

  /**
   * Registers (or replaces) a lane and starts it on its cadence. Returns
   * the unsubscribe function: after it runs the lane is fully gone (timer
   * cleared, state dropped) and re-registering the key starts fresh.
   */
  register(key: string, options: PollTaskOptions): () => void {
    // Imperative API: the lanes state touched here must NEVER become a
    // dependency of the calling effect. Both register and unregister READ
    // `lanes` (the spread / the `in` check) while writing it — tracked,
    // a caller's `$effect` would re-run on its own registration forever
    // (read → write → dirty → read …) until the update depth blows up.
    return untrack(() => {
      this.#stop(key);
      this.#tasks.set(key, options);
      const interval = clampInterval(options.intervalMs);
      this.lanes = {
        ...this.lanes,
        [key]: { running: false, erroring: false, consecutiveErrors: 0, backoffMs: 0 },
      };
      if (options.enabled !== false) {
        // First tick one interval out — callers own their on-open fetch.
        this.#schedule(key, interval);
      }
      return () => this.unregister(key);
    });
  }

  /** Removes a lane entirely (timer + state). No-op when unknown. */
  unregister(key: string): void {
    untrack(() => {
      this.#stop(key);
      this.#tasks.delete(key);
      if (key in this.lanes) {
        const next = { ...this.lanes };
        delete next[key];
        this.lanes = next;
      }
    });
  }

  /** Stops every lane (teardown helper). */
  stopAll(): void {
    for (const key of [...this.#tasks.keys()]) this.unregister(key);
  }

  /**
   * Runs a lane's work immediately (user-initiated): bypasses the hidden
   * pause, restarts the cadence from completion, and resets backoff on
   * success. No-op (resolves immediately) for unknown lanes and while the
   * lane's previous run is still in flight.
   */
  async refresh(key: string): Promise<void> {
    // Same tracking guard as register/unregister: refresh is invoked from
    // reactive contexts (tab-switch promotion) and its synchronous prefix
    // runs inside the caller's effect stack.
    const task = untrack(() => {
      const current = this.#tasks.get(key);
      if (!current || this.#running.has(key)) return null;
      // One run at a time per lane; drop the pending tick — the run below
      // reschedules from its own completion.
      this.#clearTimer(key);
      return current;
    });
    if (!task) return;
    await this.#run(key, task);
  }

  /** The delay a healthy lane waits between runs. */
  intervalOf(key: string): number {
    const task = this.#tasks.get(key);
    return task ? clampInterval(task.intervalMs) : 0;
  }

  // -- internals -------------------------------------------------------------

  #stop(key: string): void {
    this.#clearTimer(key);
  }

  #clearTimer(key: string): void {
    const handle = this.#timers.get(key);
    if (handle !== undefined) {
      this.#clock.clearTimeout(handle);
      this.#timers.delete(key);
    }
  }

  #schedule(key: string, delay: number): void {
    // A lane never carries two timers: reschedule = replace.
    this.#clearTimer(key);
    const handle = this.#clock.setTimeout(() => this.#tick(key), delay);
    this.#timers.set(key, handle);
  }

  #tick(key: string): void {
    this.#timers.delete(key);
    const task = this.#tasks.get(key);
    if (!task) return;
    const interval = clampInterval(task.intervalMs);
    const state = this.lanes[key];
    if (this.#visibility.isHidden()) {
      // Hidden window ⇒ zero spawns: skip the run, keep the same timing
      // the lane would have had (plain cadence, or its earned backoff).
      this.#schedule(key, state && state.erroring ? state.backoffMs : interval);
      return;
    }
    void this.#run(key, task);
  }

  async #run(key: string, task: PollTaskOptions): Promise<void> {
    // The synchronous prefix can run inside a caller's effect stack
    // (refresh); keep the lanes read/write out of its dependencies.
    const state = untrack(() => this.lanes[key]);
    if (!state || this.#running.has(key)) return;
    this.#running.add(key);
    untrack(() => {
      this.lanes[key] = { ...state, running: true };
    });
    let failed = false;
    try {
      await task.work();
    } catch {
      // Failure (e.g. gh 403) — handled below; never surfaces.
      failed = true;
    }
    this.#running.delete(key);
    // The lane may have been replaced or unregistered mid-run: only the
    // still-current task writes state and reschedules.
    if (this.#tasks.get(key) !== task) {
      if (this.lanes[key]) this.lanes[key] = { ...this.lanes[key], running: false };
      return;
    }
    if (failed) {
      const consecutiveErrors = state.consecutiveErrors + 1;
      const backoffMs = Math.min(
        clampInterval(task.intervalMs) * BACKOFF_BASE ** consecutiveErrors,
        BACKOFF_MAX_MS,
      );
      this.lanes[key] = {
        running: false,
        erroring: true,
        consecutiveErrors,
        backoffMs,
      };
      this.#schedule(key, backoffMs);
    } else {
      // Success: reset the lane to its plain cadence.
      this.lanes[key] = {
        running: false,
        erroring: false,
        consecutiveErrors: 0,
        backoffMs: 0,
      };
      this.#schedule(key, clampInterval(task.intervalMs));
    }
  }
}

function clampInterval(intervalMs: number): number {
  const finite = Number.isFinite(intervalMs) ? Math.floor(intervalMs) : MIN_INTERVAL_MS;
  return Math.max(MIN_INTERVAL_MS, finite);
}

/** The application-wide scheduler (Workstream E1 adopts this). */
export const pollScheduler = new PollScheduler();
