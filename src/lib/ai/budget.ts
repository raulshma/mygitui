/**
 * AI usage budget (item 58) — pure helpers behind the soft monthly token
 * budget: the month window start, the spent→percent/level math and the
 * status-bar label. No store, no IO, so the StatusBar chip, the settings
 * hint and the tests all render the same numbers.
 *
 * SOFT BUDGET, by design: at 100% jobs are still allowed — the launcher
 * UI only turns the usage chip into a warning. A hard block would hold
 * the user's work hostage to a stale local number.
 *
 * VISIBLE-HISTORY CAVEAT, by design: month-to-date spend aggregates the
 * job history ring (`ai.jobs.history`, newest 200 records kept —
 * `AiJobStore.JOB_HISTORY_CAP`, persisted under `mygitui.ai.activity`).
 * Runs that rotated out of the ring, runs on other machines, and runs
 * whose backend did not report usage are NOT counted, so the percentage
 * is a local lower bound, never an authoritative bill. The settings hint
 * says so.
 */

/** The month-to-date window start for a budget resetting on `resetDay`. */
export function budgetMonthStart(nowMs: number, resetDay: number): number {
  const day = Math.min(Math.max(Math.floor(resetDay), 1), 31);
  const now = new Date(nowMs);
  // Date(y, m, day) rolls overflowing days into the next month (Jan 31 +
  // month=1 → Mar 3), so the day is clamped into the month's real length
  // — a reset day of 31 acts as "last day of the month" in shorter months.
  const daysInMonth = (y: number, m: number): number => new Date(y, m + 1, 0).getDate();
  const y = now.getFullYear();
  const m = now.getMonth();
  const thisMonth = new Date(y, m, Math.min(day, daysInMonth(y, m))).getTime();
  if (nowMs >= thisMonth) return thisMonth;
  // Before this month's reset: the window opened last month.
  const prevM = m === 0 ? 11 : m - 1;
  const prevY = m === 0 ? y - 1 : y;
  return new Date(prevY, prevM, Math.min(day, daysInMonth(prevY, prevM))).getTime();
}

/** How the chip should read for a spent amount against the budget. */
export type BudgetLevel = "ok" | "over";

/** Derived chip state (`null` percent = no budget configured). */
export interface BudgetView {
  /** The configured budget in tokens (> 0 when usable). */
  budget: number;
  /** Month-to-date tokens (local visible history). */
  spent: number;
  /** 0–100+ rounded percent; `null` when the budget is not set. */
  percent: number | null;
  /** "over" from exactly 100% up (the warning chip; nothing hard-blocks). */
  level: BudgetLevel | null;
}

/** Builds the chip state: no/invalid budget → everything `null`. */
export function budgetView(spent: number, budget: number): BudgetView {
  if (!Number.isFinite(budget) || budget <= 0) {
    return { budget, spent, percent: null, level: null };
  }
  const percent = Math.floor((Math.max(0, spent) / budget) * 100);
  return { budget, spent, percent, level: percent >= 100 ? "over" : "ok" };
}

/** The status-bar chip label: "42% of monthly budget". */
export function budgetLabel(percent: number): string {
  return `${percent}% of monthly budget`;
}
