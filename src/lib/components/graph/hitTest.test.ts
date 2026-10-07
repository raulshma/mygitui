/**
 * Unit tests for hit-test geometry (hitTest.ts): row resolution through the
 * virtual scroll offset, lane-column resolution with tolerance edges.
 */

import { describe, expect, it } from "vitest";
import { hitTest, laneAt, rowAt } from "$lib/components/graph/hitTest";

const LANE_COUNT = 3;
const rows = Array.from({ length: 10 }, () => ({ lane_count: LANE_COUNT }));

describe("rowAt", () => {
  it("resolves rows from canvas-local y via the scroll offset", () => {
    expect(rowAt(0, 0, 24, 10)).toBe(0);
    expect(rowAt(23.9, 0, 24, 10)).toBe(0);
    expect(rowAt(24, 0, 24, 10)).toBe(1);
    // scrollTop 480 = 20 rows down: canvas top maps to row 20.
    expect(rowAt(0, 480, 24, 10)).toBe(-1); // but the list only has 10 rows
    expect(rowAt(0, 96, 24, 10)).toBe(4);
    expect(rowAt(24, 96, 24, 10)).toBe(5);
  });

  it("returns -1 outside the row range or for degenerate heights", () => {
    expect(rowAt(-1, 0, 24, 10)).toBe(-1);
    expect(rowAt(240, 0, 24, 10)).toBe(-1); // y beyond the last row
    expect(rowAt(0, 0, 0, 10)).toBe(-1);
    expect(rowAt(0, 0, 24, 0)).toBe(-1);
  });
});

describe("laneAt", () => {
  // Defaults: laneWidth 14, padding 10 → lane columns at x = 10, 24, 38.
  it("hits the lane column center", () => {
    expect(laneAt(10, LANE_COUNT, 14, 10)).toBe(0);
    expect(laneAt(24, LANE_COUNT, 14, 10)).toBe(1);
    expect(laneAt(38, LANE_COUNT, 14, 10)).toBe(2);
  });

  it("accepts the position at the ±6px tolerance edge and rejects just past it", () => {
    expect(laneAt(16, LANE_COUNT, 14, 10)).toBe(0); // exactly 6px from lane 0
    expect(laneAt(17, LANE_COUNT, 14, 10)).toBeNull(); // 7px → miss
    expect(laneAt(4, LANE_COUNT, 14, 10)).toBe(0); // 6px below lane 0
    expect(laneAt(44, LANE_COUNT, 14, 10)).toBe(2); // 6px past lane 2
    expect(laneAt(45, LANE_COUNT, 14, 10)).toBeNull();
  });

  it("misses between adjacent lanes (tolerance zones do not overlap)", () => {
    // x = 17: 7px from lane 0 and 7px from lane 1 → no lane.
    expect(laneAt(17, LANE_COUNT, 14, 10)).toBeNull();
    expect(laneAt(0, LANE_COUNT, 14, 10)).toBeNull();
  });

  it("respects the row's lane_count at the right edge", () => {
    // Lane 2's column doesn't exist on a 2-lane row; 14px away from lane 1.
    expect(laneAt(38, 2, 14, 10)).toBeNull();
    // The last existing lane still gets its +6px tolerance.
    expect(laneAt(30, 2, 14, 10)).toBe(1);
    expect(laneAt(31, 2, 14, 10)).toBeNull();
  });

  it("returns null for rows with no lanes", () => {
    expect(laneAt(10, 0, 14, 10)).toBeNull();
  });

  it("honors a custom tolerance", () => {
    // Nearest-lane resolution with wider tolerance zones.
    expect(laneAt(20, LANE_COUNT, 14, 10, 10)).toBe(1); // 4px from lane 1
    expect(laneAt(0, LANE_COUNT, 14, 10, 10)).toBe(0); // exactly 10px left of lane 0
    expect(laneAt(-1, LANE_COUNT, 14, 10, 10)).toBeNull();
  });
});

describe("hitTest (combined)", () => {
  const base = {
    rows,
    rowHeight: 24,
    laneWidth: 14,
    padding: 10,
    tolerance: 6,
  };

  it("resolves a node hit: row from y+scrollTop, lane from x", () => {
    expect(hitTest({ ...base, x: 10, y: 12, scrollTop: 0 })).toEqual({ row: 0, lane: 0 });
    expect(hitTest({ ...base, x: 24, y: 60, scrollTop: 24 })).toEqual({ row: 3, lane: 1 });
    expect(hitTest({ ...base, x: 38, y: 0, scrollTop: 216 })).toEqual({ row: 9, lane: 2 });
  });

  it("reports the row with lane null when x misses every lane column", () => {
    expect(hitTest({ ...base, x: 200, y: 12, scrollTop: 0 })).toEqual({ row: 0, lane: null });
  });

  it("returns row -1 outside the list", () => {
    expect(hitTest({ ...base, x: 10, y: 500, scrollTop: 0 })).toEqual({ row: -1, lane: null });
    expect(hitTest({ ...base, x: 10, y: -5, scrollTop: 0 })).toEqual({ row: -1, lane: null });
  });

  it("reads the hit row's own lane_count", () => {
    const mixed = [{ lane_count: 1 }, { lane_count: 3 }];
    expect(hitTest({ ...base, rows: mixed, x: 24, y: 5, scrollTop: 0 })).toEqual({
      row: 0,
      lane: null, // row 0 only has lane 0 at x=10; x=24 is 14px off
    });
    expect(hitTest({ ...base, rows: mixed, x: 24, y: 30, scrollTop: 0 })).toEqual({
      row: 1,
      lane: 1,
    });
  });
});
