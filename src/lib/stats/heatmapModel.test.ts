/**
 * Unit tests for the pure heatmap model (M7 stats lane). No DOM, no IPC:
 * `today` is always injected so the suite is clock-independent.
 */

import { describe, expect, it } from "vitest";
import type { DayCount } from "$lib/ipc/client";
import {
  buildHeatmap,
  dayFromNumber,
  daysSinceEpoch,
  intensityThresholds,
  levelFor,
  streaks,
  todayISO,
  totals,
  weekdayOfNumber,
} from "./heatmapModel";

const day = (day: string, count: number): DayCount => ({ day, count });

// ---------------------------------------------------------------------------
// date helpers
// ---------------------------------------------------------------------------

describe("date helpers", () => {
  it("round-trips ISO dates through day numbers", () => {
    expect(daysSinceEpoch("1970-01-01")).toBe(0);
    expect(daysSinceEpoch("2024-01-01")).toBe(19_723);
    expect(daysSinceEpoch("2026-10-07")).toBe(daysSinceEpoch("2026-10-07"));
    expect(dayFromNumber(0)).toBe("1970-01-01");
    expect(dayFromNumber(19_723)).toBe("2024-01-01");
    expect(daysSinceEpoch("nope")).toBeNull();
    expect(daysSinceEpoch("2026-13-40")).toBeNull();
  });

  it("maps weekdays with Sunday = 0 (1970-01-01 was a Thursday)", () => {
    expect(weekdayOfNumber(0)).toBe(4);
    const jan4 = daysSinceEpoch("1970-01-04") ?? -1;
    const jan3 = daysSinceEpoch("1970-01-03") ?? -1;
    expect(weekdayOfNumber(jan4)).toBe(0);
    expect(weekdayOfNumber(jan3)).toBe(6);
    // Negative day numbers (pre-1970) stay in range too.
    expect(weekdayOfNumber(-1)).toBe(3);
  });

  it("todayISO produces a parseable date", () => {
    expect(daysSinceEpoch(todayISO())).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// intensity buckets
// ---------------------------------------------------------------------------

describe("intensityThresholds / levelFor", () => {
  it("falls back to [1,2,3] without data", () => {
    expect(intensityThresholds([])).toEqual([1, 2, 3]);
    expect(intensityThresholds([0, 0])).toEqual([1, 2, 3]);
  });

  it("computes quartiles over the nonzero counts", () => {
    expect(intensityThresholds([1, 2, 3, 4])).toEqual([1, 2, 3]);
    expect(intensityThresholds([5])).toEqual([5, 5, 5]);
    // Zeros are ignored (they are level 0 by definition).
    expect(intensityThresholds([0, 0, 10, 20, 30, 40])).toEqual([10, 20, 30]);
    expect(intensityThresholds([4, 1, 3, 2])).toEqual([1, 2, 3]);
  });

  it("levels: 0 none, 1..3 up to the quartiles, 4 above", () => {
    const t = intensityThresholds([1, 2, 3, 4]);
    expect(levelFor(0, t)).toBe(0);
    expect(levelFor(1, t)).toBe(1);
    expect(levelFor(2, t)).toBe(2);
    expect(levelFor(3, t)).toBe(3);
    expect(levelFor(4, t)).toBe(4);
    expect(levelFor(99, t)).toBe(4);
  });
});

// ---------------------------------------------------------------------------
// grid
// ---------------------------------------------------------------------------

describe("buildHeatmap", () => {
  // 2026-10-07 is a Wednesday (weekday 3).
  const TODAY = "2026-10-07";

  it("ends the last column on today and pads the rest of the week", () => {
    const grid = buildHeatmap([], { today: TODAY, weeks: 2 });
    expect(grid.weeks).toHaveLength(2);
    for (const column of grid.weeks) expect(column).toHaveLength(7);

    const last = grid.weeks[1];
    expect(last[3]).toEqual({
      day: "2026-10-07",
      count: 0,
      level: 0,
    });
    expect(last[0]?.day).toBe("2026-10-04"); // Sunday of the current week
    expect(last.slice(4).every((c) => c.day === null)).toBe(true);

    // First column starts on the Sunday 10 days back (full week + 4 days).
    expect(grid.weeks[0][0]?.day).toBe("2026-09-27");
  });

  it("places counts on the right cells and ignores out-of-window days", () => {
    const grid = buildHeatmap(
      [day("2026-10-06", 3), day("2026-09-28", 2), day("2020-01-01", 99)],
      { today: TODAY, weeks: 2 },
    );
    // 2026-10-06 (Tuesday) = last column, row 2.
    expect(grid.weeks[1][2]).toMatchObject({ day: "2026-10-06", count: 3 });
    // 2026-09-28 (Monday) = first column, row 1.
    expect(grid.weeks[0][1]).toMatchObject({ day: "2026-09-28", count: 2 });
    // 2020 is far outside the 2-week window: dropped entirely.
    const all = grid.weeks.flat();
    expect(all.every((c) => c.count !== 99)).toBe(true);
    // Thresholds computed from the in-window nonzero counts only ([2,3] →
    // every quartile floors to index 0 → [2,2,2]; so 3 commits is "max").
    expect(grid.thresholds).toEqual([2, 2, 2]);
    expect(grid.weeks[1][2]?.level).toBe(4);
    expect(grid.weeks[0][1]?.level).toBe(1); // count 2 ≤ q1 → "low"
  });

  it("labels months at column changes (short names)", () => {
    const grid = buildHeatmap([], { today: TODAY, weeks: 2 });
    expect(grid.monthLabels).toEqual([
      { week: 0, label: "Sep" },
      { week: 1, label: "Oct" },
    ]);
    // A single-week grid has exactly one label.
    expect(buildHeatmap([], { today: TODAY, weeks: 1 }).monthLabels).toEqual([
      { week: 0, label: "Oct" },
    ]);
  });

  it("spans a full default year (53 columns) ending on today", () => {
    const grid = buildHeatmap([], { today: TODAY });
    expect(grid.weeks).toHaveLength(53);
    const first = grid.weeks[0][0]?.day;
    const last = grid.weeks[52][3]?.day;
    const firstNum = daysSinceEpoch(first ?? "") ?? 0;
    const lastNum = daysSinceEpoch(last ?? "") ?? 0;
    expect(lastNum - firstNum).toBe(52 * 7 + 3);
  });

  it("throws on a malformed injected today", () => {
    expect(() => buildHeatmap([], { today: "not-a-date" })).toThrow(TypeError);
  });
});

// ---------------------------------------------------------------------------
// streaks + totals
// ---------------------------------------------------------------------------

describe("streaks", () => {
  const TODAY = "2026-10-07"; // Wednesday

  it("counts the current streak through today", () => {
    const s = streaks(
      [day("2026-10-05", 1), day("2026-10-06", 2), day("2026-10-07", 3)],
      TODAY,
    );
    expect(s).toEqual({ current: 3, longest: 3 });
  });

  it("defers an idle today to yesterday (GitHub semantics)", () => {
    const s = streaks(
      [
        day("2026-10-02", 1),
        day("2026-10-03", 1),
        day("2026-10-04", 1),
        day("2026-10-06", 1),
      ],
      TODAY,
    );
    // Today is idle but yesterday (Tue 6th) is active; the Fri..Sun run is
    // the longest.
    expect(s).toEqual({ current: 1, longest: 3 });
  });

  it("reports zero without activity or gaps in the far past", () => {
    expect(streaks([], TODAY)).toEqual({ current: 0, longest: 0 });
    expect(streaks([day("2020-01-01", 5)], TODAY)).toEqual({
      current: 0,
      longest: 1,
    });
  });

  it("finds the longest run anywhere in the data", () => {
    const s = streaks(
      [
        day("2026-09-01", 1),
        day("2026-09-02", 1),
        day("2026-09-03", 1),
        day("2026-09-04", 1),
        day("2026-09-08", 1),
        day("2026-09-09", 1),
      ],
      TODAY,
    );
    expect(s.longest).toBe(4);
    expect(s.current).toBe(0);
  });
});

describe("totals", () => {
  it("sums commits, active days and the per-active-day average", () => {
    const t = totals([
      day("2026-10-01", 3),
      day("2026-10-02", 2),
      day("2026-10-03", 0), // idle day: not active, no commits
    ]);
    expect(t).toEqual({ commits: 5, activeDays: 2, avgPerDay: 2.5 });
  });

  it("is zero-safe without data", () => {
    expect(totals([])).toEqual({ commits: 0, activeDays: 0, avgPerDay: 0 });
  });

  it("rounds the average to one decimal", () => {
    expect(totals([day("2026-10-01", 1), day("2026-10-02", 1)]).avgPerDay).toBe(1);
    expect(totals([day("2026-10-01", 2), day("2026-10-02", 2)]).avgPerDay).toBe(2);
    expect(totals([day("2026-10-01", 10)]).avgPerDay).toBe(10);
    expect(totals([day("2026-10-01", 1), day("2026-10-02", 1), day("2026-10-03", 1)]).avgPerDay).toBe(1);
    // 4 commits over 3 active days = 1.333… → 1.3
    expect(
      totals([day("2026-10-01", 2), day("2026-10-02", 1), day("2026-10-03", 1)])
        .avgPerDay,
    ).toBe(1.3);
  });
});
