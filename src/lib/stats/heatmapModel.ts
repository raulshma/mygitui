/**
 * Pure heatmap model (M7 stats lane) — turns backend `DayCount[]` buckets
 * into a GitHub-style contribution grid plus streak/totals summaries.
 *
 * No runes, no DOM, no IPC: fully unit-testable, exactly like
 * `statusModel.ts` / `panelModel.ts`. The reactive wrapper lives in
 * `./statsStore.svelte.ts`, the UI in StatsPanel.svelte.
 *
 * Grid contract (GitHub-style):
 *   - columns are WEEKS, rows are weekdays (0 = Sunday … 6 = Saturday);
 *   - the LAST column is the current week and ends on `today` (cells after
 *     today are `null` placeholders so every column has 7 rows);
 *   - the default window is 53 weeks (~1 year); days before the window
 *     start are `null` too;
 *   - intensity levels are 0 (no commits) + 4 quartile buckets computed
 *     from the NONZERO day counts (low / medium / high / max).
 */

import type { DayCount } from "$lib/ipc/client";

// ---------------------------------------------------------------------------
// Date helpers (all pure, UTC-based, "YYYY-MM-DD" strings)
// ---------------------------------------------------------------------------

const ISO_RE = /^(\d{4})-(\d{2})-(\d{2})$/;

export const MONTH_ABBREVS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

/** Parses "YYYY-MM-DD" → days since 1970-01-01; `null` when malformed. */
export function daysSinceEpoch(iso: string): number | null {
  const match = ISO_RE.exec(iso);
  if (!match) return null;
  const ms = Date.UTC(
    Number(match[1]),
    Number(match[2]) - 1,
    Number(match[3]),
  );
  if (Number.isNaN(ms)) return null;
  const days = Math.floor(ms / 86_400_000);
  // Strict round-trip: Date.UTC rolls over out-of-range components
  // (2026-13-40 would normalize into February 2027), so only accept the
  // value when it re-encodes to the exact same ISO string.
  return dayFromNumber(days) === iso ? days : null;
}

/** Formats days-since-epoch back to "YYYY-MM-DD" (UTC civil date). */
export function dayFromNumber(days: number): string {
  const date = new Date(days * 86_400_000);
  const y = date.getUTCFullYear();
  const m = String(date.getUTCMonth() + 1).padStart(2, "0");
  const d = String(date.getUTCDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

/** Weekday of a day number: 0 = Sunday … 6 = Saturday (1970-01-01 = Thu). */
export function weekdayOfNumber(days: number): number {
  return (((days + 4) % 7) + 7) % 7;
}

/** Today as "YYYY-MM-DD" (the only wall-clock read in this module). */
export function todayISO(): string {
  const now = new Date();
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, "0");
  const d = String(now.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

// ---------------------------------------------------------------------------
// Intensity buckets (quartiles over the nonzero counts)
// ---------------------------------------------------------------------------

/** Intensity thresholds `[q1, q2, q3]`; `null` thresholds are level 1..3. */
export type Thresholds = [number, number, number];

/**
 * Quartile thresholds from a list of day counts (zeros ignored — they are
 * level 0 by definition). Nearest-rank on the sorted nonzero values; with
 * no data the fallback `[1, 2, 3]` makes every commit "low".
 */
export function intensityThresholds(counts: number[]): Thresholds {
  const nonzero = counts.filter((c) => c > 0).sort((a, b) => a - b);
  if (nonzero.length === 0) return [1, 2, 3];
  const at = (p: number): number =>
    nonzero[Math.min(nonzero.length - 1, Math.floor(p * (nonzero.length - 1)))];
  return [at(0.25), at(0.5), at(0.75)];
}

/** Heatmap intensity 0 (none) … 4 (max) for one day count. */
export function levelFor(count: number, thresholds: Thresholds): 0 | 1 | 2 | 3 | 4 {
  if (count <= 0) return 0;
  if (count <= thresholds[0]) return 1;
  if (count <= thresholds[1]) return 2;
  if (count <= thresholds[2]) return 3;
  return 4;
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

/** One heatmap cell; `day: null` marks outside-window / future slots. */
export interface HeatmapCell {
  day: string | null;
  count: number;
  level: 0 | 1 | 2 | 3 | 4;
}

/** Month label pinned to a week column. */
export interface MonthLabel {
  /** Week (grid column) index. */
  week: number;
  /** Short month name ("Jan" … "Dec"). */
  label: string;
}

export interface HeatmapGrid {
  /** `[week][weekday]`, weekday 0 = Sunday; every column has 7 cells. */
  weeks: HeatmapCell[][];
  monthLabels: MonthLabel[];
  thresholds: Thresholds;
}

export interface BuildHeatmapOptions {
  /** Today, "YYYY-MM-DD" (defaults to the real clock — inject in tests). */
  today?: string;
  /** Window length in week columns (default 53, min 1). */
  weeks?: number;
}

/**
 * Builds the contribution grid from backend buckets. Days with zero
 * commits need not be present in `dayCounts`; entries outside the window
 * are ignored.
 */
export function buildHeatmap(
  dayCounts: DayCount[],
  options: BuildHeatmapOptions = {},
): HeatmapGrid {
  const today = options.today ?? todayISO();
  const todayNum = daysSinceEpoch(today);
  if (todayNum === null) throw new TypeError(`malformed today: "${today}"`);
  const weekCount = Math.max(1, Math.floor(options.weeks ?? 53));

  // Total covered days: full weeks plus the trailing partial week.
  const trailing = weekdayOfNumber(todayNum) + 1; // Sun..today
  const totalDays = (weekCount - 1) * 7 + trailing;
  const startNum = todayNum - totalDays + 1;

  // Only in-window entries matter: the grid (and thus the quartile
  // thresholds) must reflect the displayed period, never older data.
  const counts = new Map<string, number>();
  for (const entry of dayCounts) {
    if (typeof entry?.day !== "string" || entry.count <= 0) continue;
    const num = daysSinceEpoch(entry.day);
    if (num !== null && num >= startNum && num <= todayNum) {
      counts.set(entry.day, entry.count);
    }
  }
  const thresholds = intensityThresholds([...counts.values()]);

  const weeks: HeatmapCell[][] = [];
  let lastMonth: string | null = null;
  const monthLabels: MonthLabel[] = [];
  for (let week = 0; week < weekCount; week += 1) {
    const column: HeatmapCell[] = [];
    for (let dow = 0; dow < 7; dow += 1) {
      const dayNum = startNum + week * 7 + dow;
      const day = dayNum >= startNum && dayNum <= todayNum ? dayFromNumber(dayNum) : null;
      const count = day !== null ? (counts.get(day) ?? 0) : 0;
      column.push({ day, count, level: levelFor(count, thresholds) });
    }
    // Month label: month of the column's first in-window day, emitted when
    // it changes from the previous column's (the first column always
    // labels, so months never disappear between partial columns).
    const firstDay = column.find((cell) => cell.day !== null);
    if (firstDay?.day) {
      const month = MONTH_ABBREVS[Number(firstDay.day.slice(5, 7)) - 1];
      if (month !== lastMonth) {
        monthLabels.push({ week, label: month });
        lastMonth = month;
      }
    }
    weeks.push(column);
  }

  return { weeks, monthLabels, thresholds };
}

// ---------------------------------------------------------------------------
// Streaks + totals
// ---------------------------------------------------------------------------

export interface Streaks {
  /** Consecutive active days ending today (or yesterday if today is idle). */
  current: number;
  /** Longest run of consecutive active days in the data. */
  longest: number;
}

/** Current + longest streaks from the day buckets (active = count > 0). */
export function streaks(dayCounts: DayCount[], today?: string): Streaks {
  const active = new Set<number>();
  for (const entry of dayCounts) {
    if (entry.count > 0) {
      const num = daysSinceEpoch(entry.day);
      if (num !== null) active.add(num);
    }
  }
  if (active.size === 0) return { current: 0, longest: 0 };

  // Current: walk back from today; an idle today defers to yesterday
  // (GitHub semantics — this morning's idleness must not wipe the streak).
  let current = 0;
  if (today !== undefined) {
    const todayNum = daysSinceEpoch(today);
    if (todayNum !== null) {
      let cursor = active.has(todayNum) ? todayNum : todayNum - 1;
      while (active.has(cursor)) {
        current += 1;
        cursor -= 1;
      }
    }
  }

  // Longest: sort the active days once, count the longest gap-free run.
  const sorted = [...active].sort((a, b) => a - b);
  let longest = 1;
  let run = 1;
  for (let i = 1; i < sorted.length; i += 1) {
    run = sorted[i] === sorted[i - 1] + 1 ? run + 1 : 1;
    longest = Math.max(longest, run);
  }

  return { current, longest };
}

export interface Totals {
  /** Sum of all commits in the window. */
  commits: number;
  /** Days with at least one commit. */
  activeDays: number;
  /** Average commits per ACTIVE day (1 decimal); 0 without data. */
  avgPerDay: number;
}

/** Window totals for the summary cards. */
export function totals(dayCounts: DayCount[]): Totals {
  let commits = 0;
  let activeDays = 0;
  for (const entry of dayCounts) {
    if (entry.count > 0) {
      commits += entry.count;
      activeDays += 1;
    }
  }
  const avgPerDay =
    activeDays === 0 ? 0 : Math.round((commits / activeDays) * 10) / 10;
  return { commits, activeDays, avgPerDay };
}
