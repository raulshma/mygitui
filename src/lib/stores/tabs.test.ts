/**
 * Unit tests for the tab store (`$lib/stores/tabs.svelte`). The IPC client
 * is fully mocked (`vi.mock`) — no Tauri runtime is touched.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  closeRepo,
  onRepoChanged,
  openRepo,
  repoStatus,
} from "$lib/ipc/client";
import type { RepoChangedEvent } from "$lib/ipc/client";
import { RecentRepoStore, type StorageLike } from "$lib/entry/recentRepos";
import {
  closeTab,
  openTab,
  parseStoredSession,
  resetTabStore,
  setActive,
  startTabEvents,
  tabStore,
  TabStore,
} from "$lib/stores/tabs.svelte";

vi.mock("$lib/ipc/client", () => ({
  openRepo: vi.fn(),
  closeRepo: vi.fn(),
  repoStatus: vi.fn(),
  onRepoChanged: vi.fn(),
}));

const mockOpenRepo = vi.mocked(openRepo);
const mockCloseRepo = vi.mocked(closeRepo);
const mockRepoStatus = vi.mocked(repoStatus);
const mockOnRepoChanged = vi.mocked(onRepoChanged);

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function repoInfo(repoId: string, root: string, name: string) {
  return {
    repo_id: repoId,
    root,
    name,
    bare: false,
    git_dir: `${root}/.git`,
  };
}

function status(branch: string, ahead: number, behind: number) {
  return {
    branch,
    head: "abc123def456",
    detached: false,
    ahead,
    behind,
    merging: false,
    rebasing: false,
    sequencer: false,
    entries: [],
  };
}

const ALPHA = repoInfo("repo-1", "C:/repos/alpha", "alpha");
const BETA = repoInfo("repo-2", "C:/repos/beta", "beta");
const STATUS_1 = status("main", 2, 1);
const STATUS_2 = status("feature", 0, 3);

/** Storage that never touches localStorage — keeps tests hermetic. */
const noStorage: StorageLike | null = null;

/** An isolated store over throwaway recents; assertions target it. */
function freshStore(): TabStore {
  return new TabStore(new RecentRepoStore(noStorage));
}

beforeEach(() => {
  mockOpenRepo.mockReset();
  mockCloseRepo.mockReset();
  mockRepoStatus.mockReset();
  mockOnRepoChanged.mockReset();
  mockCloseRepo.mockResolvedValue(undefined);
  mockRepoStatus.mockResolvedValue(status("main", 0, 0));
  mockOnRepoChanged.mockResolvedValue(() => {});
});

afterEach(() => {
  resetTabStore();
});

// ---------------------------------------------------------------------------
// openTab
// ---------------------------------------------------------------------------

describe("openTab", () => {
  it("opens a repository: adds a tab, activates it, fetches status, records recent", async () => {
    const recents = new RecentRepoStore(noStorage);
    const store = new TabStore(recents);
    mockOpenRepo.mockResolvedValue(ALPHA);
    mockRepoStatus.mockResolvedValue(STATUS_1);

    const tab = await store.openTab("C:/repos/alpha");

    expect(mockOpenRepo).toHaveBeenCalledWith("C:/repos/alpha");
    expect(tab).toMatchObject({
      id: "repo-1",
      name: "alpha",
      root: "C:/repos/alpha",
      generation: 0,
    });
    expect(store.tabs).toHaveLength(1);
    expect(store.activeId).toBe("repo-1");
    expect(store.tabs[0]?.status).toEqual(STATUS_1); // populated by refreshStatus
    expect(recents.list().map((entry) => entry.path)).toEqual(["C:/repos/alpha"]);
  });

  it("dedupes by exact path: focuses the existing tab without re-opening", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    await store.openTab("C:/repos/alpha");
    store.setActive("repo-1");

    const focused = await store.openTab("C:/repos/alpha");

    expect(mockOpenRepo).toHaveBeenCalledTimes(1);
    expect(focused.id).toBe("repo-1");
    expect(store.tabs).toHaveLength(1);
    expect(store.activeId).toBe("repo-1");
  });

  it("dedupes by canonical root: a second path resolving to the same root focuses it", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValueOnce(ALPHA);
    await store.openTab("C:/repos/alpha");

    // e.g. a trailing-slash or symlinked variant the backend canonicalizes.
    mockOpenRepo.mockResolvedValueOnce(repoInfo("repo-9", "C:/repos/alpha", "alpha"));
    const focused = await store.openTab("C:/repos/alpha/");

    expect(mockOpenRepo).toHaveBeenCalledTimes(2);
    expect(focused.id).toBe("repo-1");
    expect(store.tabs).toHaveLength(1);
    expect(store.activeId).toBe("repo-1");
  });

  it("opens a second tab and activates it", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValueOnce(ALPHA);
    mockOpenRepo.mockResolvedValueOnce(BETA);

    await store.openTab("C:/repos/alpha");
    await store.openTab("C:/repos/beta");

    expect(store.tabs.map((tab) => tab.id)).toEqual(["repo-1", "repo-2"]);
    expect(store.activeId).toBe("repo-2");
  });

  it("propagates open failures without mutating state", async () => {
    const store = freshStore();
    mockOpenRepo.mockRejectedValue(new Error("not a git repository"));

    await expect(store.openTab("C:/nope")).rejects.toThrow("not a git repository");
    expect(store.tabs).toHaveLength(0);
    expect(store.activeId).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// closeTab / setActive
// ---------------------------------------------------------------------------

describe("closeTab", () => {
  it("notifies the backend and removes the tab", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    await store.openTab("C:/repos/alpha");

    await store.closeTab("repo-1");

    expect(mockCloseRepo).toHaveBeenCalledWith("repo-1");
    expect(store.tabs).toHaveLength(0);
    expect(store.activeId).toBeNull();
  });

  it("switches to the left neighbor when the active tab closes", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValueOnce(ALPHA);
    mockOpenRepo.mockResolvedValueOnce(BETA);
    await store.openTab("C:/repos/alpha");
    await store.openTab("C:/repos/beta");
    expect(store.activeId).toBe("repo-2");

    await store.closeTab("repo-2");

    expect(store.activeId).toBe("repo-1");
    expect(store.tabs.map((tab) => tab.id)).toEqual(["repo-1"]);
  });

  it("falls back to the right neighbor when closing the first tab", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValueOnce(ALPHA);
    mockOpenRepo.mockResolvedValueOnce(BETA);
    await store.openTab("C:/repos/alpha");
    await store.openTab("C:/repos/beta");

    await store.closeTab("repo-1"); // active is repo-2, stays active
    expect(store.activeId).toBe("repo-2");

    mockOpenRepo.mockResolvedValueOnce(ALPHA);
    await store.openTab("C:/repos/alpha");
    await store.closeTab("repo-2"); // repo-1 is left neighbor of repo-2 now
    expect(store.activeId).toBe("repo-1");
  });

  it("keeps the active tab when a background tab closes", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValueOnce(ALPHA);
    mockOpenRepo.mockResolvedValueOnce(BETA);
    await store.openTab("C:/repos/alpha");
    await store.openTab("C:/repos/beta");

    await store.closeTab("repo-1");

    expect(store.activeId).toBe("repo-2");
  });

  it("still removes the tab when the backend close fails", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    await store.openTab("C:/repos/alpha");
    mockCloseRepo.mockRejectedValue(new Error("already gone"));

    await expect(store.closeTab("repo-1")).resolves.toBeUndefined();

    expect(store.tabs).toHaveLength(0);
    expect(store.activeId).toBeNull();
  });

  it("is a no-op for unknown ids", async () => {
    const store = freshStore();
    await expect(store.closeTab("missing")).resolves.toBeUndefined();
    expect(mockCloseRepo).not.toHaveBeenCalled();
  });
});

describe("setActive", () => {
  it("activates a known tab and ignores unknown ids", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValueOnce(ALPHA);
    mockOpenRepo.mockResolvedValueOnce(BETA);
    await store.openTab("C:/repos/alpha");
    await store.openTab("C:/repos/beta");

    store.setActive("repo-1");
    expect(store.activeId).toBe("repo-1");

    store.setActive("unknown");
    expect(store.activeId).toBe("repo-1");
  });
});

// ---------------------------------------------------------------------------
// refreshStatus (rune reactivity: fresh object reference)
// ---------------------------------------------------------------------------

describe("refreshStatus", () => {
  it("replaces the status object reference so rune effects re-run", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    mockRepoStatus.mockResolvedValue(STATUS_1);
    await store.openTab("C:/repos/alpha");

    const before = store.tabs[0]?.status;
    expect(before).toEqual(STATUS_1);

    mockRepoStatus.mockResolvedValue(STATUS_2);
    await store.refreshStatus("repo-1");

    const after = store.tabs[0]?.status;
    expect(after).not.toBe(before); // new reference → $state mutation fires
    expect(after).toEqual(STATUS_2);
  });

  it("swallows status failures and leaves the previous status intact", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    mockRepoStatus.mockResolvedValue(STATUS_1);
    await store.openTab("C:/repos/alpha");

    mockRepoStatus.mockRejectedValue(new Error("repo busy"));
    await expect(store.refreshStatus("repo-1")).resolves.toBeUndefined();

    expect(store.tabs[0]?.status).toEqual(STATUS_1);
  });

  it("is a no-op for unknown ids", async () => {
    const store = freshStore();
    await store.refreshStatus("missing");
    expect(mockRepoStatus).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// startTabEvents (singleton + repo-changed subscription)
// ---------------------------------------------------------------------------

describe("startTabEvents", () => {
  it("subscribes once (idempotent) and refreshes the matching tab on repo-changed", async () => {
    mockOpenRepo.mockResolvedValue(ALPHA);
    mockRepoStatus.mockResolvedValue(STATUS_1);
    await openTab("C:/repos/alpha"); // singleton store
    expect(tabStore.tabs).toHaveLength(1);

    const handlers: Array<(event: RepoChangedEvent) => void> = [];
    mockOnRepoChanged.mockImplementation((cb) => {
      handlers.push(cb);
      return Promise.resolve(() => {});
    });

    startTabEvents();
    startTabEvents();
    expect(mockOnRepoChanged).toHaveBeenCalledTimes(1);

    mockRepoStatus.mockResolvedValue(STATUS_2);
    handlers[0]?.({
      repo_id: "repo-1",
      paths: ["src/a.ts"],
      head_moved: true,
      full: false,
    });

    await vi.waitFor(() => {
      expect(mockRepoStatus).toHaveBeenCalledWith("repo-1");
      expect(tabStore.tabs[0]?.status).toEqual(STATUS_2);
    });
  });

  it("ignores events for unknown repositories", async () => {
    const handlers: Array<(event: RepoChangedEvent) => void> = [];
    mockOnRepoChanged.mockImplementation((cb) => {
      handlers.push(cb);
      return Promise.resolve(() => {});
    });
    startTabEvents();

    handlers[0]?.({ repo_id: "who-is-this", paths: [], head_moved: false, full: true });
    await Promise.resolve(); // flush microtasks

    expect(mockRepoStatus).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// Standalone function API over the singleton
// ---------------------------------------------------------------------------

describe("singleton function API", () => {
  it("getTabs/getActiveId/openTab/closeTab/setActive delegate to the shared store", async () => {
    mockOpenRepo.mockResolvedValue(ALPHA);
    await openTab("C:/repos/alpha");

    expect(tabStore.tabs).toHaveLength(1);
    expect(tabStore.activeId).toBe("repo-1");

    setActive("repo-1");
    expect(tabStore.activeId).toBe("repo-1");

    await closeTab("repo-1");
    expect(tabStore.tabs).toHaveLength(0);
    expect(tabStore.activeId).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Session persistence (relaunch restores the last open tabs)
// ---------------------------------------------------------------------------

/** In-memory session storage double. */
function memSessionStorage(): { getItem: (k: string) => string | null; setItem: (k: string, v: string) => void; data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => void data.set(k, v),
  };
}

describe("session persistence", () => {
  it("persists open roots + the focused root on open/activate/close", async () => {
    const storage = memSessionStorage();
    const store = new TabStore(new RecentRepoStore(noStorage), storage);
    mockOpenRepo.mockImplementation(async (path: string) =>
      path === ALPHA.root ? ALPHA : BETA,
    );

    await store.openTab(ALPHA.root);
    expect(JSON.parse(storage.data.get("mygitui.session")!)).toEqual({
      roots: [ALPHA.root],
      activeRoot: ALPHA.root,
    });

    await store.openTab(BETA.root);
    expect(JSON.parse(storage.data.get("mygitui.session")!)).toEqual({
      roots: [ALPHA.root, BETA.root],
      activeRoot: BETA.root,
    });

    store.setActive("repo-1");
    expect(JSON.parse(storage.data.get("mygitui.session")!).activeRoot).toBe(ALPHA.root);

    await store.closeTab("repo-1");
    expect(JSON.parse(storage.data.get("mygitui.session")!)).toEqual({
      roots: [BETA.root],
      activeRoot: BETA.root,
    });
  });

  it("restoreSession reopens the stored tabs and focuses the stored root", async () => {
    const storage = memSessionStorage();
    storage.data.set(
      "mygitui.session",
      JSON.stringify({ roots: [ALPHA.root, BETA.root], activeRoot: BETA.root }),
    );
    const store = new TabStore(new RecentRepoStore(noStorage), storage);
    mockOpenRepo.mockImplementation(async (path: string) =>
      path === ALPHA.root ? ALPHA : BETA,
    );

    await store.restoreSession();

    expect(store.tabs.map((t) => t.root)).toEqual([ALPHA.root, BETA.root]);
    expect(store.activeId).toBe("repo-2");
  });

  it("restoreSession skips folders that no longer open and never throws", async () => {
    const storage = memSessionStorage();
    storage.data.set(
      "mygitui.session",
      JSON.stringify({ roots: [ALPHA.root, BETA.root], activeRoot: ALPHA.root }),
    );
    const store = new TabStore(new RecentRepoStore(noStorage), storage);
    mockOpenRepo.mockImplementation(async (path: string) => {
      if (path === ALPHA.root) throw new Error("not a git repository");
      return BETA;
    });

    await expect(store.restoreSession()).resolves.toBeUndefined();
    expect(store.tabs.map((t) => t.root)).toEqual([BETA.root]);
    expect(store.activeId).toBe("repo-2");
  });
});

describe("parseStoredSession", () => {
  it("drops malformed payloads", () => {
    expect(parseStoredSession(null)).toEqual({ roots: [], activeRoot: null });
    expect(parseStoredSession("not json")).toEqual({ roots: [], activeRoot: null });
    expect(parseStoredSession('{"roots":"nope"}')).toEqual({ roots: [], activeRoot: null });
    expect(parseStoredSession('{"roots":["ok", 42, ""], "activeRoot": 9}')).toEqual({
      roots: ["ok"],
      activeRoot: null,
    });
  });
});
