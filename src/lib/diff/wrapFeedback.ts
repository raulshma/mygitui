/**
 * Wrap estimate → measure → patch feedback loop for the diff viewer.
 *
 * Wrap-mode heights are analytic estimates (wrapHeights.ts); rendered rows
 * report their true DOM text height back and the corrections are patched
 * into the layout (`heightOverrides`). This module owns that cycle:
 *
 *   - `measure(index, textHeight)` — snaps a reported height to the line
 *     grid, batches measurements, and applies them one animation frame
 *     later (rows of one render pass report together);
 *   - a frame applies ONLY divergences (a measurement that already matches
 *     the current layout height — or the current override — is dropped), so
 *     the cycle converges instead of looping;
 *   - OSCILLATION GUARD (added with this extraction; the original inline
 *     DiffViewer loop had none): if `maxRounds` (default 5) consecutive
 *     frames each changed the overrides, the measurements keep flip-flopping
 *     rather than converging — further frames are dropped until a fresh
 *     `invalidate()` (content/mode/wrap/width change) re-arms the loop, so
 *     the layout settles with the last applied (measured) values instead of
 *     oscillating forever;
 *   - `invalidate()` — drops all corrections AND any pending measurements
 *     (stale row indices from the old content must not be applied);
 *   - `dispose()` — cancels a pending frame (component unmount).
 *
 * The host keeps ownership of the reactive state: it injects reads
 * (`heightAt`, `overrides`) and a write (`applyOverrides`) that mirror
 * `heightOverrides` — the module itself holds no runes.
 */

/** Frame scheduling injected for tests (defaults to rAF/cancelAnimationFrame). */
export interface FrameScheduler {
  requestFrame(cb: () => void): number;
  cancelFrame(handle: number): void;
}

export type WrapFeedbackDeps = {
  /** Row heights are multiples of this grid unit (DiffViewer: ROW_HEIGHTS.line). */
  lineHeight: number;
  /** Current layout height of a row (estimate patched by overrides). */
  heightAt(index: number): number | undefined;
  /** The current measured-corrections record. */
  overrides(): Record<number, number>;
  /** Installs a fresh corrections record (the host mirrors it into state). */
  applyOverrides(next: Record<number, number>): void;
  /** Mutating frames tolerated before the oscillation guard settles. */
  maxRounds?: number;
} & Partial<FrameScheduler>;

export interface WrapFeedback {
  /** A rendered row reported its measured text height (rAF-batched). */
  measure(index: number, textHeight: number): void;
  /** Content/mode/wrap/width changed: corrections and pending rows are stale. */
  invalidate(): void;
  /** Cancels any pending frame (component unmount). */
  dispose(): void;
}

/** Consecutive mutating frames tolerated before settling (see module doc). */
const DEFAULT_MAX_ROUNDS = 5;

export function createWrapFeedback(deps: WrapFeedbackDeps): WrapFeedback {
  const requestFrame =
    deps.requestFrame ?? ((cb: () => void) => requestAnimationFrame(cb));
  const cancelFrame =
    deps.cancelFrame ?? ((handle: number) => cancelAnimationFrame(handle));
  const maxRounds = deps.maxRounds ?? DEFAULT_MAX_ROUNDS;

  /** Measurements awaiting the next frame (one per row index). */
  const pending = new Map<number, number>();
  let frame = 0;
  /** Consecutive frames that changed the overrides (oscillation counter). */
  let mutatingRounds = 0;

  /** Snaps a measured height to the line grid, floored at one line. */
  function snap(textHeight: number): number {
    const line = deps.lineHeight;
    return Math.max(line, Math.round(textHeight / line) * line);
  }

  function flush(): void {
    frame = 0;
    if (mutatingRounds >= maxRounds) {
      // Oscillating (e.g. estimates and measurements keep disagreeing):
      // settle with the last applied values until the host invalidates.
      pending.clear();
      return;
    }
    const overrides = deps.overrides();
    let changed = false;
    let next: Record<number, number> = overrides;
    let copied = false;
    for (const [index, height] of pending) {
      if (deps.heightAt(index) !== height && overrides[index] !== height) {
        if (!copied) {
          next = { ...overrides };
          copied = true;
        }
        next[index] = height;
        changed = true;
      }
    }
    pending.clear();
    if (changed) {
      mutatingRounds++;
      deps.applyOverrides(next);
    } else {
      mutatingRounds = 0; // converged this round
    }
  }

  return {
    measure(index, textHeight) {
      pending.set(index, snap(textHeight));
      if (frame) return;
      frame = requestFrame(flush);
    },

    invalidate() {
      pending.clear();
      mutatingRounds = 0;
      deps.applyOverrides({});
    },

    dispose() {
      if (frame) {
        cancelFrame(frame);
        frame = 0;
      }
      pending.clear();
    },
  };
}
