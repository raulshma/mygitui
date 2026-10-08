/**
 * Stats store (Svelte 5 runes) — M7 contribution statistics.
 *
 * Caches one {@link StatsEntry} per open repo: the commit-activity buckets
 * (heatmap input) and the contributor rollups (datalist + list) fetched via
 * `$lib/ipc/client` — the only backend access route. `load(repoId, opts)`
 * re-fetches for a different author filter / window and dedupes concurrent
 * identical fetches; failures are stored as `error` (previous data kept).
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

import { commitActivity, contributorStats } from "$lib/ipc/client";
import type { RepoId } from "$lib/ipc/types";
import type { Contributor, DayCount } from "$lib/ipc/client";

/** Default stats window in days (~1 year, GitHub-heatmap scale). */
export const DEFAULT_MAX_DAYS = 365;

/** Cached state for one repository. */
export interface StatsEntry {
  /** Commits per local day, day ascending (heatmap input). */
  activity: DayCount[];
  /** Contributor rollups, count descending (datalist + list). */
  contributors: Contributor[];
  loading: boolean;
  error: string | null;
  /** Author filter the cached activity was fetched with (null = all). */
  author: string | null;
  /** Window in days the cached data covers. */
  maxDays: number;
}

const EMPTY: StatsEntry = {
  activity: [],
  contributors: [],
  loading: false,
  error: null,
  author: null,
  maxDays: DEFAULT_MAX_DAYS,
};

export interface LoadStatsOptions {
  /** Window in days (default {@link DEFAULT_MAX_DAYS}, min 1). */
  maxDays?: number;
  /** Author substring filter (blank/undefined = all authors). */
  author?: string | null;
}

export class StatsStore {
  #byRepo: Record<string, StatsEntry> = $state({});
  /** In-flight loads keyed by `repo|window|author` (dedupes repeats). */
  #inflight = new Map<string, Promise<void>>();

  /** The raw entry for `repoId` (EMPTY when never loaded). */
  entry(repoId: RepoId): StatsEntry {
    return this.#byRepo[repoId] ?? EMPTY;
  }

  activity(repoId: RepoId): DayCount[] {
    return this.entry(repoId).activity;
  }

  contributors(repoId: RepoId): Contributor[] {
    return this.entry(repoId).contributors;
  }

  isLoading(repoId: RepoId): boolean {
    return this.entry(repoId).loading;
  }

  error(repoId: RepoId): string | null {
    return this.entry(repoId).error;
  }

  /**
   * Fetches (or joins an identical in-flight fetch of) the activity +
   * contributor rollups for `repoId`. Never throws: failures land in
   * `error` with the previous data kept.
   */
  load(repoId: RepoId, options: LoadStatsOptions = {}): Promise<void> {
    const maxDays = Math.max(1, Math.floor(options.maxDays ?? DEFAULT_MAX_DAYS));
    const trimmed = options.author?.trim();
    const author = trimmed ? trimmed : null;
    const key = `${repoId}|${maxDays}|${author ?? ""}`;
    const pending = this.#inflight.get(key);
    if (pending) return pending;

    this.#set(repoId, {
      ...this.entry(repoId),
      loading: true,
      error: null,
      author,
      maxDays,
    });
    const promise = Promise.all([
      commitActivity(repoId, maxDays, author),
      contributorStats(repoId, maxDays),
    ])
      .then(([activity, contributors]) => {
        this.#set(repoId, {
          activity,
          contributors,
          loading: false,
          error: null,
          author,
          maxDays,
        });
      })
      .catch((err: unknown) => {
        const message = err instanceof Error ? err.message : String(err);
        this.#set(repoId, {
          ...this.entry(repoId),
          loading: false,
          error: message,
        });
      })
      .finally(() => {
        this.#inflight.delete(key);
      });
    this.#inflight.set(key, promise);
    return promise;
  }

  /** Drops the cache for `repoId` (repo closed). */
  clear(repoId: RepoId): void {
    delete this.#byRepo[repoId];
  }

  /** Drops everything (test helper). */
  clearAll(): void {
    this.#byRepo = {};
  }

  #set(repoId: RepoId, entry: StatsEntry): void {
    this.#byRepo[repoId] = entry;
  }
}

/** The application-wide stats cache. */
export const statsStore = new StatsStore();
