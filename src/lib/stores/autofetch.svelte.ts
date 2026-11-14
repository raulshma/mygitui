/**
 * Auto-fetch store (Svelte 5 runes) — per-repo background fetch intervals.
 *
 * Interval minutes live in localStorage under `mygitui.autofetch` as
 * `{ [root]: minutes }` (`0` = off), keyed by the canonical repo ROOT (or
 * the `ssh://` URI for remote tabs) — the same convention as the bookmarks
 * and branch-colors stores. Roots are stable across restarts; repo ids are
 * minted per session (`millis-pid-counter`), so keying by id (the original
 * scheme) never survived a relaunch. Load-time migration: keys that don't
 * look like a root path/URI are legacy ids and are dropped. Roots without
 * an entry follow the global default (`appSettings.defaultAutofetchMinutes`,
 * 0 = off).
 *
 * `start(repoId)` is called when a tab opens (RepoView `$effect`): with an
 * interval > 0 it resolves the repo's root (from the hint or the tab store)
 * plus the default remote (`origin`, else the first configured remote) and
 * fetches with prune on that cadence. Toasts fire only on error or when the
 * fetch actually updated refs — a silent up-to-date fetch stays silent.
 * `stop(repoId)` clears the timer on tab close. The scheduler API stays
 * repo-id keyed because timers belong to open tabs, not disk paths.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client`; the scheduler
 * (`setInterval`/`clearInterval`) and storage are injectable for tests.
 */

import { branches, fetchRepo, remotes } from "$lib/ipc/client";
import type { BranchInfo, NetStats, RepoId } from "$lib/ipc/types";
import { tabStore } from "$lib/stores/tabs.svelte";
import { toast } from "$lib/toast";
import { describeError } from "$lib/errors";
import { appSettings } from "$lib/state/appSettings.svelte";

/** localStorage key: `{ [root]: minutes }` (root-keyed since the re-key). */
export const AUTOFETCH_STORAGE_KEY = "mygitui.autofetch";

/** Max "upstream moved" toasts per fetch cycle (M12). */
export const UPSTREAM_MOVED_TOAST_CAP = 3;

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

/**
 * A persisted key is a repo root: a POSIX absolute path, a Windows drive/
 * UNC path, or an `ssh://` tab URI. Anything else is a legacy per-session
 * repo id — unrecognizable old keys are dropped (ids never survive restart,
 * so those settings were unreachable anyway).
 */
function looksLikeRoot(key: string): boolean {
  return (
    key.startsWith("/") ||
    key.startsWith("\\\\") ||
    key.startsWith("ssh://") ||
    /^[A-Za-z]:[\\/]/.test(key)
  );
}

/** Parses persisted config; drops malformed entries + legacy id keys. */
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
  for (const [root, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (!looksLikeRoot(root)) continue;
    if (typeof value === "number" && Number.isFinite(value) && value >= 0) {
      out[root] = value;
    }
  }
  return out;
}

interface RunningFetch {
  timer: unknown;
  remote: string;
  /** Persistence key (repo root) this repo's interval came from. */
  root: string;
}

/**
 * M12 upstream-moved detection (pure): given the previous cycle's
 * branch → behind counts and the fresh branch list, returns the messages to
 * toast — one per local branch that is behind its upstream (behind > 0,
 * upstream not gone) whose behind-count INCREASED or appeared since last
 * cycle, capped at {@link UPSTREAM_MOVED_TOAST_CAP} entries. Also returns
 * the current counts map to store for the next cycle.
 */
export function upstreamMoved(
  previous: ReadonlyMap<string, number>,
  list: readonly BranchInfo[],
): { messages: string[]; current: Map<string, number> } {
  const current = new Map<string, number>();
  const messages: string[] = [];
  for (const branch of list) {
    if (branch.behind <= 0 || branch.gone) continue;
    current.set(branch.name, branch.behind);
    const before = previous.get(branch.name);
    if (before === undefined || branch.behind > before) {
      messages.push(`upstream moved: ${branch.name} is ${branch.behind} behind`);
    }
  }
  return { messages: messages.slice(0, UPSTREAM_MOVED_TOAST_CAP), current };
}

export class AutofetchStore {
  /** Interval minutes per repo root (0 = off); persisted on change. */
  config: Record<string, number> = $state({});

  /**
   * In-flight background fetches per repo (1 = running). Reactive so the
   * header/status-bar sync controls can tell an auto-fetch apart from an
   * op the user started: a background tick must not make those controls
   * look busy (let alone lock them out for its duration).
   */
  fetching: Record<RepoId, number> = $state({});

  #storage: StorageLike | null;
  #scheduler: Scheduler;
  #running = new Map<RepoId, RunningFetch>();
  /**
   * Bumped by every start AND stop. `start` awaits its remote lookup before
   * it can register a timer, so a `stop` landing in that window has nothing
   * to clear yet — the token is the tombstone that makes it stick.
   */
  #startToken = new Map<RepoId, number>();
  /** M12: last cycle's branch → behind counts, per repo (dedupe baseline). */
  #lastBehind = new Map<RepoId, Map<string, number>>();

  constructor(
    storage: StorageLike | null = defaultStorage(),
    scheduler: Scheduler = defaultScheduler,
  ) {
    this.#storage = storage;
    this.#scheduler = scheduler;
    this.config = parseConfig(storage?.getItem(AUTOFETCH_STORAGE_KEY) ?? null);
  }

  /**
   * Persistence key for a repo: its canonical root. The tabs store owns the
   * id → root binding (ids are minted per session; roots are stable, like
   * the bookmarks/branch-colors keys). Falls back to the id itself when the
   * tab is already gone — the lookup then misses and the fetch stays off.
   */
  #rootFor(repoId: RepoId): string {
    return tabStore.tabs.find((tab) => tab.id === repoId)?.root ?? repoId;
  }

  /**
   * Interval minutes for a repo root (0 = off). An explicit per-repo
   * override wins; without one, the global default
   * (`appSettings.defaultAutofetchMinutes`, 0 = off) applies.
   */
  getInterval(root: string): number {
    return this.config[root] ?? appSettings.defaultAutofetchMinutes;
  }

  /** True when auto-fetch is currently running for the repo. */
  isRunning(repoId: RepoId): boolean {
    return this.#running.has(repoId);
  }

  /** True while a background fetch started by this store is in flight. */
  isFetching(repoId: RepoId): boolean {
    return (this.fetching[repoId] ?? 0) > 0;
  }

  #beginFetch(repoId: RepoId): void {
    this.fetching = {
      ...this.fetching,
      [repoId]: (this.fetching[repoId] ?? 0) + 1,
    };
  }

  #endFetch(repoId: RepoId): void {
    const count = (this.fetching[repoId] ?? 0) - 1;
    const next = { ...this.fetching };
    if (count > 0) next[repoId] = count;
    else delete next[repoId];
    this.fetching = next;
  }

  /**
   * Sets (and persists) the interval for a repo root; `0` disables. Any
   * running repo bound to this root restarts immediately with the new
   * cadence.
   */
  setInterval(root: string, minutes: number): void {
    const safe = Number.isFinite(minutes) && minutes > 0 ? Math.floor(minutes) : 0;
    const config = { ...this.config };
    if (safe === 0) delete config[root];
    else config[root] = safe;
    this.config = config;
    this.#persist();
    for (const [repoId, running] of [...this.#running]) {
      if (running.root !== root) continue;
      this.stop(repoId);
      // Restart with the recorded root — the tab may already be closing, so
      // a fresh tabs-store lookup could miss. Clearing an override (0) can
      // still leave a positive cadence: the global default applies then.
      if (this.getInterval(root) > 0) void this.start(repoId, running.root);
    }
  }

  /**
   * Starts the background fetch for a repo whose interval is > 0 (no-op
   * otherwise, and when it is already running). `rootHint` overrides the
   * tabs-store root lookup (tests inject it; production callers omit it).
   * The default remote is `origin`, else the first configured remote, else
   * the literal fallback "origin" (the fetch will error-toast if the repo
   * has none).
   */
  async start(repoId: RepoId, rootHint?: string): Promise<void> {
    if (this.#running.has(repoId)) return;
    const root = rootHint ?? this.#rootFor(repoId);
    const minutes = this.getInterval(root);
    if (minutes <= 0) return;

    const token = this.#bumpToken(repoId);
    const remote = await this.#defaultRemote(repoId);
    // Config may have changed while the remote lookup was in flight, and a
    // stop (or a newer start) may have superseded this one.
    if (this.#startToken.get(repoId) !== token) return;
    if (this.getInterval(root) <= 0 || this.#running.has(repoId)) return;

    const tick = (): void => {
      this.#beginFetch(repoId);
      fetchRepo(repoId, { remote, prune: true, refs: [], depth: null })
        .then((stats: NetStats) => {
          // A stop during the fetch must not toast for a closed repo.
          if (this.#startToken.get(repoId) !== token) return;
          if (stats.updated_refs.length > 0) {
            toast(`${remote} updated: ${stats.updated_refs.length} refs`);
            // M12: after refs moved, check which local branches fell behind
            // their upstream and toast the deltas (capped, deduped).
            void this.reportUpstreamMoved(repoId);
          }
        })
        .catch((err: unknown) => {
          if (this.#startToken.get(repoId) !== token) return;
          toast(
            `Auto-fetch failed: ${describeError(err)}`,
            { kind: "error" },
          );
        })
        .finally(() => this.#endFetch(repoId));
    };

    const timer = this.#scheduler.setInterval(tick, minutes * 60_000);
    this.#running.set(repoId, { timer, remote, root });
  }

  /** Stops the background fetch for a repo (tab close). No-op when off. */
  stop(repoId: RepoId): void {
    // Tombstone first: a start still awaiting its remote lookup must not
    // register a timer after this returns.
    this.#bumpToken(repoId);
    const running = this.#running.get(repoId);
    if (!running) return;
    this.#running.delete(repoId);
    this.#lastBehind.delete(repoId);
    this.#scheduler.clearInterval(running.timer);
  }

  #bumpToken(repoId: RepoId): number {
    const next = (this.#startToken.get(repoId) ?? 0) + 1;
    this.#startToken.set(repoId, next);
    return next;
  }

  /**
   * M12: after a fetch updated refs, re-reads the branch list and toasts
   * "upstream moved" for every local branch whose behind-count increased or
   * appeared since the previous cycle (cap {@link UPSTREAM_MOVED_TOAST_CAP}
   * per cycle; branch-list failures are silent — the toast is a bonus).
   */
  async reportUpstreamMoved(repoId: RepoId): Promise<void> {
    let list: BranchInfo[];
    try {
      list = await branches(repoId);
    } catch {
      return;
    }
    // Stopped while the branch list was in flight: don't resurrect the
    // behind-count map that stop() just cleared.
    if (!this.#running.has(repoId)) return;
    const { messages, current } = upstreamMoved(
      this.#lastBehind.get(repoId) ?? new Map<string, number>(),
      list,
    );
    this.#lastBehind.set(repoId, current);
    for (const message of messages) toast(message);
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
