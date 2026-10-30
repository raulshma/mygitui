/**
 * Tab store (Svelte 5 runes) — one tab per open repository.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Components bind to the exported {@link tabStore} singleton (reading
 * `tabStore.tabs` / `tabStore.activeId` inside a template is reactive) or to
 * the standalone delegate functions below; tests build isolated instances
 * via `new TabStore(recents)`.
 *
 * Backend access goes exclusively through `$lib/ipc/client` (mocked in
 * tests via `vi.mock`).
 */

import {
  closeRepo,
  openRepo,
  remoteRepoOpen,
  repoStatus,
} from "$lib/ipc/client";
import type { RepoId, RepoStatus } from "$lib/ipc/types";
import { onRepoChange } from "$lib/events/repoEvents";
import { RecentRepoStore } from "$lib/entry/recentRepos";

/** One open repository tab. */
export interface Tab {
  /** `RepoInfo.repo_id` from the backend. */
  id: RepoId;
  /** Human-friendly repository name (`RepoInfo.name`). */
  name: string;
  /** Canonical repository root (`RepoInfo.root`) — dedupe key. */
  root: string;
  /** Latest known status; `null` until the first `repo_status` lands. */
  status: RepoStatus | null;
  /** Watcher/stream generation, bumped when the repo changes under us. */
  generation: number;
  /**
   * Owning device for remote tabs (`RepoInfo.device_id`), `null` for local
   * repositories.
   */
  deviceId: string | null;
  /**
   * Remote tabs only: why the last `remote_repo_open` failed. A tab with an
   * `openError` has no backend repo yet (`id` is its `ssh://` URI) and the
   * shell renders an offline view with a Retry affordance; `null` once open.
   */
  openError: string | null;
}

// -- ssh:// remote paths -----------------------------------------------------
//
// A repository on a device opens as a normal tab; everywhere a local tab is
// keyed by its canonical path, a remote tab is keyed by the URI below (the
// recents list and the persisted session therefore hold URIs too, and
// restoreSession re-opens them through the same openTab entry point).

/** URI scheme that prefixes remote tab roots / recents entries. */
export const SSH_SCHEME = "ssh://";

/** Builds the canonical `ssh://<device-id>/<absolute-path>` URI. */
export function sshUri(deviceId: string, path: string): string {
  return `${SSH_SCHEME}${deviceId}/${path.replace(/^\/+/, "")}`;
}

/** Parses an `ssh://` tab root; `null` for anything else. */
export function parseSshUri(
  uri: string,
): { deviceId: string; path: string } | null {
  if (!uri.startsWith(SSH_SCHEME)) return null;
  const rest = uri.slice(SSH_SCHEME.length);
  const slash = rest.indexOf("/");
  if (slash <= 0) return null;
  const deviceId = rest.slice(0, slash);
  const path = rest.slice(slash);
  return deviceId !== "" && path !== "" ? { deviceId, path } : null;
}

/** Last `/`-segment of a device path (remote tab display names). */
function remoteRepoName(path: string): string {
  const segments = path.split("/").filter(Boolean);
  return segments.length > 0 ? segments[segments.length - 1]! : path;
}

/** Shared recent-repositories instance for the whole app (App.svelte's home
 * view reads it; TabStore touches it on open). Lives here because the tab
 * flow owns "a repository was opened".
 */
export const recentRepos = new RecentRepoStore();

// -- last-session persistence ----------------------------------------------

/** localStorage key for the open-tabs session (roots + focused root). */
export const SESSION_STORAGE_KEY = "mygitui.session";

/** What gets persisted: the open tab roots and the focused one. */
interface StoredSession {
  roots: string[];
  activeRoot: string | null;
}

/** Minimal storage surface (subset of DOM `Storage`; injectable in tests). */
interface SessionStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** Optional behavior for {@link TabStore.openTab}. */
export interface OpenTabOptions {
  /**
   * Focus the tab after opening (default `true`). Session restore passes
   * `false` so parallel opens don't fight over the focused tab.
   */
  activate?: boolean;
  /**
   * Strip position for a newly created tab (default: append at the end).
   * Ignored when the open dedupes into an existing tab. Session restore
   * passes the stored index so resolve order can't reshuffle the strip.
   */
  index?: number;
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultSessionStorage(): SessionStorage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Parses persisted session JSON; drops anything malformed. */
export function parseStoredSession(raw: string | null): StoredSession {
  if (!raw) return { roots: [], activeRoot: null };
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) {
      return { roots: [], activeRoot: null };
    }
    const obj = parsed as Record<string, unknown>;
    const roots = Array.isArray(obj.roots)
      ? obj.roots.filter((r): r is string => typeof r === "string" && r.length > 0)
      : [];
    const activeRoot =
      typeof obj.activeRoot === "string" && obj.activeRoot.length > 0
        ? obj.activeRoot
        : null;
    return { roots, activeRoot };
  } catch {
    return { roots: [], activeRoot: null };
  }
}

export class TabStore {
  /** Open tabs, in strip order. */
  tabs: Tab[] = $state([]);
  /** `Tab.id` of the focused tab, or `null` when nothing is open. */
  activeId: string | null = $state(null);
  /**
   * True from construction until {@link restoreSession} settles. The shell
   * uses it to hold the launch view instead of flashing the recents list
   * while the stored repository is still opening.
   */
  restoring: boolean = $state(false);
  /** First root the launch restore focuses (for the launch hint). */
  restoreRoot: string | null = $state(null);
  /**
   * True while the recent-repositories home is shown even though tabs are
   * open (the top-bar home button). Cleared by any tab activation.
   */
  recentsOpen: boolean = $state(false);

  readonly #recents: RecentRepoStore;
  readonly #sessionStorage: SessionStorage | null;

  /** Per-repo status refresh cycle (coalesces event bursts). */
  readonly #statusRefresh = new Map<
    RepoId,
    { again: boolean; promise: Promise<void> }
  >();

  constructor(
    recents: RecentRepoStore = recentRepos,
    sessionStorage: SessionStorage | null = defaultSessionStorage(),
  ) {
    this.#recents = recents;
    this.#sessionStorage = sessionStorage;
    // Decide before the first render (onMount runs after paint): a stored
    // session means the shell starts in the launch view, not on home.
    const stored = parseStoredSession(
      this.#sessionStorage?.getItem(SESSION_STORAGE_KEY) ?? null,
    );
    this.restoring = stored.roots.length > 0;
    this.restoreRoot = stored.activeRoot ?? stored.roots[0] ?? null;
  }

  /** The currently active tab, or `null`. */
  get active(): Tab | null {
    return this.tabs.find((tab) => tab.id === this.activeId) ?? null;
  }

  /** Writes the current open-tab roots + focused root to storage. */
  #persistSession(): void {
    if (!this.#sessionStorage) return;
    try {
      const session: StoredSession = {
        roots: this.tabs.map((tab) => tab.root),
        activeRoot: this.active?.root ?? null,
      };
      this.#sessionStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(session));
    } catch {
      // Storage may be full or unavailable; the session stays in memory.
    }
  }

  /**
   * Reopens the last session's tabs (launch restore): opens every stored
   * root in parallel (folders that vanished are skipped silently) and
   * focuses the stored active root the moment its own open resolves, so
   * the previously-focused repository is the first full view. Tabs insert
   * at their stored strip position, so resolve order never reshuffles the
   * strip. Tabs opened before this call (CLI arg / deep link) are kept and
   * deduped by root.
   */
  async restoreSession(): Promise<void> {
    const session = parseStoredSession(
      this.#sessionStorage?.getItem(SESSION_STORAGE_KEY) ?? null,
    );
    if (session.roots.length === 0) {
      this.restoring = false;
      return;
    }
    this.restoring = true;
    try {
      await Promise.all(
        session.roots.map((root, index) =>
          this.openTab(root, { activate: false, index })
            .then((tab) => {
              if (tab.root === session.activeRoot) this.activeId = tab.id;
            })
            .catch(() => {
              // Folder deleted/moved since the last run — skip it.
            }),
        ),
      );
    } finally {
      // The stored active root may not have come back (deleted/moved): fall
      // back to the first restored tab instead of landing on the home view.
      if (this.active === null && this.tabs.length > 0) {
        this.activeId = this.tabs[0]!.id;
      }
      this.restoring = false;
      this.restoreRoot = null;
      this.#persistSession();
    }
  }

  /**
   * Opens `path` as a repository tab: adds it, activates it and records it
   * in the recents store. Opening a root that already has a tab just
   * focuses that tab (matched by exact path first, then by the canonical
   * root the backend returns). Rejects (IpcError) when the backend refuses
   * the open.
   *
   * `ssh://<device-id>/<path>` URIs (remote tabs, also what the recents
   * list persists) route through {@link openRemoteTab}.
   *
   * The first `repo_status` snapshot is fetched in the background: the tab
   * renders immediately with `status: null` and the snapshot swaps in via
   * reactivity (awaiting it here serialized session restore behind a full
   * working-copy scan per tab).
   */
  async openTab(path: string, options: OpenTabOptions = {}): Promise<Tab> {
    const remote = parseSshUri(path);
    if (remote) return this.openRemoteTab(remote.deviceId, remote.path, options);
    const activate = options.activate ?? true;
    const byPath = this.tabs.find((tab) => tab.root === path);
    if (byPath) {
      if (activate) {
        this.activeId = byPath.id;
        this.recentsOpen = false;
      }
      this.#recents.add(path);
      this.#persistSession();
      return byPath;
    }

    const info = await openRepo(path);

    const byRoot = this.tabs.find((tab) => tab.root === info.root);
    if (byRoot) {
      if (activate) {
        this.activeId = byRoot.id;
        this.recentsOpen = false;
      }
      this.#recents.add(path);
      this.#persistSession();
      return byRoot;
    }

    const tab: Tab = {
      id: info.repo_id,
      name: info.name,
      root: info.root,
      status: null,
      generation: 0,
      deviceId: null,
      openError: null,
    };
    const at = Math.min(options.index ?? this.tabs.length, this.tabs.length);
    this.tabs.splice(at, 0, tab);
    if (activate) {
      this.activeId = tab.id;
      this.recentsOpen = false;
    }
    this.#recents.add(path);
    this.#persistSession();

    // Populate the status stub in the background; failures are non-fatal
    // (logged inside, and `refreshStatus` never throws).
    void this.refreshStatus(tab.id);
    return tab;
  }

  /**
   * Opens a repository that lives on a device as a normal tab: calls
   * `remote_repo_open` and mirrors {@link openTab} (dedupe, activate,
   * recents). The tab is keyed by the `ssh://` URI (`root`), so the
   * recents list and the persisted session round-trip it unchanged.
   *
   * Unlike {@link openTab} this NEVER throws for backend failures: an
   * unknown/offline device (or a vanished path) opens the tab in an
   * offline state (`openError` set, `id` = the URI) with the entry kept in
   * the strip — {@link retryRemoteTab} re-runs the open from the Retry
   * affordance the shell renders for it.
   */
  async openRemoteTab(
    deviceId: string,
    path: string,
    options: OpenTabOptions = {},
  ): Promise<Tab> {
    const activate = options.activate ?? true;
    const uri = sshUri(deviceId, path);
    const byUri = this.tabs.find((tab) => tab.root === uri);
    if (byUri && byUri.openError === null) {
      if (activate) {
        this.activeId = byUri.id;
        this.recentsOpen = false;
      }
      this.#recents.add(uri);
      this.#persistSession();
      return byUri;
    }

    try {
      const info = await remoteRepoOpen(deviceId, path);
      const tab: Tab = {
        id: info.repo_id,
        name: info.name,
        root: uri,
        status: null,
        generation: 0,
        deviceId,
        openError: null,
      };
      if (byUri) {
        // Upgrade the offline tab in place: strip position is preserved.
        this.tabs.splice(this.tabs.indexOf(byUri), 1, tab);
      } else {
        const at = Math.min(options.index ?? this.tabs.length, this.tabs.length);
        this.tabs.splice(at, 0, tab);
      }
      if (activate) {
        this.activeId = tab.id;
        this.recentsOpen = false;
      }
      this.#recents.add(uri);
      this.#persistSession();
      void this.refreshStatus(tab.id);
      return tab;
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      const tab: Tab = {
        id: uri,
        name: remoteRepoName(path),
        root: uri,
        status: null,
        generation: 0,
        deviceId,
        openError: message,
      };
      if (byUri) {
        this.tabs.splice(this.tabs.indexOf(byUri), 1, tab);
      } else {
        const at = Math.min(options.index ?? this.tabs.length, this.tabs.length);
        this.tabs.splice(at, 0, tab);
      }
      if (activate) {
        this.activeId = tab.id;
        this.recentsOpen = false;
      }
      this.#recents.add(uri);
      this.#persistSession();
      return tab;
    }
  }

  /**
   * Re-runs the remote open for an offline tab (`Retry` affordance).
   * No-op for unknown or already-open roots.
   */
  async retryRemoteTab(uri: string): Promise<void> {
    const parsed = parseSshUri(uri);
    if (!parsed) return;
    const tab = this.tabs.find((entry) => entry.root === uri);
    if (!tab || tab.openError === null) return;
    await this.openRemoteTab(parsed.deviceId, parsed.path);
  }

  /**
   * Closes a tab: notifies the backend, removes the tab, and — when the
   * closed tab was active — activates its neighbor (left preferred, then
   * right, else nothing). Backend close failures still remove the tab.
   */
  async closeTab(id: RepoId): Promise<void> {
    if (this.tabs.every((tab) => tab.id !== id)) return;
    const wasActive = this.activeId === id;
    try {
      await closeRepo(id);
    } catch (err) {
      console.warn(`[tabs] repo_close failed for ${id}:`, err);
    }
    const index = this.tabs.findIndex((tab) => tab.id === id);
    if (index === -1) return;
    this.tabs.splice(index, 1);
    if (wasActive) {
      const neighbor = this.tabs[Math.max(0, index - 1)];
      this.activeId = neighbor ? neighbor.id : null;
    }
    this.#persistSession();
  }

  /** Activates a tab (no-op for unknown ids). Leaves the recents home. */
  setActive(id: RepoId): void {
    if (this.tabs.some((tab) => tab.id === id)) {
      this.activeId = id;
      this.recentsOpen = false;
      this.#persistSession();
    }
  }

  /** Shows the recents home above the open tabs (nothing is closed). */
  showRecentRepos(): void {
    this.recentsOpen = true;
  }

  /** Returns from the recents home to the active tab. */
  hideRecentRepos(): void {
    this.recentsOpen = false;
  }

  /**
   * Refetches `repo_status` for a tab and swaps in the new status object
   * (fresh reference → rune reactivity). Never throws.
   *
   * Coalesced per repo: a burst of watcher events / post-mutation refreshes
   * shares one fetch, plus at most one trailing fetch when another request
   * arrived while it was running. Callers all resolve once the status they
   * asked for is in (never while a pre-request fetch is still stale).
   */
  async refreshStatus(id: RepoId): Promise<void> {
    const running = this.#statusRefresh.get(id);
    if (running) {
      running.again = true;
      return running.promise;
    }
    const state: { again: boolean; promise: Promise<void> } = {
      again: false,
      promise: Promise.resolve(),
    };
    this.#statusRefresh.set(id, state);
    state.promise = (async () => {
      do {
        state.again = false;
        const tab = this.tabs.find((t) => t.id === id);
        if (!tab) return;
        try {
          tab.status = await repoStatus(id);
        } catch (err) {
          console.warn(`[tabs] repo_status failed for ${id}:`, err);
        }
      } while (state.again);
    })().finally(() => {
      if (this.#statusRefresh.get(id) === state) {
        this.#statusRefresh.delete(id);
      }
    });
    return state.promise;
  }
}

/** The application-wide tab store. */
export const tabStore = new TabStore();

// Standalone function API over the singleton (what components import). --

export function getTabs(): Tab[] {
  return tabStore.tabs;
}

export function getActiveId(): string | null {
  return tabStore.activeId;
}

export function openTab(path: string, options: OpenTabOptions = {}): Promise<Tab> {
  return tabStore.openTab(path, options);
}

/** Opens a repository on a device (see {@link TabStore.openRemoteTab}). */
export function openRemoteTab(
  deviceId: string,
  path: string,
  options: OpenTabOptions = {},
): Promise<Tab> {
  return tabStore.openRemoteTab(deviceId, path, options);
}

/** Re-runs a failed remote open (see {@link TabStore.retryRemoteTab}). */
export function retryRemoteTab(uri: string): Promise<void> {
  return tabStore.retryRemoteTab(uri);
}

export function closeTab(id: RepoId): Promise<void> {
  return tabStore.closeTab(id);
}

export function setActive(id: RepoId): void {
  tabStore.setActive(id);
}

/** Shows the recents home without closing the open tabs. */
export function showRecentRepos(): void {
  tabStore.showRecentRepos();
}

/** Returns from the recents home to the active tab. */
export function hideRecentRepos(): void {
  tabStore.hideRecentRepos();
}

/** Reopens the last session's tabs (see {@link TabStore.restoreSession}). */
export function restoreSession(): Promise<void> {
  return tabStore.restoreSession();
}

export function refreshStatus(id: RepoId): Promise<void> {
  return tabStore.refreshStatus(id);
}

// -- repo-changed watcher wiring ------------------------------------------

let unregisterEvents: (() => void) | null = null;

/**
 * Subscribes the tab store to backend `repo-changed` events through the
 * shared event module (`$lib/events/repoEvents`). Policy, declared here:
 * ANY watcher event for a known tab triggers a status refresh — `head_moved`
 * and `full` alike, and index/workdir churn too — because the tab status
 * covers the working copy as well as refs, so no event kind is skippable.
 * Idempotent: safe to call from every entry point.
 */
export function startTabEvents(): void {
  if (unregisterEvents !== null) return;
  unregisterEvents = onRepoChange({
    filter: () => true,
    handle: (change) => {
      const tab = tabStore.tabs.find((t) => t.id === change.repo_id);
      if (!tab) return;
      void tabStore.refreshStatus(tab.id);
    },
  });
}

/** Stops the watcher subscription and clears the singleton (test helper). */
export function resetTabStore(): void {
  unregisterEvents?.();
  unregisterEvents = null;
  tabStore.tabs = [];
  tabStore.activeId = null;
  tabStore.restoring = false;
  tabStore.restoreRoot = null;
  tabStore.recentsOpen = false;
}
