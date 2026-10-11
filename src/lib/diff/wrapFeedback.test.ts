/**
 * Unit tests for the wrap estimate → measure → patch feedback loop
 * (diff/wrapFeedback.ts) with injected fake frames: rAF batching, divergence-
 * only recording (convergence), the oscillation guard, and invalidation on
 * content change.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { createWrapFeedback, type WrapFeedbackDeps } from "./wrapFeedback";

// ---------------------------------------------------------------------------
// Fakes

/** Single-slot frame queue — the loop schedules at most one frame at a time. */
function fakeFrames() {
  let queued: (() => void) | null = null;
  let cancels = 0;
  return {
    requestFrame(cb: () => void): number {
      queued = cb;
      return 1;
    },
    cancelFrame(): void {
      queued = null;
      cancels++;
    },
    /** Runs the pending frame callback, if any. */
    flush(): void {
      const cb = queued;
      queued = null;
      cb?.();
    },
    get pending(): boolean {
      return queued !== null;
    },
    get cancels(): number {
      return cancels;
    },
  };
}

/**
 * Harness mirroring the component: `heightAt` reads the PATCHED heights
 * (override, else estimate) and `applyOverrides` installs the record into
 * reactive state — here a plain local, so convergence behaves as in vivo.
 */
function harness(opts: { base?: number[]; lineHeight?: number; maxRounds?: number } = {}) {
  const frames = fakeFrames();
  const base = opts.base ?? [20, 20, 20];
  let overrides: Record<number, number> = {};
  const applied: Record<number, number>[] = [];
  const deps: WrapFeedbackDeps = {
    lineHeight: opts.lineHeight ?? 20,
    heightAt: (index) => overrides[index] ?? base[index],
    overrides: () => overrides,
    applyOverrides: (next) => {
      overrides = next;
      applied.push(next);
    },
    requestFrame: frames.requestFrame,
    cancelFrame: frames.cancelFrame,
    ...(opts.maxRounds === undefined ? {} : { maxRounds: opts.maxRounds }),
  };
  return {
    feedback: createWrapFeedback(deps),
    frames,
    applied,
    get overrides(): Record<number, number> {
      return overrides;
    },
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------

describe("measure batching", () => {
  it("batches multiple rows into a single applied frame", () => {
    const { feedback, frames, applied } = harness();
    feedback.measure(0, 60);
    feedback.measure(1, 40);
    expect(frames.pending).toBe(true); // one frame for both
    frames.flush();
    expect(applied).toEqual([{ 0: 60, 1: 40 }]);
    expect(frames.pending).toBe(false);
  });

  it("snaps heights to the line grid, floored at one line", () => {
    const { feedback, frames, applied } = harness();
    feedback.measure(0, 43); // → 2 lines
    feedback.measure(1, 51); // → 3 lines (rounds up)
    feedback.measure(9, 4); // sub-line text → floored at one line (row 9: no layout height)
    frames.flush();
    expect(applied).toEqual([{ 0: 40, 1: 60, 9: 20 }]);
  });

  it("measurements matching the layout are dropped (no frame work)", () => {
    const { feedback, frames, applied } = harness();
    feedback.measure(0, 20); // base height is 20
    frames.flush();
    expect(applied).toEqual([]);
  });

  it("a converged row is not re-applied on re-measure", () => {
    const { feedback, frames, applied } = harness();
    feedback.measure(0, 60);
    frames.flush();
    expect(applied).toEqual([{ 0: 60 }]);
    feedback.measure(0, 60); // layout now agrees with the measurement
    frames.flush();
    expect(applied).toEqual([{ 0: 60 }]);
  });

  it("rows beyond the current layout are recorded (layout may grow later)", () => {
    const { feedback, frames, applied } = harness({ base: [20, 20, 20] });
    feedback.measure(9, 40);
    frames.flush();
    expect(applied).toEqual([{ 9: 40 }]);
  });
});

describe("oscillation guard", () => {
  it("settles after maxRounds consecutive mutating frames", () => {
    const { feedback, frames, applied } = harness({ maxRounds: 2 });
    feedback.measure(0, 60); // round 1: applies
    frames.flush();
    feedback.measure(0, 80); // round 2: applies
    frames.flush();
    feedback.measure(0, 60); // flip-flop: dropped — layout settles
    frames.flush();
    expect(applied).toEqual([{ 0: 60 }, { 0: 80 }]);
  });

  it("a convergent round re-arms the guard", () => {
    const { feedback, frames, applied } = harness({ maxRounds: 2 });
    feedback.measure(0, 60); // mutating round 1
    frames.flush();
    feedback.measure(0, 60); // converged → counter resets
    frames.flush();
    feedback.measure(0, 80); // allowed again (would be blocked without reset)
    frames.flush();
    expect(applied).toEqual([{ 0: 60 }, { 0: 80 }]);
  });
});

describe("invalidate / dispose", () => {
  it("invalidate applies an empty record, drops pending measurements, re-arms", () => {
    const { feedback, frames, applied } = harness({ maxRounds: 1 });
    feedback.measure(0, 60);
    frames.flush();
    feedback.measure(0, 80); // settled (maxRounds 1) → dropped
    frames.flush();
    feedback.invalidate(); // new content: corrections cleared, guard re-armed
    feedback.measure(0, 80); // applies again
    frames.flush();
    expect(applied).toEqual([{ 0: 60 }, {}, { 0: 80 }]);
  });

  it("a frame flushed after invalidate applies nothing stale", () => {
    const { feedback, frames, applied } = harness();
    feedback.measure(0, 60);
    feedback.invalidate(); // content changed before the frame ran
    frames.flush();
    expect(applied).toEqual([{}]);
  });

  it("dispose cancels the pending frame", () => {
    const { feedback, frames, applied } = harness();
    feedback.measure(0, 60);
    feedback.dispose();
    expect(frames.cancels).toBe(1);
    frames.flush();
    expect(applied).toEqual([]);
  });

  it("clears pending rows so a later measure batches fresh", () => {
    const { feedback, frames, applied } = harness();
    feedback.measure(0, 60);
    feedback.invalidate();
    feedback.measure(1, 40);
    frames.flush();
    expect(applied).toEqual([{}, { 1: 40 }]);
  });
});

describe("default scheduler", () => {
  it("falls back to requestAnimationFrame without an injected one", () => {
    const frames = fakeFrames();
    vi.stubGlobal("requestAnimationFrame", frames.requestFrame);
    vi.stubGlobal("cancelAnimationFrame", frames.cancelFrame);
    let overrides: Record<number, number> = {};
    const applied: Record<number, number>[] = [];
    const feedback = createWrapFeedback({
      lineHeight: 20,
      heightAt: (index) => overrides[index] ?? 20,
      overrides: () => overrides,
      applyOverrides: (next) => {
        overrides = next;
        applied.push(next);
      },
    });
    feedback.measure(0, 40);
    expect(frames.pending).toBe(true);
    frames.flush();
    expect(applied).toEqual([{ 0: 40 }]);
  });
});
