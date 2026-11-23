/**
 * Overview data manager (Svelte 5 runes) — builds the Overview rows from the
 * library (recents ∪ open tabs ∪ favorites) and keeps them fresh per the
 * three data tiers:
 *
 * - Open tabs: LIVE — the row reads the tab's already-maintained
 *   `tab.status` (watchers + repo-changed events via `startTabEvents`) and
 *   the forge store's snapshot/PR lanes. No duplicate fetching.
 * - Warmed repos: a non-open repo whose row the user expanded gets a REAL
 *   session (`repo_open`, watcher + gh lane) for the Overview's lifetime,
 *   managed by {@link OverviewWarmer}. Its status refreshes through
 *   `repo_status` on `repo-changed` events, exactly like a tab; leaving the
 *   Overview closes warmed sessions again (`repo_close`) except roots that
 *   are genuine tabs.
 * - Everything else: scanned ONLY while the Overview is visible (become-
 *   visible, window focus, manual Refresh) through the read-only
 *   `repo_summary` command, concurrency-capped, rows updating in place.
 *   No persistent watchers for non-open repos.
 */

import {
  closeRepo,
  onRepoChanged,
  openRepo,
  remoteRepoOpen,
  repoStatus,
  repoSummary,
} from "$lib/ipc/client";
import type { PrInfo, RepoId, RepoStatus, RepoSummary } from "$lib/ipc/types";
import { forgeStore } from "$lib/stores/forge.svelte";
import { favoriteRepos } from "$lib/stores/favoriteRepos.svelte";
import { parseSshUri, recentRepos, tabStore } from "$lib/stores/tabs.svelte";
import {
  applyLiveStatus,
  applySummary,
  buildLibrary,
  emptyRow,
  markMissing,
  matchesFilter,
  type OverviewRowData,
  rowFailingChecks,
  sortRows,
  totalsOf,
  type OverviewTotals,
} from "./overviewModel";

/** Max in-flight `repo_summary` scans (spec: 4). */
export const SCAN_CONCURRENCY = 4;

// ---------------------------------------------------------------------------
// Warming manager (own class, injected IPC for tests)
// ---------------------------------------------------------------------------

/** Opening/closing seams (tests inject fakes). */
export interface WarmerIpc {
  open(path: string): Promise<{ repo_id: RepoId; root: string }>;
  close(repoId: RepoId): Promise<void>;
}

const realWarmerIpc: WarmerIpc = {
  open: async (path: string) => {
    const remote = parseSshUri(path);
    if (remote) {
      const info = await remoteRepoOpen(remote.deviceId, remote.path);
      forgeStore.registerRepoDevice(info.repo_id, remote.deviceId);
      return { repo_id: info.repo_id, root: path };
    }
    return openRepo(path);
  },
  close: closeRepo,
};

/**
 * Warms non-open repos for the Overview session: `repo_open` gives the row
 * a real backend session (watcher + gh lane), and {@link closeExcept} tears
 * those sessions down again when the Overview closes — keeping repos that
 * are genuine tabs.
 */
export class OverviewWarmer {
  readonly #ipc: WarmerIpc;
  /** root → warmed repo id. */
  readonly #warmed = new Map<string, RepoId>();
  /** In-flight warm opens (a double-expand must not open twice). */
  readonly #pending = new Map<string, Promise<RepoId | null>>();
  /** Bumped by closeExcept/clear: an open landing after the bump is torn
   *  back down instead of leaking a session past the Overview's exit. */
  #epoch = 0;

  constructor(ipc: WarmerIpc = realWarmerIpc) {
    this.#ipc = ipc;
  }

  /** The warmed repo id for `root`, or null (not warmed). */
  idFor(root: string): RepoId | null {
    return this.#warmed.get(root) ?? null;
  }

  isWarmed(root: string): boolean {
    return this.#warmed.has(root);
  }

  /**
   * Warms `root` (idempotent, coalesced per root): resolves the warmed repo
   * id, or null when the open failed (the row stays summary-tier).
   */
  warm(root: string): Promise<RepoId | null> {
    const existing = this.#warmed.get(root);
    if (existing) return Promise.resolve(existing);
    const pending = this.#pending.get(root);
    if (pending) return pending;
    const epoch = this.#epoch;
    const run = this.#ipc
      .open(root)
      .then((info) => {
        if (epoch !== this.#epoch) {
          // closeExcept ran while the open was in flight: close the session
          // again instead of leaking a watcher past the Overview's exit.
          void this.#ipc.close(info.repo_id).catch((err: unknown) => {
            console.warn(`[overview] closing late warm ${info.repo_id} failed:`, err);
          });
          forgeStore.registerRepoDevice(info.repo_id, null);
          return null;
        }
        this.#warmed.set(root, info.repo_id);
        return info.repo_id;
      })
      .catch((err: unknown) => {
        console.warn(`[overview] warming ${root} failed:`, err);
        return null;
      })
      .finally(() => {
        this.#pending.delete(root);
      });
    this.#pending.set(root, run);
    return run;
  }

  /**
   * Closes every warmed session whose root is NOT in `keepRoots` (the open
   * tabs). Close failures are swallowed (the session is forgotten either
   * way — the backend close is best-effort, like `tabStore.closeTab`).
   */
  async closeExcept(keepRoots: string[]): Promise<void> {
    this.#epoch += 1;
    const keep = new Set(keepRoots.map((root) => root.toLowerCase()));
    const entries = [...this.#warmed.entries()];
    this.#warmed.clear();
    for (const [root, repoId] of entries) {
      if (keep.has(root.toLowerCase())) this.#warmed.set(root, repoId);
    }
    await Promise.all(
      entries
        .filter(([root]) => !keep.has(root.toLowerCase()))
        .map(([, repoId]) => {
          forgeStore.registerRepoDevice(repoId, null);
          return this.#ipc.close(repoId).catch((err: unknown) => {
            console.warn(`[overview] closing warmed ${repoId} failed:`, err);
          });
        }),
    );
  }

  /** Forgets the map without closing (test reset). */
  clear(): void {
    for (const repoId of this.#warmed.values()) {
      forgeStore.registerRepoDevice(repoId, null);
    }
    this.#epoch += 1;
    this.#warmed.clear();
  }
}

// ---------------------------------------------------------------------------
// Concurrency-capped summary scan (exported pure-ish for tests)
// ---------------------------------------------------------------------------

export interface ScanCallbacks {
  /** Fetch one root's summary (rejects for missing/invalid repos). */
  fetch(root: string): Promise<RepoSummary>;
  /** Called with each landed result, in completion order. */
  onResult(root: string, summary: RepoSummary): void;
  /** Called per failed root (missing folder, non-repo, ...). */
  onError(root: string, err: unknown): void;
}

/**
 * Scans `roots` with at most {@link SCAN_CONCURRENCY} fetches in flight,
 * calling back per result as it lands (rows update in place, not in batch).
 * Resolves when every root settled — never throws.
 */
export async function scanRepoSummaries(
  roots: string[],
  callbacks: ScanCallbacks,
  concurrency: number = SCAN_CONCURRENCY,
): Promise<void> {
  let next = 0;
  const take = (): string | null => (next < roots.length ? roots[next++]! : null);
  const worker = async (): Promise<void> => {
    for (;;) {
      const root = take();
      if (root === null) return;
      try {
        const summary = await callbacks.fetch(root);
        callbacks.onResult(root, summary);
      } catch (err) {
        callbacks.onError(root, err);
      }
    }
  };
  await Promise.all(
    Array.from({ length: Math.min(concurrency, Math.max(roots.length, 1)) }, worker),
  );
}

// ---------------------------------------------------------------------------
// Forge helpers (PR + check summaries per row)
// ---------------------------------------------------------------------------

/** The open PRs the forge store currently holds for `repoId` (or null). */
export function prsForRepo(repoId: string | null): PrInfo[] | null {
  if (repoId === null) return null;
  const prs = forgeStore.prsStaleFor(repoId);
  if (prs !== null) {
    return prs.filter((pr) => pr.state === "OPEN");
  }
  const snapshot = forgeStore.snapshotStaleFor(repoId);
  if (snapshot?.pr && snapshot.pr.state === "OPEN") {
    return [snapshot.pr];
  }
  return null;
}

/**
 * The repo's HEAD check status as a failing-count: the snapshot's head
 * checks (PR-head checks when the PR head diverged). Null = no snapshot.
 */
export function failingChecksFor(repoId: string | null): number | null {
  if (repoId === null) return null;
  const snapshot = forgeStore.snapshotStaleFor(repoId);
  if (snapshot === null) return null;
  const checks = snapshot.pr_head_checks ?? snapshot.head_checks;
  return checks.filter((check) => check.state === "fail").length;
}

/** True when at least one row has data the "Failing checks" filter can use. */
export function anyChecksKnown(rows: OverviewRowData[]): boolean {
  return rows.some((row) => row.failingChecks !== null);
}

// ---------------------------------------------------------------------------
// OverviewData store
// ---------------------------------------------------------------------------

/** Whether the Overview is currently visible (activate/deactivate contract). */
export interface OverviewDataOptions {
  /** Scan seam (tests inject a fake instead of `vi.mock`ing the client). */
  summarize?: (root: string) => Promise<RepoSummary>;
  /** Status seam for warmed repos. */
  status?: (repoId: RepoId, paths?: string[]) => Promise<RepoStatus>;
  /** Repo-changed subscription seam (tests fire their own events). */
  onChange?: typeof onRepoChanged;
  /** Warming manager (tests inject one preloaded with a fake IPC). */
  warmer?: OverviewWarmer;
  /** Wall-clock seam (stamp determinism). */
  now?: () => number;
}

export class OverviewData {
  /** Rows in library order (unfiltered, unsorted — the view derives). */
  rows = $state<OverviewRowData[]>([]);
  /** True while a summary scan is in flight. */
  scanning = $state(false);
  /** Wall-clock of the last full refresh pass ("Refreshed Xs ago"). */
  lastRefreshAt = $state<number | null>(null);

  readonly #summarize: (root: string) => Promise<RepoSummary>;
  readonly #status: (repoId: RepoId, paths?: string[]) => Promise<RepoStatus>;
  readonly #onChange: typeof onRepoChanged;
  readonly #warmer: OverviewWarmer;
  readonly #now: () => number;
  /** Unlisten for the warmed repos' repo-changed events (while active). */
  #unlisten: (() => void) | null = null;
  /** True between activate() and deactivate(); checked after awaits so an
   *  activate racing a deactivate can't land its subscription late. */
  #active = false;
  /** Bumped whenever a scan pass is superseded (deactivate): callbacks of a
   *  stale generation drop their results instead of writing hidden rows. */
  #scanGen = 0;

  constructor(options: OverviewDataOptions = {}) {
    this.#summarize = options.summarize ?? repoSummary;
    this.#status = options.status ?? repoStatus;
    this.#onChange = options.onChange ?? onRepoChanged;
    this.#warmer = options.warmer ?? new OverviewWarmer();
    this.#now = options.now ?? (() => Date.now());
  }

  /** The warming manager (the view closes warmed sessions on exit). */
  get warmer(): OverviewWarmer {
    return this.#warmer;
  }

  rowFor(root: string): OverviewRowData | null {
    return this.rows.find((row) => row.root === root) ?? null;
  }

  /**
   * Rebuilds the rows from the library, preserving per-root scan/warm state.
   * Called on every activate + whenever tabs/recents/favorites change while
   * the Overview is open.
   */
  sync(): void {
    const library = buildLibrary(
      recentRepos.list(),
      tabStore.tabs.map((tab) => tab.root),
      favoriteRepos.favorites,
    );
    const previous = new Map(this.rows.map((row) => [row.root, row]));
    this.rows = library.map((entry) => {
      const old = previous.get(entry.root);
      const row = old ? { ...old } : emptyRow(entry);
      // Provenance flags follow the live library (pins/favorites change).
      row.pinned = entry.pinned;
      row.favorite = entry.favorite;
      row.open = entry.open;
      row.lastOpened = entry.lastOpened;

      const tab = tabStore.tabs.find((t) => t.root === entry.root);
      row.isRemoteDevice = Boolean(tab?.deviceId || entry.root.startsWith("ssh://"));

      if (old) {
        row.hasRemote = old.hasRemote;
        row.remoteUrl = old.remoteUrl;
      }

      if (tab) {
        const tabAny = tab as unknown as {
          hasRemote?: boolean;
          remoteUrl?: string | null;
          remote_url?: string | null;
          has_remote?: boolean;
        };
        const statusAny = tab.status as unknown as {
          hasRemote?: boolean;
          remoteUrl?: string | null;
          remote_url?: string | null;
          has_remote?: boolean;
        } | null;

        const remoteUrl =
          tabAny.remoteUrl ??
          tabAny.remote_url ??
          statusAny?.remoteUrl ??
          statusAny?.remote_url;
        if (remoteUrl !== undefined) {
          row.remoteUrl = remoteUrl;
        }
        const hasRemote =
          tabAny.hasRemote ??
          tabAny.has_remote ??
          statusAny?.hasRemote ??
          statusAny?.has_remote ??
          (remoteUrl !== undefined ? remoteUrl !== null : undefined);
        if (hasRemote !== undefined) {
          row.hasRemote = hasRemote;
        }
      }

      // repoId follows the live tiers: a closed tab must not leave its dead
      // id behind (diff streams / PR reads against it would fail), so the
      // assignment is unconditional — tab id while open, warmed session
      // otherwise, null for pure summary-tier rows.
      row.repoId = row.open
        ? (tab?.id ?? null)
        : (this.#warmer.idFor(entry.root) ?? null);
      return row;
    });
  }

  /**
   * The Overview became visible: subscribe to warmed-repo events, sync the
   * library, apply live tab data immediately, and scan everything that has
   * no live source yet.
   */
  async activate(): Promise<void> {
    this.#active = true;
    this.sync();
    if (this.#unlisten !== null) return;
    const unlisten = await this.#onChange((change) => {
      const row = this.rows.find(
        (candidate) =>
          candidate.repoId === change.repo_id && !candidate.open,
      );
      if (!row) return;
      void this.refreshWarmedRow(row.root);
    });
    if (!this.#active) {
      // deactivate() ran while the subscription was being created: undo it
      // instead of leaking a listener (and a scan) past the exit.
      unlisten();
      return;
    }
    this.#unlisten = unlisten;
    await this.refresh();
  }

  /**
   * The Overview is no longer visible: unsubscribe, drop any in-flight scan
   * (stale results are discarded), and close warmed sessions (repos that
   * are genuine tabs keep theirs).
   */
  async deactivate(): Promise<void> {
    this.#active = false;
    this.#scanGen += 1;
    this.scanning = false;
    this.#unlisten?.();
    this.#unlisten = null;
    const keep = tabStore.tabs.map((tab) => tab.root);
    await this.#warmer.closeExcept(keep);
  }

  /** Applies the open tabs' live status to their rows (no IPC). */
  #applyTabData(): void {
    for (const row of this.rows) {
      if (!row.open) continue;
      const tab = tabStore.tabs.find((t) => t.root === row.root);
      if (!tab) continue;
      row.repoId = tab.id;
      row.isRemoteDevice = Boolean(tab.deviceId || tab.root.startsWith("ssh://"));

      const tabAny = tab as unknown as {
        hasRemote?: boolean;
        remoteUrl?: string | null;
        remote_url?: string | null;
        has_remote?: boolean;
      };
      const statusAny = tab.status as unknown as {
        hasRemote?: boolean;
        remoteUrl?: string | null;
        remote_url?: string | null;
        has_remote?: boolean;
      } | null;

      const remoteUrl =
        tabAny.remoteUrl ??
        tabAny.remote_url ??
        statusAny?.remoteUrl ??
        statusAny?.remote_url;
      if (remoteUrl !== undefined) {
        row.remoteUrl = remoteUrl;
      }
      const hasRemote =
        tabAny.hasRemote ??
        tabAny.has_remote ??
        statusAny?.hasRemote ??
        statusAny?.has_remote ??
        (remoteUrl !== undefined ? remoteUrl !== null : undefined);
      if (hasRemote !== undefined) {
        row.hasRemote = hasRemote;
      }

      if (tab.status !== null) {
        const next = applyLiveStatus(row, tab.status, this.#now());
        // Keep a scan's last-commit (tab status has none) and PR data.
        next.lastCommit = row.lastCommit;
        next.prs = row.prs;
        next.failingChecks = row.failingChecks;
        Object.assign(row, next);
      }
      row.prs = prsForRepo(tab.id);
      row.failingChecks = failingChecksFor(tab.id);
    }
  }

  /** One warmed repo's status refresh (repo-changed event / warm landing). */
  async refreshWarmedRow(root: string): Promise<void> {
    const row = this.rowFor(root);
    const repoId = row?.repoId ?? this.#warmer.idFor(root);
    if (!row || repoId === null) return;
    try {
      const status = await this.#status(repoId);
      const next = applyLiveStatus(row, status, this.#now());
      next.lastCommit = row.lastCommit;
      Object.assign(row, next);
      row.prs = prsForRepo(repoId);
      row.failingChecks = failingChecksFor(repoId);
    } catch (err) {
      console.warn(`[overview] status refresh failed for ${root}:`, err);
    }
  }

  /**
   * Warms `root` (expanding a non-open row) and applies its first live
   * status. Resolves the warmed repo id, or null when the open failed.
   */
  async warmRow(root: string): Promise<RepoId | null> {
    this.sync();
    const repoId = await this.#warmer.warm(root);
    if (repoId === null) return null;
    const row = this.rowFor(root);
    if (row) {
      row.repoId = repoId;
      row.refreshing = true;
      await this.refreshWarmedRow(root);
      await this.refreshForgeRow(root, repoId);
    }
    return repoId;
  }

  /** PRs + head checks for a warmed/open repo (silent, cached lanes). */
  async refreshForgeRow(root: string, repoId: RepoId): Promise<void> {
    try {
      // Snapshot + PR list only when the machine's gh is authed; the strip
      // shows the banner once when it is not (the rows stay em-dash).
      const status = forgeStore.statusFor(repoId);
      if (status === null) await forgeStore.refreshStatus(repoId);
      const gh = forgeStore.statusFor(repoId);
      if (gh === null || !gh.available || !gh.authed) return;
      await Promise.all([
        forgeStore.refreshSnapshot(repoId, { silent: true }).catch(() => null),
        forgeStore.refreshPrs(repoId).catch(() => []),
      ]);
      const row = this.rowFor(root);
      if (row) {
        if (!row.remoteUrl) {
          const ctx = await forgeStore.refreshContext(repoId).catch(() => null);
          if (ctx?.remote_url) {
            row.remoteUrl = ctx.remote_url;
            row.hasRemote = true;
          }
        }
        row.prs = prsForRepo(repoId);
        row.failingChecks = failingChecksFor(repoId);
      }
    } catch (err) {
      console.warn(`[overview] forge refresh failed for ${root}:`, err);
    }
  }

  /**
   * Full refresh pass: sync + live tab data + one capped summary scan over
   * every non-open, non-warmed root. Rows update in place as results land.
   */
  async refresh(): Promise<void> {
    // One pass at a time: focus + visibilitychange can both fire for the
    // same event — a second pass must not double-scan the library.
    if (this.scanning) return;
    this.sync();
    this.#applyTabData();

    for (const row of this.rows) {
      if (row.open && row.repoId) {
        void this.refreshForgeRow(row.root, row.repoId);
      }
    }

    const pending: string[] = [];
    for (const row of this.rows) {
      if (row.open) continue;
      if (this.#warmer.isWarmed(row.root)) {
        row.refreshing = true;
        void this.refreshWarmedRow(row.root).then(() => {
          const id = this.#warmer.idFor(row.root);
          if (id !== null) void this.refreshForgeRow(row.root, id);
        });
        continue;
      }
      if (row.isRemoteDevice || row.root.startsWith("ssh://")) {
        row.refreshing = true;
        void this.warmRow(row.root);
        continue;
      }
      pending.push(row.root);
    }
    if (pending.length === 0) {
      this.lastRefreshAt = this.#now();
      return;
    }
    this.scanning = true;
    const gen = ++this.#scanGen;
    for (const root of pending) {
      const row = this.rowFor(root);
      if (row) row.refreshing = true;
    }
    try {
      await scanRepoSummaries(pending, {
        fetch: this.#summarize,
        onResult: (root, summary) => {
          if (gen !== this.#scanGen) return;
          const row = this.rowFor(root);
          if (!row) return;
          Object.assign(row, applySummary(row, summary, this.#now()));
          row.prs = prsForRepo(row.repoId);
          row.failingChecks = failingChecksFor(row.repoId);
          if (row.hasRemote && (row.remoteUrl?.toLowerCase().includes("github") || row.isRemoteDevice)) {
            void this.warmRow(root);
          }
        },
        onError: (root) => {
          if (gen !== this.#scanGen) return;
          const row = this.rowFor(root);
          if (!row) return;
          Object.assign(row, markMissing(row, this.#now()));
        },
      });
    } finally {
      this.scanning = false;
      if (gen === this.#scanGen) this.lastRefreshAt = this.#now();
    }
  }

  /** "Add folder…": validates via the scan tier, then registers in recents. */
  async addFolder(path: string): Promise<boolean> {
    try {
      await this.#summarize(path);
    } catch {
      return false;
    }
    recentRepos.add(path);
    this.sync();
    return true;
  }

  /**
   * "Remove from library": drops the root from recents AND favorites. An
   * open tab stays until closed (the row just loses its library entry).
   */
  removeFromLibrary(root: string): void {
    recentRepos.remove(root);
    favoriteRepos.remove(root);
    this.rows = this.rows.filter((row) => row.root !== root);
  }

  // -- view derivations ------------------------------------------------------

  /** Filtered + sorted rows for the list. */
  visibleRows(filter: Parameters<typeof matchesFilter>[1], sort: Parameters<typeof sortRows>[1], sortDir: Parameters<typeof sortRows>[2]): OverviewRowData[] {
    return sortRows(
      this.rows.filter((row) => matchesFilter(row, filter)),
      sort,
      sortDir,
    );
  }

  /** Summary-strip totals over ALL rows (unfiltered). */
  totals(): OverviewTotals {
    return totalsOf(this.rows);
  }

  /** Drops all rows + scan state (tests; warmed sessions untouched). */
  reset(): void {
    this.rows = [];
    this.scanning = false;
    this.lastRefreshAt = null;
  }
}

/** The application-wide Overview data manager. */
export const overviewData = new OverviewData();
