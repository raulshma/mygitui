/**
 * PollScheduler unit tests (plan §4.3) — a fully injected environment: a
 * FakeClock drives all timers (no vi.useFakeTimers — the scheduler's clock
 * is injectable by contract), a flag-backed VisibilitySource stands in for
 * document.hidden. Covers: lane cadence, the seconds floor, hidden pause,
 * exponential backoff (403 ⇒ backoff) with reset on success, manual
 * refresh, unsubscribe and lane replacement.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  BACKOFF_MAX_MS,
  MIN_INTERVAL_MS,
  PollScheduler,
  type Clock,
  type VisibilitySource,
} from "$lib/stores/pollScheduler.svelte";

/** Drains microtasks + one macrotask: work chains settle before the next
 * fake timer fires. */
async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

/** Injectable fake clock: deterministic timeouts, advanced by hand. */
class FakeClock implements Clock {
  #seq = 0;
  #timers = new Map<number, { at: number; run: () => void }>();
  #now = 0;

  setTimeout(run: () => void, ms: number): unknown {
    const id = ++this.#seq;
    this.#timers.set(id, { at: this.#now + Math.max(0, ms), run });
    return id;
  }

  clearTimeout(handle: unknown): void {
    this.#timers.delete(handle as number);
  }

  /** Pending timer count (lanes waiting to tick). */
  get pending(): number {
    return this.#timers.size;
  }

  now(): number {
    return this.#now;
  }

  /** Fires every timer due within `ms`, earliest first, settling the
   * microtasks each fires before judging the next one due. */
  async advance(ms: number): Promise<void> {
    const target = this.#now + ms;
    for (;;) {
      let due: { id: number; at: number; run: () => void } | null = null;
      for (const [id, timer] of this.#timers) {
        if (timer.at <= target && (due === null || timer.at < due.at)) {
          due = { id, at: timer.at, run: timer.run };
        }
      }
      if (due === null) break;
      this.#timers.delete(due.id);
      this.#now = due.at;
      due.run();
      await settle();
    }
    this.#now = target;
  }
}

function flagVisibility(initialHidden = false): VisibilitySource & {
  setHidden(next: boolean): void;
} {
  let hidden = initialHidden;
  return {
    isHidden: () => hidden,
    setHidden: (next: boolean) => {
      hidden = next;
    },
  };
}

let clock: FakeClock;
let visibility: ReturnType<typeof flagVisibility>;

beforeEach(() => {
  clock = new FakeClock();
  visibility = flagVisibility();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("PollScheduler lanes", () => {
  it("runs a lane on its cadence and keeps exactly one timer pending", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const work = vi.fn();
    scheduler.register("repo:a:snapshot", { intervalMs: 5_000, work });

    expect(clock.pending).toBe(1);
    await clock.advance(4_999);
    expect(work).not.toHaveBeenCalled();
    await clock.advance(1);
    expect(work).toHaveBeenCalledTimes(1);
    expect(clock.pending).toBe(1);

    await clock.advance(5_000);
    expect(work).toHaveBeenCalledTimes(2);
    expect(scheduler.intervalOf("repo:a:snapshot")).toBe(5_000);
    expect(scheduler.lanes["repo:a:snapshot"]?.erroring).toBe(false);
  });

  it("clamps sub-second cadences up to the seconds floor", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const work = vi.fn();
    scheduler.register("repo:a:logs", { intervalMs: 250, work });

    await clock.advance(1_000);
    expect(work).toHaveBeenCalledTimes(1);
    expect(scheduler.intervalOf("repo:a:logs")).toBe(MIN_INTERVAL_MS);
  });

  it("does not schedule a lane registered with enabled=false", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const work = vi.fn();
    scheduler.register("repo:a:paused", { intervalMs: 1_000, work, enabled: false });

    expect(clock.pending).toBe(0);
    await clock.advance(60_000);
    expect(work).not.toHaveBeenCalled();
    // Registered-but-paused: state exists, healthy.
    expect(scheduler.lanes["repo:a:paused"]).toEqual({
      running: false,
      erroring: false,
      consecutiveErrors: 0,
      backoffMs: 0,
    });
  });
});

describe("PollScheduler hidden pause", () => {
  it("skips work while the document is hidden and resumes on visibility", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const work = vi.fn();
    scheduler.register("repo:a:snapshot", { intervalMs: 5_000, work });

    visibility.setHidden(true);
    await clock.advance(15_000); // three ticks worth — all skipped
    expect(work).not.toHaveBeenCalled();

    visibility.setHidden(false);
    await clock.advance(5_000);
    expect(work).toHaveBeenCalledTimes(1);
  });

  it("keeps a failing lane's earned backoff across hidden ticks", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const work = vi.fn(() => Promise.reject(new Error("403")));
    scheduler.register("repo:a:snapshot", { intervalMs: 5_000, work });

    await clock.advance(5_000); // first failure → next at 10s
    expect(work).toHaveBeenCalledTimes(1);

    visibility.setHidden(true);
    await clock.advance(20_000); // skipped, but the 10s delay stands
    visibility.setHidden(false);
    await clock.advance(5_000); // total visible time since failure: 5s < 10s
    expect(work).toHaveBeenCalledTimes(1);
    await clock.advance(5_000); // reaches the 10s backoff point
    expect(work).toHaveBeenCalledTimes(2);
  });
});

describe("PollScheduler backoff", () => {
  it("doubles the delay per consecutive failure and caps it", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const work = vi.fn(() => Promise.reject(new Error("403 rate limited")));
    scheduler.register("repo:a:snapshot", { intervalMs: 5_000, work });

    await clock.advance(5_000);
    expect(work).toHaveBeenCalledTimes(1);
    expect(scheduler.lanes["repo:a:snapshot"]).toEqual({
      running: false,
      erroring: true,
      consecutiveErrors: 1,
      backoffMs: 10_000,
    });

    // The second attempt only lands after the doubled delay.
    await clock.advance(9_999);
    expect(work).toHaveBeenCalledTimes(1);
    await clock.advance(1);
    expect(work).toHaveBeenCalledTimes(2);
    expect(scheduler.lanes["repo:a:snapshot"]?.backoffMs).toBe(20_000);

    await clock.advance(20_000);
    expect(work).toHaveBeenCalledTimes(3);
    expect(scheduler.lanes["repo:a:snapshot"]?.backoffMs).toBe(40_000);

    // ...capped at BACKOFF_MAX_MS no matter how many failures pile up.
    for (let i = 0; i < 8; i++) {
      await clock.advance(BACKOFF_MAX_MS);
    }
    expect(scheduler.lanes["repo:a:snapshot"]?.backoffMs).toBe(BACKOFF_MAX_MS);
    expect(scheduler.lanes["repo:a:snapshot"]?.erroring).toBe(true);
  });

  it("resets to the plain cadence after a success", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    let fail = true;
    const work = vi.fn(() => (fail ? Promise.reject(new Error("403")) : Promise.resolve()));
    scheduler.register("repo:a:snapshot", { intervalMs: 5_000, work });

    await clock.advance(5_000); // fail 1 → next at 10s
    await clock.advance(10_000); // fail 2 → next at 20s
    expect(scheduler.lanes["repo:a:snapshot"]?.consecutiveErrors).toBe(2);

    fail = false;
    await clock.advance(20_000); // success
    expect(scheduler.lanes["repo:a:snapshot"]).toEqual({
      running: false,
      erroring: false,
      consecutiveErrors: 0,
      backoffMs: 0,
    });

    // Back on the plain cadence: next runs every 5s exactly.
    await clock.advance(4_999);
    expect(work).toHaveBeenCalledTimes(3);
    await clock.advance(1);
    expect(work).toHaveBeenCalledTimes(4);
  });

  it("reports the run as in-flight while work is pending", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const gate: { release: (() => void) | null } = { release: null };
    const work = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          gate.release = resolve;
        }),
    );
    scheduler.register("repo:a:snapshot", { intervalMs: 1_000, work });

    await clock.advance(1_000);
    expect(work).toHaveBeenCalledTimes(1);
    expect(scheduler.lanes["repo:a:snapshot"]?.running).toBe(true);

    gate.release?.();
    await settle();
    expect(scheduler.lanes["repo:a:snapshot"]?.running).toBe(false);
  });
});

describe("PollScheduler manual refresh", () => {
  it("runs the work immediately and restarts the cadence", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const work = vi.fn();
    scheduler.register("repo:a:snapshot", { intervalMs: 60_000, work });

    await scheduler.refresh("repo:a:snapshot");
    expect(work).toHaveBeenCalledTimes(1);

    // The cadence restarted at the refresh: the first tick is a full
    // interval after the manual run, not after registration.
    await clock.advance(59_999);
    expect(work).toHaveBeenCalledTimes(1);
    await clock.advance(1);
    expect(work).toHaveBeenCalledTimes(2);
  });

  it("bypasses the hidden pause (user-initiated) and resets backoff on success", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    let fail = true;
    const work = vi.fn(() => (fail ? Promise.reject(new Error("403")) : Promise.resolve()));
    scheduler.register("repo:a:snapshot", { intervalMs: 5_000, work });
    await clock.advance(5_000); // one failure
    expect(scheduler.lanes["repo:a:snapshot"]?.erroring).toBe(true);

    visibility.setHidden(true);
    fail = false;
    await scheduler.refresh("repo:a:snapshot");
    expect(work).toHaveBeenCalledTimes(2);
    expect(scheduler.lanes["repo:a:snapshot"]?.erroring).toBe(false);
  });

  it("is a no-op for unknown lanes and while a run is in flight", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    await expect(scheduler.refresh("nope")).resolves.toBeUndefined();

    const gate: { release: (() => void) | null } = { release: null };
    const work = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          gate.release = resolve;
        }),
    );
    scheduler.register("repo:a:snapshot", { intervalMs: 1_000, work });
    await clock.advance(1_000);
    expect(work).toHaveBeenCalledTimes(1);

    const second = scheduler.refresh("repo:a:snapshot"); // in flight → ignored
    gate.release?.();
    await settle();
    await second;
    expect(work).toHaveBeenCalledTimes(1);
  });
});

describe("PollScheduler lifecycle", () => {
  it("unsubscribes fully: no timers, no state", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const work = vi.fn();
    const unsubscribe = scheduler.register("repo:a:snapshot", {
      intervalMs: 5_000,
      work,
    });

    unsubscribe();
    expect(clock.pending).toBe(0);
    expect(scheduler.lanes["repo:a:snapshot"]).toBeUndefined();

    await clock.advance(60_000);
    expect(work).not.toHaveBeenCalled();
  });

  it("re-registering a key replaces the lane (cadence change)", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const work = vi.fn();
    scheduler.register("repo:a:snapshot", { intervalMs: 10_000, work });
    scheduler.register("repo:a:snapshot", { intervalMs: 3_000, work });

    expect(clock.pending).toBe(1);
    await clock.advance(3_000);
    expect(work).toHaveBeenCalledTimes(1);
  });

  it("a run finishing after unregister leaves no state or timer behind", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const gate: { release: (() => void) | null } = { release: null };
    const work = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          gate.release = resolve;
        }),
    );
    scheduler.register("repo:a:snapshot", { intervalMs: 1_000, work });
    await clock.advance(1_000);
    expect(work).toHaveBeenCalledTimes(1);

    const unsubscribe = scheduler.register("repo:a:snapshot", { intervalMs: 1_000, work });
    // Register replaced the lane while the first run is still in flight;
    // unsubscribing mid-run must not resurrect the lane when it settles.
    unsubscribe();
    gate.release?.();
    await settle();

    expect(scheduler.lanes["repo:a:snapshot"]).toBeUndefined();
    expect(clock.pending).toBe(0);
    expect(work).toHaveBeenCalledTimes(1);
  });

  it("stopAll clears every lane", async () => {
    const scheduler = new PollScheduler({ clock, visibility });
    const work = vi.fn();
    scheduler.register("a", { intervalMs: 1_000, work });
    scheduler.register("b", { intervalMs: 2_000, work });
    scheduler.stopAll();

    expect(clock.pending).toBe(0);
    expect(Object.keys(scheduler.lanes)).toEqual([]);
    await clock.advance(60_000);
    expect(work).not.toHaveBeenCalled();
  });
});
