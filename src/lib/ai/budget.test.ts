/**
 * Unit tests for the AI usage budget (item 58): the month window start
 * (same-month, cross-month, reset-day clamping in short months), the
 * spent→percent/level view math (soft budget — "over" warns, never
 * blocks), the label, and the month-to-date aggregation over the job
 * history store (the exact aggregation the StatusBar chip performs).
 */

import { describe, expect, it } from "vitest";
import { AiJobStore } from "./jobs.svelte";
import { budgetLabel, budgetMonthStart, budgetView } from "./budget";

function memStorage(initial: Record<string, string> = {}) {
  const data = { ...initial };
  return {
    getItem: (key: string) => data[key] ?? null,
    setItem: (key: string, value: string) => {
      data[key] = value;
    },
    removeItem: (key: string) => {
      delete data[key];
    },
    dump: () => ({ ...data }),
  };
}

describe("budgetMonthStart", () => {
  // Fixed instants built with LOCAL-time constructors — the helper returns
  // local midnight of the reset day, so expectations must use the same
  // calendar (Date.UTC would drift by the test machine's TZ offset).
  const nov18 = new Date(2026, 10, 18, 12, 0, 0).getTime();
  const nov1 = new Date(2026, 10, 1, 0, 0, 0).getTime();

  it("returns the 1st of the current month for the default reset day", () => {
    expect(budgetMonthStart(nov18, 1)).toBe(nov1);
  });

  it("returns this month's reset day when it has passed", () => {
    const nov15 = new Date(2026, 10, 15, 0, 0, 0).getTime();
    expect(budgetMonthStart(nov18, 15)).toBe(nov15);
    // The reset instant itself belongs to the new window.
    expect(budgetMonthStart(nov15, 15)).toBe(nov15);
  });

  it("reaches back into the previous month before the reset day", () => {
    const nov10 = new Date(2026, 10, 10, 0, 0, 0).getTime();
    expect(budgetMonthStart(nov10, 15)).toBe(new Date(2026, 9, 15, 0, 0, 0).getTime());
    // Year boundary (reset day 15, now Jan 3rd 2027 → back to Dec 2026).
    const jan3 = new Date(2027, 0, 3, 0, 0, 0).getTime();
    expect(budgetMonthStart(jan3, 15)).toBe(new Date(2026, 11, 15, 0, 0, 0).getTime());
  });

  it("clamps the reset day into each month's real length (31 → last day)", () => {
    // November has 30 days: reset day 31 fires on Nov 30, so while Nov 29
    // is still inside, the current window is the one opened Oct 31.
    const nov29 = new Date(2026, 10, 29, 0, 0, 0).getTime();
    expect(budgetMonthStart(nov29, 31)).toBe(new Date(2026, 9, 31, 0, 0, 0).getTime());
    // On the clamped reset day itself the new window opens.
    const nov30 = new Date(2026, 10, 30, 0, 0, 0).getTime();
    expect(budgetMonthStart(nov30, 31)).toBe(nov30);
    // February 2026 (28 days), reset day 31, now Mar 1 → window opened Feb 28.
    const mar1 = new Date(2026, 2, 1, 0, 0, 0).getTime();
    expect(budgetMonthStart(mar1, 31)).toBe(new Date(2026, 1, 28, 0, 0, 0).getTime());
  });

  it("tolerates out-of-range reset days", () => {
    expect(budgetMonthStart(nov18, 0)).toBe(nov1);
    // 99 clamps to 31 → same "last day of the month" behavior as above:
    // before Nov 30, the window opened Oct 31.
    expect(budgetMonthStart(nov18, 99)).toBe(new Date(2026, 9, 31, 0, 0, 0).getTime());
    expect(budgetMonthStart(nov18, 1.9)).toBe(nov1); // floor
  });
});

describe("budgetView (threshold rendering logic)", () => {
  it("reads null percent/level when no budget is configured", () => {
    expect(budgetView(5000, 0)).toEqual({ budget: 0, spent: 5000, percent: null, level: null });
    expect(budgetView(5000, -3)).toEqual({ budget: -3, spent: 5000, percent: null, level: null });
    expect(budgetView(5000, Number.NaN).percent).toBeNull();
  });

  it("computes the percent and the over/ok threshold", () => {
    expect(budgetView(420, 1000)).toEqual({ budget: 1000, spent: 420, percent: 42, level: "ok" });
    expect(budgetView(0, 1000).percent).toBe(0);
    // 100% exactly is already the warning state.
    expect(budgetView(1000, 1000)).toEqual({ budget: 1000, spent: 1000, percent: 100, level: "over" });
    expect(budgetView(2413, 1000)).toEqual({ budget: 1000, spent: 2413, percent: 241, level: "over" });
  });

  it("floors the percent and never goes negative on spent", () => {
    expect(budgetView(1499, 1000).percent).toBe(149);
    expect(budgetView(-5, 1000).percent).toBe(0);
  });

  it("formats the status-bar label", () => {
    expect(budgetLabel(42)).toBe("42% of monthly budget");
    expect(budgetLabel(100)).toBe("100% of monthly budget");
  });
});

describe("month-to-date aggregation (the chip's data path)", () => {
  it("sums usageSince(budgetMonthStart) across the history ring", () => {
    // Deterministic clock: begin() stamps startedAt from the store's now.
    let now = 0;
    const store = new AiJobStore({ storage: memStorage(), now: () => (now += 1000) });
    const run = (tokens: number): void => {
      const ticket = store.begin({ repoId: "r1", feature: "chat" });
      ticket.usage({ totalTokens: tokens });
      ticket.done({
        backend: "opencode",
        model: "m",
        elapsedMs: 10,
        usage: { totalTokens: tokens },
        text: null,
      });
    };
    run(100); // startedAt 1000
    run(250); // startedAt 2000
    run(50); // startedAt 3000

    expect(store.usageSince(0).totalTokens).toBe(400);
    expect(store.usageSince(1500).totalTokens).toBe(300);
    expect(store.usageSince(3500).totalTokens).toBe(0);
  });
});
