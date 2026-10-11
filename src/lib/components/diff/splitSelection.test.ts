/**
 * Unit tests for the split-view selection model (splitSelection.ts): the
 * per-row segment geometry DiffViewer paints with the Custom Highlight API.
 * Pure functions — no DOM.
 */

import { describe, expect, it } from "vitest";
import {
  segmentText,
  selectionSegments,
  wordSpanAt,
  type HalfLength,
} from "./splitSelection";

/** Four rows; the pair filler on row 2 has no half on the tracked side. */
const TEXTS = ["line-0-abcdef", "line-1-abcdef", "line-2-skipped", "line-3-abcdef"];
const lengths: HalfLength = (row) => (row === 2 ? null : TEXTS[row]!.length);

describe("selectionSegments", () => {
  it("one forward row yields a single partial segment", () => {
    expect(selectionSegments({ row: 1, char: 3 }, { row: 1, char: 8 }, lengths)).toEqual([
      { row: 1, start: 3, end: 8 },
    ]);
  });

  it("spans whole rows between partial endpoints", () => {
    expect(
      selectionSegments({ row: 0, char: 6 }, { row: 3, char: 2 }, lengths),
    ).toEqual([
      { row: 0, start: 6, end: 13 },
      { row: 1, start: 0, end: 13 },
      { row: 3, start: 0, end: 2 },
    ]);
  });

  it("is order-agnostic (drag up vs down)", () => {
    const down = selectionSegments({ row: 0, char: 1 }, { row: 1, char: 4 }, lengths);
    const up = selectionSegments({ row: 1, char: 4 }, { row: 0, char: 1 }, lengths);
    expect(up).toEqual(down);
  });

  it("same-row drag normalizes to the span between the points", () => {
    expect(selectionSegments({ row: 0, char: 7 }, { row: 0, char: 2 }, lengths)).toEqual([
      { row: 0, start: 2, end: 7 },
    ]);
  });

  it("skips rows without a half on the side (pair fillers)", () => {
    expect(selectionSegments({ row: 1, char: 0 }, { row: 3, char: 3 }, lengths)).toEqual([
      { row: 1, start: 0, end: 13 },
      { row: 3, start: 0, end: 3 },
    ]);
  });

  it("clamps points past the end of their line", () => {
    expect(selectionSegments({ row: 0, char: 99 }, { row: 0, char: 5 }, lengths)).toEqual([
      { row: 0, start: 5, end: 13 },
    ]);
  });
});

describe("segmentText", () => {
  const text = (row: number) => TEXTS[row]!;

  it("joins row slices with newlines, skipping filler rows", () => {
    const segs = selectionSegments({ row: 0, char: 5 }, { row: 2, char: 7 }, lengths);
    expect(segmentText(segs, text)).toBe("0-abcdef\nline-1-abcdef");
  });

  it("emits nothing for an empty selection", () => {
    expect(segmentText([], text)).toBe("");
  });
});

describe("wordSpanAt", () => {
  const line = "const alpha_beta = gamma12 + x";

  it("expands to the full word around the offset", () => {
    expect(wordSpanAt(line, 8)).toEqual([6, 16]); // alpha_beta
    expect(wordSpanAt(line, 0)).toEqual([0, 5]); // const
    expect(wordSpanAt(line, line.length - 1)).toEqual([line.length - 1, line.length]); // x
  });

  it("yields an empty span on whitespace or out of range", () => {
    expect(wordSpanAt(line, 5)).toEqual([0, 0]); // space
    expect(wordSpanAt(line, -1)).toEqual([0, 0]);
    expect(wordSpanAt(line, line.length)).toEqual([0, 0]);
    expect(wordSpanAt("", 0)).toEqual([0, 0]);
  });
});
