/**
 * Pure logic for the history lane (B3): ref classification, date formatting,
 * `LogFilter` serialization and the incremental log flatten cache.
 *
 * Everything here is plain, synchronous and side-effect free (no runes, no
 * IPC) so it can be unit tested directly; the reactive wrapper lives in
 * `./history.svelte` and the UI in `$lib/components/panels/HistoryView`.
 */

import type { CommitInfo, GraphRow, LogFilter, LogPage } from "$lib/ipc/types";

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
// Filter state → LogFilter serialization
// ---------------------------------------------------------------------------

/** Editable filter-bar fields (strings, as typed; dates are `YYYY-MM-DD`). */
export interface HistoryFilterFields {
  text: string;
  /** When true, `text` is a regex (M9). */
  regex: boolean;
  author: string;
  path: string;
  after: string;
  before: string;
  /** Pickaxe -S: patches must add or remove this string (M10). */
  pickaxe: string;
  /**
   * Pickaxe -G: patches must match this regex (M12). Optional (default "")
   * so persisted older filter literals stay type-compatible.
   */
  pickaxeRegex?: string;
}

export const EMPTY_FILTER: HistoryFilterFields = {
  text: "",
  regex: false,
  author: "",
  path: "",
  after: "",
  before: "",
  pickaxe: "",
  pickaxeRegex: "",
};

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

/**
 * Serializes filter-bar fields into the exact backend `LogFilter` shape.
 * `refs` stays a filter-bar non-concern; `follow` marks a file-history walk
 * (rename-following, single path). `pickaxe_regex` is only emitted when set
 * (keeps the serialized default shape identical to the M10 contract, and
 * `-S`/`-G` may both be present — the backend intersects them).
 */
export function toLogFilter(
  fields: HistoryFilterFields,
  opts: { follow?: boolean } = {},
): LogFilter {
  const pickaxeRegex = fields.pickaxeRegex?.trim() || null;
  return {
    text: fields.text.trim() || null,
    regex: fields.regex,
    author: fields.author.trim() || null,
    path: fields.path.trim() || null,
    after_unix: dayToUnixSeconds(fields.after, "start"),
    before_unix: dayToUnixSeconds(fields.before, "end"),
    refs: [],
    follow: opts.follow ?? false,
    pickaxe: fields.pickaxe.trim() || null,
    ...(pickaxeRegex !== null ? { pickaxe_regex: pickaxeRegex } : {}),
  };
}

// ---------------------------------------------------------------------------
// Incremental flatten cache
// ---------------------------------------------------------------------------

/** Cached flattened view: commits and their index-aligned graph rows. */
export interface FlatLog {
  commits: CommitInfo[];
  rows: GraphRow[];
}

/**
 * Incrementally flattened log — the store's derived-commits cache.
 *
 * `append` is strictly incremental (per-page pushes; the arrays are never
 * rebuilt), so appending is O(page size) regardless of log size: appending
 * 100 pages × 500 commits stays in the low-millisecond range (tested — the
 * hot path deliberately avoids hashing every sha; see the dedupe note).
 *
 * Dedupe: an incoming page may resend the tail of the previous one, so only
 * the head of each page is checked against the kept tail (plain string
 * compares). Anything duplicated deeper inside a page is a backend bug and
 * passes through.
 *
 * Rows are matched to commits by sha — index-aligned fast path, sha-map
 * fallback, synthesized single-lane row as last resort — so `rows[i]` always
 * corresponds to `commits[i]`.
 *
 * `indexOf`/`has` use a sha→index map that is materialized lazily on first
 * query and then extended incrementally, keeping `append` free of per-commit
 * hashing (50k `Map.set`s of 40-char shas cost ~12ms — more than the whole
 * append budget).
 */
export class LogIndex {
  /** Commits that can overlap at a page boundary (kept tail). */
  static readonly TAIL_SIZE = 16;
  /** How many leading commits of an incoming page to dedupe-check. */
  static readonly BOUNDARY_CHECK = 64;

  readonly commits: CommitInfo[] = [];
  readonly rows: GraphRow[] = [];
  #tail: string[] = [];
  #index: Map<string, number> | null = null;
  #indexedUpTo = 0;

  get size(): number {
    return this.commits.length;
  }

  /** Index of a sha in the flattened commit list, or -1. */
  indexOf(sha: string): number {
    return this.#ensureIndex().get(sha) ?? -1;
  }

  /** Whether the sha is already in the flattened list. */
  has(sha: string): boolean {
    return this.#ensureIndex().has(sha);
  }

  /** Appends one page; returns how many commits were actually added. */
  append(page: Pick<LogPage, "commits" | "rows">): number {
    const pageCommits = page.commits;
    const pageRows = page.rows;

    // Boundary dedupe (see class doc): skip the leading run of shas that
    // repeats the tail of the previous append.
    let skip = 0;
    const head = Math.min(LogIndex.BOUNDARY_CHECK, pageCommits.length);
    while (skip < head && this.#tail.includes(pageCommits[skip]?.sha ?? "")) {
      skip++;
    }

    let added: number;
    if (pageRows.length === pageCommits.length) {
      // Fast path — the backend contract aligns rows[i] with commits[i], so
      // bulk-append (spread push keeps this allocation-loop free; a per-
      // commit loop costs ~10× more on slow machines).
      if (skip === 0) {
        this.commits.push(...pageCommits);
        this.rows.push(...pageRows);
      } else {
        this.commits.push(...pageCommits.slice(skip));
        this.rows.push(...pageRows.slice(skip));
      }
      added = pageCommits.length - skip;
    } else {
      // Defensive slow path: rows missing/extra — match by sha, synthesize
      // a single-lane row when there is none for a commit.
      const bySha = new Map(pageRows.map((r) => [r.sha, r]));
      for (let i = skip; i < pageCommits.length; i++) {
        const commit = pageCommits[i] as CommitInfo;
        const row = bySha.get(commit.sha);
        this.commits.push(commit);
        this.rows.push(row ?? { sha: commit.sha, lane: 0, edges: [], lane_count: 1 });
      }
      added = pageCommits.length - skip;
    }

    const total = this.commits.length;
    this.#tail = this.commits
      .slice(Math.max(0, total - LogIndex.TAIL_SIZE), total)
      .map((c) => c.sha);
    return added;
  }

  /** Clears the cache (arrays keep their identity). */
  reset(): void {
    this.commits.length = 0;
    this.rows.length = 0;
    this.#tail = [];
    this.#index = null;
    this.#indexedUpTo = 0;
  }

  /** Materializes/extends the lazy sha→index map up to the current size. */
  #ensureIndex(): Map<string, number> {
    if (this.#index === null) this.#index = new Map();
    const index = this.#index;
    while (this.#indexedUpTo < this.commits.length) {
      index.set(
        (this.commits[this.#indexedUpTo] as CommitInfo).sha,
        this.#indexedUpTo,
      );
      this.#indexedUpTo++;
    }
    return index;
  }
}
