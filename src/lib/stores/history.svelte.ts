/**
 * History store (Svelte 5 runes) — owns the streamed commit log for one
 * repository view: pages, the flattened/derived commit+row cache, filter
 * state, loading/generation/error flags and the repo-changed restart.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * `HistoryView` creates one instance per mounted view (one per repo tab) and
 * calls `start(repoId)` in an `$effect` / `destroy()` in its cleanup. All
 * backend access goes through `$lib/ipc/client` (mocked with `vi.mock` in
 * tests). Pure helpers (dedupe/flatten, filter serialization) live in
 * `./history-logic` and are tested there.
 *
 * Streaming model: the backend pushes `LogPage`s of up to 500 commits over
 * one channel until the log is exhausted; a new `repo_log_stream` for the
 * same repo cancels the previous one server-side, so "cancel" here means
 * invalidating stale callbacks (generation tokens) rather than an IPC call.
 * Arriving pages land in a pending buffer and are released to the UI in
 * chunks of {@link RELEASE_CHUNK_COMMITS} commits — `loadMore()` (wired to
 * GraphCanvas `onReachEnd`) releases the next chunk, which keeps huge repos
 * from handing the canvas/DOM list the entire history at once.
 */

import { onRepoChanged, streamLog } from "$lib/ipc/client";
import type { RepoChangedEvent } from "$lib/ipc/client";
import type { LogPage, RepoId } from "$lib/ipc/types";
import {
  EMPTY_FILTER,
  LogIndex,
  toLogFilter,
  type FlatLog,
  type HistoryFilterFields,
} from "./history-logic";

/** Debounce for filter edits before the stream restarts (ms). */
export const FILTER_DEBOUNCE_MS = 200;

/** How many commits each release window hands to the UI at once. */
export const RELEASE_CHUNK_COMMITS = 1_000;

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export class HistoryStore {
  /** Released `LogPage`s — the source GraphCanvas renders from. */
  pages: LogPage[] = $state([]);
  /**
   * Flattened commits + index-aligned graph rows (derived cache). Replaced by
   * a fresh wrapper object per release so rune readers re-run; the inner
   * arrays are reused (appended in place — never reflattened from scratch).
   */
  flat: FlatLog = $state({ commits: [], rows: [] });
  /** Filter-bar state (typed strings; serialized via `toLogFilter`). */
  filter: HistoryFilterFields = $state({ ...EMPTY_FILTER });
  /** True while a `repo_log_stream` request is in flight. */
  loading = $state(false);
  /** Latest backend generation accepted from a page (bumps on repo change). */
  generation = $state(0);
  /** Last stream failure message, if any. */
  error: string | null = $state(null);
  /** Pages buffered but not yet released to `pages`/`flat`. */
  pendingCount = $state(0);

  readonly #index = new LogIndex();
  /** Rename-following walk (file-history mode). */
  readonly #follow: boolean;
  #pending: LogPage[] = [];
  #releasedCommits = 0;
  #releaseCap = RELEASE_CHUNK_COMMITS;
  #repoId: RepoId | null = null;
  #started = false;
  #token = 0;
  #acceptedGen = 0;
  #debounceTimer: ReturnType<typeof setTimeout> | null = null;
  #unlisten: (() => void) | null = null;

  /**
   * `opts.follow` marks a file-history store: the walk follows the path in
   * `filter.path` through renames (backend `LogFilter.follow`).
   */
  constructor(opts: { follow?: boolean } = {}) {
    this.#follow = opts.follow ?? false;
  }

  /** Repo this store is bound to, or null before `start`/after `destroy`. */
  get repoId(): RepoId | null {
    return this.#repoId;
  }

  /** Whether buffered pages remain that `loadMore()` can release. */
  get hasMore(): boolean {
    return this.pendingCount > 0;
  }

  /** Total flattened commits currently released to the UI. */
  get count(): number {
    return this.#index.size;
  }

  /** Index of a sha in the flattened list, or -1 (e.g. parent beyond window). */
  indexOfSha(sha: string): number {
    return this.#index.indexOf(sha);
  }

  /**
   * Starts (or re-targets) the stream for `repoId`. Idempotent for the same
   * repo; switching repos resets everything and restarts. Also subscribes the
   * repo-changed watcher that restarts the stream when the backend bumps its
   * generation.
   */
  start(repoId: RepoId): void {
    if (this.#started && this.#repoId === repoId) return;
    if (this.#repoId !== repoId) {
      this.#acceptedGen = 0;
      this.generation = 0;
      this.#resetState();
    }
    this.#started = true;
    this.#repoId = repoId;
    void this.#subscribeWatcher();
    this.#beginStream();
  }

  /**
   * Updates filter fields and schedules a debounced stream restart (200ms —
   * rapid typing coalesces into one restart). The backend cancels the prior
   * stream itself when the new request arrives.
   */
  setFilter(patch: Partial<HistoryFilterFields>): void {
    this.filter = { ...this.filter, ...patch };
    this.#scheduleRestart();
  }

  /** True when any filter field is set (drives the "clear" button). */
  get filterActive(): boolean {
    const f = this.filter;
    return Boolean(f.text || f.author || f.path || f.after || f.before);
  }

  /** Clears all filter fields and restarts immediately-ish (debounced). */
  clearFilter(): void {
    this.filter = { ...EMPTY_FILTER };
    this.#scheduleRestart();
  }

  /**
   * Releases the next chunk of buffered pages (GraphCanvas `onReachEnd`).
   * No-op when nothing is buffered — the stream may still deliver more.
   */
  loadMore(): void {
    if (this.#pending.length === 0) return;
    this.#releaseCap += RELEASE_CHUNK_COMMITS;
    this.#release();
    this.pendingCount = this.#pending.length;
  }

  /** Manual restart (e.g. an error toast's "Retry" button). */
  restart(): void {
    this.#beginStream();
  }

  /**
   * Releases buffered pages until `sha` is present (for parent navigation to
   * a commit beyond the current window). Returns whether it was found.
   */
  revealSha(sha: string): boolean {
    while (this.#index.indexOf(sha) < 0 && this.#pending.length > 0) {
      this.loadMore();
    }
    return this.#index.indexOf(sha) >= 0;
  }

  /**
   * Stops the store: invalidates in-flight streams and timers, unsubscribes
   * the watcher and clears all state. Safe to call repeatedly; `start` may be
   * called again afterwards.
   */
  destroy(): void {
    this.#token++;
    this.#clearDebounce();
    try {
      this.#unlisten?.();
    } catch {
      // Ignore unlisten failures.
    }
    this.#unlisten = null;
    this.#started = false;
    this.#repoId = null;
    this.#acceptedGen = 0;
    this.generation = 0;
    this.#resetState();
  }

  // -- internals ------------------------------------------------------------

  #beginStream(): void {
    const repoId = this.#repoId;
    if (!this.#started || !repoId) return;
    this.#clearDebounce();
    const token = ++this.#token;
    this.#index.reset();
    this.#pending = [];
    this.#releasedCommits = 0;
    this.#releaseCap = RELEASE_CHUNK_COMMITS;
    this.pages = [];
    this.flat = { commits: this.#index.commits, rows: this.#index.rows };
    this.loading = true;
    this.error = null;
    this.pendingCount = 0;

    try {
      streamLog(repoId, toLogFilter(this.filter, { follow: this.#follow }), (page) =>
        this.#onPage(token, page),
      )
        .then(() => {
          if (this.#token === token) this.loading = false;
        })
        .catch((err: unknown) => {
          if (this.#token === token) {
            this.loading = false;
            this.error = errorMessage(err);
          }
        });
    } catch (err) {
      // Synchronous throw from a (mocked) client — treat like a rejection.
      this.loading = false;
      this.error = errorMessage(err);
    }
  }

  #onPage(token: number, page: LogPage): void {
    if (token !== this.#token) return; // stale stream (we restarted)
    if (page.generation < this.#acceptedGen) return; // stale backend stream
    if (page.generation > this.#acceptedGen) {
      this.#acceptedGen = page.generation;
      this.generation = page.generation;
    }
    this.#pending.push(page);
    this.#release();
    this.pendingCount = this.#pending.length;
  }

  #release(): void {
    let released = false;
    while (
      this.#pending.length > 0 &&
      this.#releasedCommits < this.#releaseCap
    ) {
      const page = this.#pending.shift() as LogPage;
      this.pages.push(page);
      this.#index.append(page);
      this.#releasedCommits += page.commits.length;
      released = true;
    }
    if (released) {
      // Cheap wrapper swap (same inner arrays) → rune readers re-run.
      this.flat = { commits: this.#index.commits, rows: this.#index.rows };
    }
  }

  #scheduleRestart(): void {
    this.#clearDebounce();
    this.#debounceTimer = setTimeout(() => {
      this.#debounceTimer = null;
      this.#beginStream();
    }, FILTER_DEBOUNCE_MS);
  }

  #clearDebounce(): void {
    if (this.#debounceTimer !== null) {
      clearTimeout(this.#debounceTimer);
      this.#debounceTimer = null;
    }
  }

  async #subscribeWatcher(): Promise<void> {
    try {
      this.#unlisten?.();
    } catch {
      // Ignore unlisten failures.
    }
    this.#unlisten = null;
    const repoId = this.#repoId;
    if (!repoId) return;
    try {
      this.#unlisten = await onRepoChanged((event: RepoChangedEvent) => {
        if (!this.#started || event.repo_id !== this.#repoId) return;
        // Generation-bumped restart: fresh stream, stale pages ignored.
        this.#beginStream();
      });
    } catch {
      this.#unlisten = null; // never fatal (non-Tauri envs no-op already)
    }
  }

  #resetState(): void {
    this.#index.reset();
    this.#pending = [];
    this.#releasedCommits = 0;
    this.#releaseCap = RELEASE_CHUNK_COMMITS;
    this.pages = [];
    this.flat = { commits: this.#index.commits, rows: this.#index.rows };
    this.loading = false;
    this.error = null;
    this.pendingCount = 0;
  }
}

