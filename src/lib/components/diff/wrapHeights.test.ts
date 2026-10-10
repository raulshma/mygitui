/**
 * Unit tests for wrap-mode row heights (diff/wrapHeights.ts): analytic
 * wrapped-line counts (tab-aware), split pair max-side heights, fixed
 * heights for non-line rows, and the viewport → metrics math.
 */

import { describe, expect, it } from "vitest";
import type { DiffLine } from "$lib/ipc/types";
import { rowHeight } from "$lib/components/diff/rowModel";
import {
  GUTTER_PX,
  wrappedHeights,
  wrappedLineCount,
  wrappedRowHeight,
  wrapMetrics,
} from "$lib/components/diff/wrapHeights";

function line(origin: DiffLine["origin"], text: string): DiffLine {
  return { old_no: 1, new_no: 1, origin, text, highlights: [] };
}

// CHAR_W chosen so the math is readable: 10px/col.
const CHAR_W = 10;
const METRICS = {
  splitTextWidth: 400, // 40 columns per half
  unifiedTextWidth: 800, // 80 columns per row
  charWidth: CHAR_W,
};

describe("wrappedLineCount", () => {
  it("fits lines within the width onto one visual line", () => {
    expect(wrappedLineCount("short", 400, CHAR_W)).toBe(1);
    expect(wrappedLineCount("x".repeat(40), 400, CHAR_W)).toBe(1);
  });

  it("rounds up past the width", () => {
    expect(wrappedLineCount("x".repeat(41), 400, CHAR_W)).toBe(2);
    expect(wrappedLineCount("x".repeat(81), 400, CHAR_W)).toBe(3);
  });

  it("counts a tab by its stop advance", () => {
    // Tab to column 8, then 3 chars → 11 columns.
    expect(wrappedLineCount("\tabc", 10 * 11, CHAR_W)).toBe(1);
    expect(wrappedLineCount("\tabc", 10 * 10, CHAR_W)).toBe(2);
    expect(wrappedLineCount("\t".repeat(10), 10 * 80, CHAR_W)).toBe(1); // 80 cols
    expect(wrappedLineCount("\t".repeat(13), 10 * 80, CHAR_W)).toBe(2); // 104 cols
  });

  it("never returns zero (empty line still occupies one row)", () => {
    expect(wrappedLineCount("", 400, CHAR_W)).toBe(1);
  });

  it("degrades to one line at non-positive width", () => {
    expect(wrappedLineCount("x".repeat(100), 0, CHAR_W)).toBe(1);
    expect(wrappedLineCount("x".repeat(100), 400, 0)).toBe(1);
  });
});

describe("wrappedRowHeight", () => {
  it("uses unified width for single lines", () => {
    const row = { kind: "line" as const, line: line("+", "x".repeat(81)), fileIndex: 0, hunkIndex: 0, lineIndex: 0 };
    // 81 cols at 80 cols/line → 2 lines × 20px.
    expect(wrappedRowHeight(row, METRICS)).toBe(40);
  });

  it("uses split width for context rows", () => {
    const row = { kind: "context" as const, line: line(" ", "x".repeat(41)), fileIndex: 0, hunkIndex: 0, lineIndex: 0 };
    expect(wrappedRowHeight(row, METRICS)).toBe(40);
  });

  it("takes the taller side of a pair", () => {
    const row = {
      kind: "pair" as const,
      left: line("-", "x".repeat(41)), // 2 lines at 40 cols
      right: line("+", "y".repeat(10)), // 1 line
      fileIndex: 0,
      hunkIndex: 0,
      leftIndex: 0,
      rightIndex: 1,
    };
    expect(wrappedRowHeight(row, METRICS)).toBe(40);
  });

  it("pads null pair sides at one line", () => {
    const row = {
      kind: "pair" as const,
      left: null,
      right: line("+", "y"),
      fileIndex: 0,
      hunkIndex: 0,
      leftIndex: null,
      rightIndex: 0,
    };
    expect(wrappedRowHeight(row, METRICS)).toBe(20);
  });

  it("keeps non-line rows at their fixed heights", () => {
    expect(wrappedRowHeight({ kind: "binary" }, METRICS)).toBe(rowHeight({ kind: "binary" }));
  });
});

describe("wrappedHeights / wrapMetrics", () => {
  it("materializes one height per row", () => {
    const rows = [
      { kind: "file-header" as const, file: {} as never, fileIndex: 0, collapsed: false },
      { kind: "binary" as const },
    ];
    const heights = wrappedHeights(rows, METRICS);
    expect(heights).toHaveLength(2);
    expect(heights[1]).toBe(56);
  });

  it("derives usable text widths from the viewport", () => {
    const split = wrapMetrics(1001, "split");
    expect(split.splitTextWidth).toBe(Math.floor((1001 - 1) / 2) - GUTTER_PX.splitSide);
    expect(split.unifiedTextWidth).toBe(1001 - GUTTER_PX.unified);

    const unified = wrapMetrics(1001, "unified");
    expect(unified.splitTextWidth).toBe(0 - GUTTER_PX.splitSide);
  });
});
