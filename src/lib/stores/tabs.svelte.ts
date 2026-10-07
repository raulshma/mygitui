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
  onRepoChanged,
  openRepo,
  repoStatus,
} from "$lib/ipc/client";
import type { RepoId, RepoStatus } from "$lib/ipc/types";
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
}

/**
 * Shared recent-repositories instance for the whole app (App.svelte's home
 * view reads it; TabStore touches it on open). Lives here because the tab
 * flow owns "a repository was opened".
 */
export const recentRepos = new RecentRepoStore();

export class TabStore {
  /** Open tabs, in strip order. */
  tabs: Tab[] = $state([]);
  /** `Tab.id` of the focused tab, or `null` when nothing is open. */
  activeId: string | null = $state(null);

  readonly #recents: RecentRepoStore;

  constructor(recents: RecentRepoStore = recentRepos) {
    this.#recents = recents;
  }

  /** The currently active tab, or `null`. */
  get active(): Tab | null {
    return this.tabs.find((tab) => tab.id === this.activeId) ?? null;
  }

  /**
   * Opens `path` as a repository tab: adds it, activates it and records it
   * in the recents store. Opening a root that already has a tab just
   * focuses that tab (matched by exact path first, then by the canonical
   * root the backend returns). Rejects (IpcError) when the backend refuses
   * the open.
   */
  async openTab(path: string): Promise<Tab> {
    const byPath = this.tabs.find((tab) => tab.root === path);
    if (byPath) {
      this.activeId = byPath.id;
      this.#recents.add(path);
      return byPath;
    }

    const info = await openRepo(path);

    const byRoot = this.tabs.find((tab) => tab.root === info.root);
    if (byRoot) {
      this.activeId = byRoot.id;
      this.#recents.add(path);
      return byRoot;
    }

    const tab: Tab = {
      id: info.repo_id,
      name: info.name,
      root: info.root,
      status: null,
      generation: 0,
    };
    this.tabs.push(tab);
    this.activeId = tab.id;
    this.#recents.add(path);

    // Populate the status stub; failures are non-fatal (logged inside).
    await this.refreshStatus(tab.id);
    return tab;
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
  }

  /** Activates a tab (no-op for unknown ids). */
  setActive(id: RepoId): void {
    if (this.tabs.some((tab) => tab.id === id)) this.activeId = id;
  }

  /**
   * Refetches `repo_status` for a tab and swaps in the new status object
   * (fresh reference → rune reactivity). Never throws.
   */
  async refreshStatus(id: RepoId): Promise<void> {
    const tab = this.tabs.find((t) => t.id === id);
    if (!tab) return;
    try {
      tab.status = await repoStatus(id);
    } catch (err) {
      console.warn(`[tabs] repo_status failed for ${id}:`, err);
    }
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

export function openTab(path: string): Promise<Tab> {
  return tabStore.openTab(path);
}

export function closeTab(id: RepoId): Promise<void> {
  return tabStore.closeTab(id);
}

export function setActive(id: RepoId): void {
  tabStore.setActive(id);
}

export function refreshStatus(id: RepoId): Promise<void> {
  return tabStore.refreshStatus(id);
}

// -- repo-changed watcher wiring ------------------------------------------

let eventsStarted = false;
let eventsUnlisten: (() => void) | null = null;

/**
 * Subscribes the tab store to backend `repo-changed` events (any watcher
 * event for a known tab triggers a status refresh — `head_moved` and
 * `full` alike). Idempotent: safe to call from every entry point.
 */
export function startTabEvents(): void {
  if (eventsStarted) return;
  eventsStarted = true;
  onRepoChanged((event) => {
    const tab = tabStore.tabs.find((t) => t.id === event.repo_id);
    if (!tab) return;
    void tabStore.refreshStatus(tab.id);
  })
    .then((unlisten) => {
      eventsUnlisten = unlisten;
    })
    .catch((err) =>
      console.error("[tabs] repo-changed subscription failed:", err),
    );
}

/** Stops the watcher subscription and clears the singleton (test helper). */
export function resetTabStore(): void {
  try {
    eventsUnlisten?.();
  } catch {
    // Ignore.
  }
  eventsUnlisten = null;
  eventsStarted = false;
  tabStore.tabs = [];
  tabStore.activeId = null;
}
