/**
 * History lane index structures — `LogIndex` and log filtering.
 *
 * Everything here is plain, synchronous and side-effect free (no runes, no
 * IPC) so it can be unit tested directly; the reactive wrapper lives in
 * `./history.svelte` and the UI in `$lib/components/panels/HistoryView`.
 */

import type { CommitInfo, GraphRow, LogFilter, LogPage } from "$lib/ipc/types";
import { dayToUnixSeconds } from "$lib/git/format";

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

// ---------------------------------------------------------------------------
// Restored-selection resolution
// ---------------------------------------------------------------------------

/** Outcome of the selection-restore machine (`resolveRestoredSelection`). */
export type RestoredSelection =
  /** The commit reached the loaded log at this row index. */
  | { kind: "resolved"; index: number }
  /** The stream may still deliver the commit — keep the selection parked. */
  | { kind: "pending" }
  /** The stream exhausted without the commit — drop the phantom selection. */
  | { kind: "dropped" };

/**
 * Pure decision core of the restored-selection machine: a hydrated
 * (per-root persisted) selection resolves once its commit reaches the loaded
 * log; a commit that never appears (outside the restored filter, gc'd, …)
 * is dropped ONLY once the stream has exhausted (`!loading && no buffered
 * pages`), so a still-arriving stream is never mistaken for "missing".
 *
 * Side effects stay with the caller: the store wraps this with the
 * page-release step (releasing buffered pages while the commit may sit
 * beyond the loaded window) and the view applies the view-state writes
 * (detail info, multi-select mirror, one-time scroll, phantom drop).
 */
export function resolveRestoredSelection(
  index: LogIndex,
  targetSha: string,
  streamExhausted: boolean,
): RestoredSelection {
  const at = index.indexOf(targetSha);
  if (at >= 0) return { kind: "resolved", index: at };
  return streamExhausted ? { kind: "dropped" } : { kind: "pending" };
}
