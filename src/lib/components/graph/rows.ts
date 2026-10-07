/**
 * Incremental flattening of cumulative `LogPage[]` into one row list.
 *
 * `pages` (owned by the parent, e.g. HistoryView) only ever GROWS while a
 * generation is live: each streamed page is appended. Re-flattening 250k rows
 * on every scroll frame would blow the 60fps budget, so `update()` finds the
 * common page prefix by reference and appends only the new pages. A replaced
 * prefix (generation reset, filter change, array shrink) triggers exactly one
 * full rebuild.
 */

import type { CommitInfo, GraphRow, LogPage } from "$lib/ipc/types";

export interface RowCacheStats {
  /** Full rebuilds (prefix replaced / shrunk — includes the first build). */
  rebuilds: number;
  /** Pages consumed by the incremental fast path. */
  appendedPages: number;
}

export class RowCache {
  readonly rows: GraphRow[] = [];
  readonly commits: CommitInfo[] = [];
  readonly stats: RowCacheStats = { rebuilds: 0, appendedPages: 0 };

  #source: readonly LogPage[] | null = null;
  #pageRefs: readonly LogPage[] = [];
  #indexBySha: Map<string, number> | null = null;
  #indexCoverage = 0;

  get length(): number {
    return this.rows.length;
  }

  /**
   * Sync with the latest `pages` array. Returns true when the flattened row
   * list changed. Safe to call on every `pages` reassignment; never call per
   * scroll frame (the parent reassigns `pages` only when data arrives).
   */
  update(pages: readonly LogPage[]): boolean {
    if (pages === this.#source) return false;

    const previous = this.#pageRefs;
    let common = 0;
    const maxCommon = Math.min(previous.length, pages.length);
    while (common < maxCommon && pages[common] === previous[common]) common += 1;

    let changed = false;
    if (common !== previous.length) {
      // Prefix replaced (or list shrank): one full rebuild.
      this.rows.length = 0;
      this.commits.length = 0;
      this.#indexBySha = null;
      this.#indexCoverage = 0;
      for (const page of pages) this.#append(page);
      this.stats.rebuilds += 1;
      changed = this.rows.length > 0 || previous.length > 0;
    } else {
      for (let i = common; i < pages.length; i += 1) {
        this.#append(pages[i]);
        this.stats.appendedPages += 1;
        changed = true;
      }
    }
    this.#source = pages;
    this.#pageRefs = pages.slice();
    return changed;
  }

  /** Commit metadata for a row (rows and commits are index-aligned). */
  commitAt(index: number): CommitInfo | null {
    return index >= 0 && index < this.commits.length ? this.commits[index] : null;
  }

  /**
   * Row index for a sha (-1 if unknown). The sha→index map is built lazily
   * (only if this is ever called) and extended incrementally per append.
   */
  indexOfSha(sha: string): number {
    if (!sha) return -1;
    let index = this.#indexBySha;
    if (!index) {
      index = new Map<string, number>();
      this.#indexBySha = index;
      this.#indexCoverage = 0;
    }
    if (this.#indexCoverage < this.rows.length) {
      for (let i = this.#indexCoverage; i < this.rows.length; i += 1) {
        index.set(this.rows[i].sha, i);
      }
      this.#indexCoverage = this.rows.length;
    }
    const hit = index.get(sha);
    return hit === undefined ? -1 : hit;
  }

  /** Drop everything (test helper). */
  clear(): void {
    this.rows.length = 0;
    this.commits.length = 0;
    this.#source = null;
    this.#pageRefs = [];
    this.#indexBySha = null;
    this.#indexCoverage = 0;
  }

  #append(page: LogPage): void {
    // Rows and commits are parallel arrays per the IPC contract; trust the
    // shorter one if a page ever arrives misaligned.
    const n = Math.min(page.rows.length, page.commits.length);
    for (let i = 0; i < n; i += 1) {
      this.rows.push(page.rows[i]);
      this.commits.push(page.commits[i]);
    }
  }
}
