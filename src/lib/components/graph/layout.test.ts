/**
 * Unit tests for the pure layout/scroll math (layout.ts), including the
 * 250k-row visible-slice perf gate (< 1ms per call).
 */

import { describe, expect, it } from "vitest";
import type { GraphRow } from "$lib/ipc/types";
import {
  applyDprTransform,
  crisp,
  laneX,
  pixelSize,
  rowCenterY,
  scrollToRowTarget,
  visibleRange,
  visibleSlice,
} from "$lib/components/graph/layout";

describe("visibleRange", () => {
  it("computes the on-screen window plus overscan on both sides", () => {
    // 40 visible rows at scrollTop 0, viewport 960px, rowHeight 24.
    expect(visibleRange(0, 960, 1000, 24, 20)).toEqual({ first: 0, last: 41 + 20 });
  });

  it("clamps to the list bounds at both ends", () => {
    expect(visibleRange(0, 960, 30, 24, 20)).toEqual({ first: 0, last: 30 });
    const atBottom = visibleRange(1000 * 24 - 960, 960, 1000, 24, 20);
    expect(atBottom.first).toBe(1000 - 40 - 20);
    expect(atBottom.last).toBe(1000);
  });

  it("extends below a mid-row scrollTop and clamps negative scrollTop", () => {
    // scrollTop 12, viewport 48: rows 0 (partial), 1, 2 (partial) → [0, 3).
    expect(visibleRange(12, 48, 100, 24, 0)).toEqual({ first: 0, last: 3 });
    expect(visibleRange(-50, 48, 100, 24, 5)).toEqual({ first: 0, last: 3 + 5 });
  });

  it("returns an empty range for degenerate inputs", () => {
    expect(visibleRange(0, 100, 0, 24)).toEqual({ first: 0, last: 0 });
    expect(visibleRange(0, 100, 10, 0)).toEqual({ first: 0, last: 0 });
    expect(visibleRange(0, 0, 10, 24)).toEqual({ first: 0, last: 0 });
    expect(visibleRange(Number.NaN, 100, 10, 24)).toEqual({ first: 0, last: 0 });
  });
});

describe("visibleSlice perf gate (250k commits)", () => {
  const TOTAL = 250_000;
  const rows: GraphRow[] = new Array(TOTAL);
  for (let i = 0; i < TOTAL; i += 1) {
    rows[i] = { sha: `sha-${i}`, lane: i % 7, edges: [], lane_count: 7 };
  }

  it("slices only the visible window and stays under 1ms per call", () => {
    const scrollTop = 120_000 * 24; // middle of the list
    const slice = visibleSlice(rows, scrollTop, 960, 24, 20);
    // first = 120000 - 20, last = 120000 + 41 + 20 → 81 rows.
    expect(slice).toHaveLength(81);
    expect(slice[0]?.sha).toBe("sha-119980");
    expect(slice[80]?.sha).toBe("sha-120060");

    const iterations = 200;
    visibleSlice(rows, scrollTop, 960, 24, 20); // warm-up
    const started = performance.now();
    for (let i = 0; i < iterations; i += 1) {
      visibleSlice(rows, scrollTop + i, 960, 24, 20);
    }
    const averageMs = (performance.now() - started) / iterations;
    // eslint-disable-next-line no-console
    console.log(`visibleSlice avg over 250k rows: ${averageMs.toFixed(4)} ms/call`);
    expect(averageMs).toBeLessThan(1);
  });
});

describe("geometry helpers", () => {
  it("laneX = padding + lane × laneWidth", () => {
    expect(laneX(0, 14, 10)).toBe(10);
    expect(laneX(3, 14, 10)).toBe(52);
    expect(laneX(2, 14, 0)).toBe(28);
  });

  it("rowCenterY sits at the band midpoint", () => {
    expect(rowCenterY(0, 24)).toBe(12);
    expect(rowCenterY(7, 24)).toBe(180);
  });
});

describe("DPR math", () => {
  it("pixelSize scales CSS px by dpr and rounds", () => {
    expect(pixelSize(100, 50, 2)).toEqual({ w: 200, h: 100 });
    expect(pixelSize(100.5, 50.25, 2)).toEqual({ w: 201, h: 101 });
    expect(pixelSize(333, 200, 1.5)).toEqual({ w: 500, h: 300 });
    expect(pixelSize(-5, 0, 2)).toEqual({ w: 0, h: 0 });
    expect(pixelSize(100, 50, 0)).toEqual({ w: 100, h: 50 }); // invalid dpr → 1
  });

  it("applyDprTransform sets the scale matrix", () => {
    const transforms: number[][] = [];
    const ctx = {
      setTransform: (...args: number[]) => transforms.push(args),
    } as unknown as CanvasRenderingContext2D;
    applyDprTransform(ctx, 2);
    applyDprTransform(ctx, 1.25);
    expect(transforms).toEqual([
      [2, 0, 0, 2, 0, 0],
      [1.25, 0, 0, 1.25, 0, 0],
    ]);
  });

  it("crisp aligns stroke centers to the physical pixel grid", () => {
    // A 1px stroke centered on an integer boundary blurs; shift by 0.5.
    expect(crisp(10, 1)).toBe(10.5);
    // 1.5px strokes align to quarter-pixel centers.
    expect(crisp(10, 1.5)).toBe(9.75);
    expect(crisp(24, 1.5)).toBe(23.75);
    // Even widths stay on integers.
    expect(crisp(10, 2)).toBe(10);
  });
});

describe("scrollToRowTarget", () => {
  it("centers the target row", () => {
    // Row 10 center = 252; viewport 480 → scrollTop 12.
    expect(scrollToRowTarget(10, 100, 24, 480)).toBe(12);
  });

  it("clamps the row index and the scroll offset", () => {
    expect(scrollToRowTarget(-5, 100, 24, 480)).toBe(0);
    // Row 99 center 2388 - 240 = 2148, but max scroll = 2400 - 480 = 1920.
    expect(scrollToRowTarget(99, 100, 24, 480)).toBe(1920);
    expect(scrollToRowTarget(500, 100, 24, 480)).toBe(1920);
    expect(scrollToRowTarget(0, 0, 24, 480)).toBe(0);
  });
});
