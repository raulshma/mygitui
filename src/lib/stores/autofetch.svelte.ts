/**
 * Auto-fetch store (Svelte 5 runes) — per-repo background fetch intervals.
 *
 * Interval minutes live in localStorage under `mygitui.autofetch` as
 * `{ [repoId]: minutes }` (`0` = off). `start(repoId)` is called when a tab
 * opens (RepoView `$effect`): with an interval > 0 it resolves the default
 * remote (`origin`, else the first configured remote) and fetches with
 * prune on that cadence. Toasts fire only on error or when the fetch
 * actually updated refs — a silent up-to-date fetch stays silent.
 * `stop(repoId)` clears the timer on tab close.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client`; the scheduler
 * (`setInterval`/`clearInterval`) and storage are injectable for tests.
 */

import { fetchRepo, remotes } from "$lib/ipc/client";
import type { NetStats, RepoId } from "$lib/ipc/types";
import { toast } from "$lib/toast";

/** localStorage key: `{ [repoId]: minutes }`. */
export const AUTOFETCH_STORAGE_KEY = "mygitui.autofetch";

/** Minimal storage surface this store needs (subset of DOM `Storage`). */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** Injectable scheduler (tests use fake timers through these). */
export interface Scheduler {
  setInterval(handler: () => void, ms: number): unknown;
  clearInterval(handle: unknown): void;
}

const defaultScheduler: Scheduler = {
  setInterval: (handler, ms) => setInterval(handler, ms),
  clearInterval: (handle) => clearInterval(handle as ReturnType<typeof setInterval>),
};

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    // Accessing localStorage can throw (privacy modes / sandboxes).
    return null;
  }
}

/** Parses persisted config; drops malformed entries. */
function parseConfig(raw: string | null): Record<string, number> {
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (typeof parsed !== "object" || parsed === null) return {};
  const out: Record<string, number> = {};
  for (const [repoId, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (typeof value === "number" && Number.isFinite(value) && value >= 0) {
      out[repoId] = value;
    }
  }
  return out;
}

interface RunningFetch {
  timer: unknown;
  remote: string;
}

export class AutofetchStore {
  /** Configured interval minutes per repo (0 = off); persisted on change. */
  config: Record<RepoId, number> = $state({});

  #storage: StorageLike | null;
  #scheduler: Scheduler;
  #running = new Map<RepoId, RunningFetch>();

  constructor(
    storage: StorageLike | null = defaultStorage(),
    scheduler: Scheduler = defaultScheduler,
  ) {
    this.#storage = storage;
    this.#scheduler = scheduler;
    this.config = parseConfig(storage?.getItem(AUTOFETCH_STORAGE_KEY) ?? null);
  }

  /** Interval minutes for a repo (0 = off). */
  getInterval(repoId: RepoId): number {
    return this.config[repoId] ?? 0;
  }

  /** True when auto-fetch is currently running for the repo. */
  isRunning(repoId: RepoId): boolean {
    return this.#running.has(repoId);
  }

  /**
   * Sets (and persists) the interval; `0` disables. A running repo restarts
   * immediately with the new cadence.
   */
  setInterval(repoId: RepoId, minutes: number): void {
    const safe = Number.isFinite(minutes) && minutes > 0 ? Math.floor(minutes) : 0;
    const config = { ...this.config };
    if (safe === 0) delete config[repoId];
    else config[repoId] = safe;
    this.config = config;
    this.#persist();
    if (this.#running.has(repoId)) {
      this.stop(repoId);
      if (safe > 0) void this.start(repoId);
    }
  }

  /**
   * Starts the background fetch for a repo whose interval is > 0 (no-op
   * otherwise, and when it is already running). The default remote is
   * `origin`, else the first configured remote, else the literal fallback
   * "origin" (the fetch will error-toast if the repo has none).
   */
  async start(repoId: RepoId): Promise<void> {
    if (this.#running.has(repoId)) return;
    const minutes = this.getInterval(repoId);
    if (minutes <= 0) return;

    const remote = await this.#defaultRemote(repoId);
    // Config may have changed while the remote lookup was in flight.
    if (this.getInterval(repoId) <= 0 || this.#running.has(repoId)) return;

    const tick = (): void => {
      fetchRepo(repoId, { remote, prune: true, refs: [], depth: null })
        .then((stats: NetStats) => {
          if (stats.updated_refs.length > 0) {
            toast(`${remote} updated: ${stats.updated_refs.length} refs`);
          }
        })
        .catch((err: unknown) => {
          toast(
            `Auto-fetch failed: ${err instanceof Error ? err.message : String(err)}`,
            { kind: "error" },
          );
        });
    };

    const timer = this.#scheduler.setInterval(tick, minutes * 60_000);
    this.#running.set(repoId, { timer, remote });
  }

  /** Stops the background fetch for a repo (tab close). No-op when off. */
  stop(repoId: RepoId): void {
    const running = this.#running.get(repoId);
    if (!running) return;
    this.#running.delete(repoId);
    this.#scheduler.clearInterval(running.timer);
  }

  /** Stops every running interval (test helper / teardown). */
  stopAll(): void {
    for (const repoId of [...this.#running.keys()]) this.stop(repoId);
  }

  async #defaultRemote(repoId: RepoId): Promise<string> {
    try {
      const list = await remotes(repoId);
      return list.find((r) => r.name === "origin")?.name ?? list[0]?.name ?? "origin";
    } catch {
      return "origin";
    }
  }

  #persist(): void {
    if (!this.#storage) return;
    try {
      this.#storage.setItem(AUTOFETCH_STORAGE_KEY, JSON.stringify(this.config));
    } catch {
      // Storage may be full or unavailable; keep the in-memory config usable.
    }
  }
}

/** The application-wide auto-fetch store. */
export const autofetch = new AutofetchStore();
