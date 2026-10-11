/**
 * Shared git-related formatting helpers. Pure and stateless.
 */

// ---------------------------------------------------------------------------
// Ref classification (CommitInfo.refs decorations)
// ---------------------------------------------------------------------------

/** Display category of a commit decoration. */
export type RefKind = "local" | "remote" | "tag" | "head";

/**
 * Classifies a ref decoration into a chip category.
 *
 * Handles both fully-qualified (`refs/heads/main`, `refs/remotes/origin/main`,
 * `refs/tags/v1.0`) and plain git-log style names (`main`, `origin/main`,
 * `tag: v2.0`, `HEAD -> main`). Heuristic: anything containing `/` and no
 * `HEAD` is remote-ish; `HEAD` anywhere wins (HEAD pointer is not a branch).
 */
export function classifyRef(ref: string): RefKind {
  const r = ref.trim();
  if (r.includes("HEAD")) return "head";
  if (r.startsWith("refs/tags/") || r.startsWith("tag:")) return "tag";
  if (r.startsWith("refs/remotes/")) return "remote";
  if (r.startsWith("refs/heads/")) return "local";
  if (r.includes("/")) return "remote";
  return "local";
}

/** Strips ref prefixes for compact chip display (`refs/heads/main` → `main`). */
export function shortRefName(ref: string): string {
  let r = ref.trim();
  if (r.startsWith("HEAD -> ")) r = r.slice("HEAD -> ".length);
  else if (r === "HEAD") return "HEAD";
  r = r.replace(/^tag:\s*/, "");
  for (const prefix of ["refs/heads/", "refs/remotes/", "refs/tags/"]) {
    if (r.startsWith(prefix)) return r.slice(prefix.length);
  }
  return r;
}

// ---------------------------------------------------------------------------
// Stable hash hue (blame shas, ref accents)
// ---------------------------------------------------------------------------

/** Deterministic hue (0–359) for a string — stable across sessions/mounts. */
export function hashHue(key: string): number {
  let h = 0;
  for (let i = 0; i < key.length; i++) {
    h = (Math.imul(31, h) + key.charCodeAt(i)) | 0;
  }
  return Math.abs(h) % 360;
}

// ---------------------------------------------------------------------------
// Date formatting
// ---------------------------------------------------------------------------

/** Options for {@link formatRelativeTime}. */
export interface RelativeTimeOptions {
  /** `Date.now()/1000` stand-in (tests). Default: now. */
  now?: number;
  /** BCP-47 locale; `undefined` = runtime default. Tests pin `"en"`. */
  locale?: string;
}

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** `Intl.RelativeTimeFormat` with auto bucketing: "now", "3 days ago", … */
export function formatRelativeTime(
  unixSeconds: number,
  opts: RelativeTimeOptions = {},
): string {
  const now = opts.now ?? Math.floor(Date.now() / 1000);
  const rtf = new Intl.RelativeTimeFormat(opts.locale, { numeric: "auto" });
  const diff = unixSeconds - now;
  const abs = Math.abs(diff);
  if (abs < MINUTE) return rtf.format(diff, "second");
  if (abs < HOUR) return rtf.format(Math.round(diff / MINUTE), "minute");
  if (abs < DAY) return rtf.format(Math.round(diff / HOUR), "hour");
  if (abs < 7 * DAY) return rtf.format(Math.round(diff / DAY), "day");
  if (abs < 30 * DAY) return rtf.format(Math.round(diff / (7 * DAY)), "week");
  if (abs < 365 * DAY) return rtf.format(Math.round(diff / (30 * DAY)), "month");
  return rtf.format(Math.round(diff / (365 * DAY)), "year");
}

/** Absolute `Intl.DateTimeFormat` (system locale/timezone unless pinned). */
export function formatDateTime(
  unixSeconds: number,
  locale?: string,
  opts?: Intl.DateTimeFormatOptions,
): string {
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
    ...opts,
  }).format(new Date(unixSeconds * 1000));
}

// ---------------------------------------------------------------------------
// Date-input conversion
// ---------------------------------------------------------------------------

const DAY_RE = /^\d{4}-\d{2}-\d{2}$/;

/**
 * Converts a date-input string to unix seconds at the given day edge.
 * Days are interpreted as UTC (`Date.parse("YYYY-MM-DD")` semantics) so the
 * value is timezone-independent; `end` adds 23:59:59. `null` when empty or
 * malformed.
 */
export function dayToUnixSeconds(
  day: string,
  edge: "start" | "end",
): number | null {
  if (!DAY_RE.test(day)) return null;
  const ms = Date.parse(`${day}T00:00:00Z`);
  if (Number.isNaN(ms)) return null;
  // Round-trip: engines roll out-of-range days ("2026-02-30" → Mar 2nd).
  if (new Date(ms).toISOString().slice(0, 10) !== day) return null;
  return Math.floor(ms / 1000) + (edge === "end" ? 86_399 : 0);
}
