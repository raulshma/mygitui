/**
 * Unit tests + perf gate for the diff virtualizer (diff/virtualizer.ts).
 *
 * Perf gate (spec): 10k hunks x ~20 lines = 200k rows — totalHeight +
 * visibleSlices must run in <2ms (asserted loosely at <50ms for CI stability;
 * the actual number is printed so regressions are visible in the log).
 */

import { describe, expect, it } from "vitest";
import type { DiffHunk, DiffLine, FileDiff } from "$lib/ipc/types";
import { buildRowModel, rowHeights, ROW_HEIGHTS } from "$lib/components/diff/rowModel";
import { buildLayout, rowTop, visibleSlices, type VLayout } from "$lib/components/diff/virtualizer";

// ---------------------------------------------------------------------------
// Bucket construction
// ---------------------------------------------------------------------------

describe("buildLayout", () => {
  it("returns an empty layout for no rows", () => {
    const layout = buildLayout([]);
    expect(layout.totalRows).toBe(0);
    expect(layout.totalHeight).toBe(0);
    expect(layout.buckets).toHaveLength(0);
    expect(visibleSlices(layout, 0, 600, 0)).toEqual([]);
  });

  it("merges consecutive equal heights into one bucket", () => {
    const layout = buildLayout([20, 20, 20]);
    expect(layout.buckets).toHaveLength(1);
    expect(layout.buckets[0]).toEqual({ firstRow: 0, count: 3, rowHeight: 20, top: 0 });
    expect(layout.totalRows).toBe(3);
    expect(layout.totalHeight).toBe(60);
  });

  it("creates a bucket per height change with correct top/firstRow", () => {
    const layout = buildLayout([40, 20, 20, 20, 26, 26, 20]);
    expect(layout.buckets).toEqual([
      { firstRow: 0, count: 1, rowHeight: 40, top: 0 },
      { firstRow: 1, count: 3, rowHeight: 20, top: 40 },
      { firstRow: 4, count: 2, rowHeight: 26, top: 100 },
      { firstRow: 6, count: 1, rowHeight: 20, top: 152 },
    ]);
    expect(layout.totalRows).toBe(7);
    expect(layout.totalHeight).toBe(172);
  });

  it("accepts array-likes (Uint32Array)", () => {
    const layout = buildLayout(new Uint32Array([20, 20, 30]));
    expect(layout.totalHeight).toBe(70);
    expect(layout.buckets).toHaveLength(2);
  });
});

// ---------------------------------------------------------------------------
// visibleSlices
// ---------------------------------------------------------------------------

describe("visibleSlices", () => {
  const layout = buildLayout([40, 20, 20, 20, 26, 26, 20]);

  it("covers the top of the list", () => {
    const slices = visibleSlices(layout, 0, 100, 0);
    // Window [0,100): bucket0 (row0), bucket1 rows 1..2 end 40+60=100 → rows 1-3.
    expect(slices).toEqual([
      { bucketIndex: 0, firstRow: 0, count: 1, top: 0, height: 40 },
      { bucketIndex: 1, firstRow: 1, count: 3, top: 40, height: 60 },
    ]);
  });

  it("computes partial bucket coverage mid-list", () => {
    const slices = visibleSlices(layout, 50, 30, 0);
    // Window [50,80): bucket1 rows 1-2 (y 40-60 partially, 60-80 fully).
    expect(slices).toEqual([{ bucketIndex: 1, firstRow: 1, count: 2, top: 40, height: 40 }]);
  });

  it("spans multiple buckets in the middle", () => {
    const slices = visibleSlices(layout, 60, 60, 0);
    // Window [60,120): bucket1 rows 2-3 (60..100), bucket2 row 4 (100..126).
    expect(slices).toEqual([
      { bucketIndex: 1, firstRow: 2, count: 2, top: 60, height: 40 },
      { bucketIndex: 2, firstRow: 4, count: 1, top: 100, height: 26 },
    ]);
  });

  it("includes the last rows at the bottom", () => {
    const slices = visibleSlices(layout, layout.totalHeight - 10, 100, 0);
    expect(slices.at(-1)).toEqual({
      bucketIndex: 3,
      firstRow: 6,
      count: 1,
      top: 152,
      height: 20,
    });
    const allFirst = slices.flatMap((s) =>
      Array.from({ length: s.count }, (_, k) => s.firstRow + k),
    );
    expect(allFirst).toContain(layout.totalRows - 1);
  });

  it("expands the window by overscan on both sides", () => {
    const tight = visibleSlices(layout, 100, 20, 0);
    const loose = visibleSlices(layout, 100, 20, 40);
    const rows = (ss: ReturnType<typeof visibleSlices>) =>
      ss.flatMap((s) => Array.from({ length: s.count }, (_, k) => s.firstRow + k));
    expect(rows(loose).length).toBeGreaterThan(rows(tight).length);
    expect(rows(loose)).toContain(2); // 40px overscan reaches above the window
    expect(rows(loose)).toContain(6); // ...and below it
  });

  it("returns nothing for a non-positive effective window", () => {
    expect(visibleSlices(layout, 0, 0, 0)).toEqual([]);
  });

  it("still renders an overscan band with a zero-height viewport", () => {
    const slices = visibleSlices(layout, 80, 0, 100);
    expect(slices.length).toBeGreaterThan(0);
    for (const s of slices) {
      expect(s.top).toBeGreaterThanOrEqual(0);
      expect(s.top + s.height).toBeLessThanOrEqual(layout.totalHeight);
    }
  });

  it("includes the final rows when scrolled to the max real position", () => {
    // Native scroll clamps scrollTop to totalHeight - viewport.
    const slices = visibleSlices(layout, layout.totalHeight - 50, 50, 0);
    expect(slices.at(-1)?.firstRow).toBe(6);
    expect(slices.at(-1)!.firstRow + slices.at(-1)!.count).toBe(layout.totalRows);
  });

  it("returns nothing when fully scrolled past the end with no overscan", () => {
    expect(visibleSlices(layout, layout.totalHeight + 500, 100, 0)).toEqual([]);
  });

  it("clamps negative scroll", () => {
    expect(visibleSlices(layout, -999, 50, 0)[0]).toMatchObject({ firstRow: 0, top: 0 });
  });
});

// ---------------------------------------------------------------------------
// rowTop
// ---------------------------------------------------------------------------

describe("rowTop", () => {
  const layout = buildLayout([40, 20, 20, 20, 26, 26, 20]);

  it("returns each row's exact offset", () => {
    const expected = [0, 40, 60, 80, 100, 126, 152];
    for (let i = 0; i < expected.length; i++) {
      expect(rowTop(layout, i)).toBe(expected[i]);
    }
  });

  it("clamps out-of-range indices", () => {
    expect(rowTop(layout, -5)).toBe(0);
    expect(rowTop(layout, 99)).toBe(152);
    expect(rowTop(buildLayout([]), 3)).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// Perf gate: 10k hunks x 20 lines = 200k+ rows
// ---------------------------------------------------------------------------

/** 10k hunks of 20 lines each (ctx/del/add mix), split across 100 files. */
function generateBigDiff(hunkCount: number, linesPerHunk: number): FileDiff[] {
  const files: FileDiff[] = [];
  const hunks: DiffHunk[] = [];
  for (let h = 0; h < hunkCount; h++) {
    const lines: DiffLine[] = [];
    let oldNo = h * linesPerHunk + 1;
    let newNo = h * linesPerHunk + 1;
    for (let l = 0; l < linesPerHunk; l++) {
      const phase = l % 5;
      if (phase === 0 || phase === 3) {
        lines.push({
          old_no: oldNo++,
          new_no: newNo++,
          origin: " ",
          text: `context line ${h}:${l} with some padding text`,
          highlights: [],
        });
      } else if (phase === 1 || phase === 4) {
        lines.push({
          old_no: oldNo++,
          new_no: null,
          origin: "-",
          text: `removed line ${h}:${l} with slightly longer content`,
          highlights: [[8, 12]],
        });
      } else {
        lines.push({
          old_no: null,
          new_no: newNo++,
          origin: "+",
          text: `added line ${h}:${l} with slightly longer content`,
          highlights: [[6, 11]],
        });
      }
    }
    hunks.push({ old_start: h * linesPerHunk + 1, new_start: h * linesPerHunk + 1, lines });
  }
  const perFile = Math.ceil(hunkCount / 100);
  for (let f = 0; f < 100; f++) {
    files.push({
      path: `src/big/file${f}.txt`,
      old_path: null,
      binary: false,
      is_image: false,
      additions: linesPerHunk,
      deletions: linesPerHunk,
      hunks: hunks.slice(f * perFile, (f + 1) * perFile),
    });
  }
  return files;
}

describe("perf gate (10k hunks / 200k rows)", () => {
  const HUNKS = 10_000;
  const LINES = 20;

  it("builds and slices 200k rows within budget", () => {
    const files = generateBigDiff(HUNKS, LINES);

    const tModel0 = performance.now();
    const model = buildRowModel(files, "split", new Set());
    const tModel = performance.now() - tModel0;

    const tHeights0 = performance.now();
    const heights = rowHeights(model.rows);
    const tHeights = performance.now() - tHeights0;

    const tLayout0 = performance.now();
    const layout = buildLayout(heights);
    const tLayout = performance.now() - tLayout0;

    // Row count sanity: 100 file headers + 1 hunk header per hunk + the
    // split-paired body. The generator's pattern pairs each '-' with a '+'
    // (8 aligned pairs + 8 single deletions + 8 context rows = 16/hunk),
    // so ~170k rows — right in the 200k-row ballpark the gate targets.
    expect(model.rows.length).toBeGreaterThanOrEqual(HUNKS * 17);
    expect(model.rows.length).toBeLessThanOrEqual(HUNKS * (LINES + 2));
    expect(layout.totalRows).toBe(model.rows.length);
    expect(layout.totalHeight).toBeGreaterThan(0);
    // Buckets stay proportional to runs (~2 per hunk), not to rows.
    expect(layout.buckets.length).toBeLessThan(HUNKS * 4);

    // The hot path: totalHeight read + one visibleSlices per scroll frame,
    // across a representative sweep of scroll positions.
    const ITER = 200;
    const tSlice0 = performance.now();
    let windowMax = 0;
    for (let i = 0; i < ITER; i++) {
      const scrollTop = (layout.totalHeight / ITER) * i;
      const slices = visibleSlices(layout, scrollTop, 800, 400);
      const rows = slices.reduce((acc, s) => acc + s.count, 0);
      windowMax = Math.max(windowMax, rows);
      if (layout.totalHeight <= 0) throw new Error("unreachable");
    }
    const tSlice = performance.now() - tSlice0;
    const perCall = tSlice / ITER;

    // Only the visible window (± overscan) is ever materialized.
    expect(windowMax).toBeLessThan(200);

    console.info(
      `[diff perf] rows=${layout.totalRows} buckets=${layout.buckets.length} ` +
        `totalHeight=${layout.totalHeight}px | rowModel=${tModel.toFixed(2)}ms ` +
        `rowHeights=${tHeights.toFixed(2)}ms buildLayout=${tLayout.toFixed(2)}ms | ` +
        `totalHeight+visibleSlices x${ITER}=${tSlice.toFixed(2)}ms ` +
        `(avg ${perCall.toFixed(3)}ms/call, max window ${windowMax} rows)`,
    );

    // Spec budget: <2ms per frame; loose CI assert at 50ms for the whole loop.
    expect(tSlice).toBeLessThan(50);

    // Row-model build is a one-time cost per mode/collapse change; keep it
    // generous to avoid CI flake while still catching order-of-magnitude breaks.
    expect(tModel + tHeights + tLayout).toBeLessThan(500);
  });

  it("keeps slices consistent with rowTop at random positions", () => {
    const layout: VLayout = buildLayout(rowHeights(buildRowModel(generateBigDiff(500, 20), "split", new Set()).rows));
    for (let i = 0; i < 50; i++) {
      const scrollTop = Math.random() * layout.totalHeight;
      for (const slice of visibleSlices(layout, scrollTop, 600, 100)) {
        expect(rowTop(layout, slice.firstRow)).toBeCloseTo(slice.top, 10);
      }
    }
  });
});
