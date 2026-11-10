/**
 * Unit tests for the tab store (`$lib/stores/tabs.svelte`). The IPC client
 * is fully mocked (`vi.mock`) — no Tauri runtime is touched.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  closeRepo,
  onRepoChanged,
  openRepo,
  remoteRepoOpen,
  repoStatus,
} from "$lib/ipc/client";
import type { RepoChangedEvent } from "$lib/ipc/types";
import { resetRepoEvents } from "$lib/events/repoEvents";
import { RecentRepoStore, type StorageLike } from "$lib/entry/recentRepos";
import { terminals } from "$lib/terminal/terminalStore.svelte";
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
  remoteRepoOpen: vi.fn(),
  onRepoChanged: vi.fn(),
  // $lib/events/repoEvents (imported by tabs.svelte) pulls the other event
  // seams in transitively; they are never called in this suite.
  onCloneProgress: vi.fn(),
  onOpProgress: vi.fn(),
  onOpLog: vi.fn(),
  onAuthRequest: vi.fn(),
  onActionOutput: vi.fn(),
  onPtyOutput: vi.fn(),
  onPtyExit: vi.fn(),
}));

const mockOpenRepo = vi.mocked(openRepo);
const mockCloseRepo = vi.mocked(closeRepo);
const mockRepoStatus = vi.mocked(repoStatus);
const mockRemoteRepoOpen = vi.mocked(remoteRepoOpen);
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
    device_id: null,
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
  mockRemoteRepoOpen.mockReset();
  mockOnRepoChanged.mockReset();
  mockCloseRepo.mockResolvedValue(undefined);
  mockRepoStatus.mockResolvedValue(status("main", 0, 0));
  mockOnRepoChanged.mockResolvedValue(() => {});
});

afterEach(() => {
  resetTabStore(); // deregisters the store's listener
  resetRepoEvents(); // drops the module-owned backend subscription
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
    // The status snapshot loads in the background: openTab resolves with
    // `status: null` and the fetched status swaps in reactively.
    expect(store.tabs[0]?.status).toBeNull();
    await vi.waitFor(() => {
      expect(store.tabs[0]?.status).toEqual(STATUS_1);
    });
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

  it("preserves a newer selection while an active tab is closing", async () => {
    const store = freshStore();
    const gamma = repoInfo("repo-3", "C:/repos/gamma", "gamma");
    mockOpenRepo.mockResolvedValueOnce(ALPHA).mockResolvedValueOnce(BETA).mockResolvedValueOnce(gamma);
    await store.openTab(ALPHA.root);
    await store.openTab(BETA.root);
    await store.openTab(gamma.root);
    let finishClose!: () => void;
    mockCloseRepo.mockImplementation(() => new Promise((resolve) => (finishClose = resolve)));

    const closing = store.closeTab(gamma.repo_id);
    store.setActive(ALPHA.repo_id);
    finishClose();
    await closing;

    expect(store.activeId).toBe(ALPHA.repo_id);
  });

  it("selects a neighbor when a closing background tab becomes active", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValueOnce(ALPHA).mockResolvedValueOnce(BETA);
    await store.openTab(ALPHA.root);
    await store.openTab(BETA.root);
    let finishClose!: () => void;
    mockCloseRepo.mockImplementation(() => new Promise((resolve) => (finishClose = resolve)));

    const closing = store.closeTab(ALPHA.repo_id);
    store.setActive(ALPHA.repo_id);
    finishClose();
    await closing;

    expect(store.activeId).toBe(BETA.repo_id);
    expect(store.active).not.toBeNull();
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
    // The background fetch from openTab must have landed before the swap.
    await vi.waitFor(() => {
      expect(store.tabs[0]?.status).toEqual(STATUS_1);
    });

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
    await vi.waitFor(() => {
      expect(store.tabs[0]?.status).toEqual(STATUS_1);
    });

    mockRepoStatus.mockRejectedValue(new Error("repo busy"));
    await expect(store.refreshStatus("repo-1")).resolves.toBeUndefined();

    expect(store.tabs[0]?.status).toEqual(STATUS_1);
  });

  it("coalesces a burst into one fetch plus one trailing fetch", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    mockRepoStatus.mockResolvedValue(STATUS_1);
    await store.openTab("C:/repos/alpha");
    await vi.waitFor(() => {
      expect(store.tabs[0]?.status).toEqual(STATUS_1);
    });

    let release!: (value: typeof STATUS_1) => void;
    const first = new Promise<typeof STATUS_1>((resolve) => {
      release = resolve;
    });
    mockRepoStatus.mockReset();
    mockRepoStatus.mockReturnValueOnce(first).mockResolvedValue(STATUS_2);

    const a = store.refreshStatus("repo-1");
    const b = store.refreshStatus("repo-1");
    const c = store.refreshStatus("repo-1");
    expect(mockRepoStatus).toHaveBeenCalledTimes(1);

    release(STATUS_1);
    await Promise.all([a, b, c]);
    // One running fetch + the trailing fetch queued by the burst.
    expect(mockRepoStatus).toHaveBeenCalledTimes(2);
    expect(store.tabs[0]?.status).toEqual(STATUS_2);
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

  it("restoreSession keeps strip order when opens resolve out of order", async () => {
    const storage = memSessionStorage();
    storage.data.set(
      "mygitui.session",
      JSON.stringify({ roots: [ALPHA.root, BETA.root], activeRoot: ALPHA.root }),
    );
    const store = new TabStore(new RecentRepoStore(noStorage), storage);
    // BETA opens immediately; ALPHA hangs until released — resolve order
    // must not reshuffle the strip (insert at the stored index instead).
    let releaseAlpha!: (info: typeof ALPHA) => void;
    mockOpenRepo.mockImplementation((path: string) =>
      path === ALPHA.root
        ? new Promise((resolve) => (releaseAlpha = resolve))
        : Promise.resolve(BETA),
    );

    const restored = store.restoreSession();
    await vi.waitFor(() => expect(store.tabs.map((t) => t.root)).toEqual([BETA.root]));
    releaseAlpha(ALPHA);
    await restored;

    expect(store.tabs.map((t) => t.root)).toEqual([ALPHA.root, BETA.root]);
    expect(store.activeId).toBe("repo-1");
  });

  it.each([
    [0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0],
  ])("restores three tabs in stored order after completion order %s, %s, %s", async (...order) => {
    const repos = [ALPHA, BETA, repoInfo("repo-3", "C:/repos/gamma", "gamma")];
    const storage = memSessionStorage();
    const roots = repos.map((repo) => repo.root);
    storage.data.set("mygitui.session", JSON.stringify({ roots, activeRoot: ALPHA.root }));
    const store = new TabStore(new RecentRepoStore(noStorage), storage);
    const releases = new Map<string, (info: typeof ALPHA) => void>();
    mockOpenRepo.mockImplementation((path) => new Promise((resolve) => releases.set(path, resolve)));

    const restored = store.restoreSession();
    for (const index of order) {
      const repo = repos[index]!;
      releases.get(repo.root)!(repo);
      await vi.waitFor(() => expect(store.tabs.some((tab) => tab.id === repo.repo_id)).toBe(true));
    }
    await restored;

    expect(store.tabs.map((tab) => tab.root)).toEqual(roots);
    expect(JSON.parse(storage.data.get("mygitui.session")!).roots).toEqual(roots);
  });

  it("does not replace an explicit selection when the stored active tab opens late", async () => {
    const storage = memSessionStorage();
    storage.data.set("mygitui.session", JSON.stringify({ roots: [ALPHA.root, BETA.root], activeRoot: ALPHA.root }));
    const store = new TabStore(new RecentRepoStore(noStorage), storage);
    let releaseAlpha!: (info: typeof ALPHA) => void;
    mockOpenRepo.mockImplementation((path) => path === ALPHA.root
      ? new Promise((resolve) => (releaseAlpha = resolve))
      : Promise.resolve(BETA));

    const restored = store.restoreSession();
    await vi.waitFor(() => expect(store.tabs).toHaveLength(1));
    store.setActive(BETA.repo_id);
    releaseAlpha(ALPHA);
    await restored;

    expect(store.activeId).toBe(BETA.repo_id);
  });

  it("restoreSession opens tabs in parallel, not one after another", async () => {
    const storage = memSessionStorage();
    storage.data.set(
      "mygitui.session",
      JSON.stringify({ roots: [ALPHA.root, BETA.root], activeRoot: BETA.root }),
    );
    const store = new TabStore(new RecentRepoStore(noStorage), storage);
    // Both opens hang until both have been *issued*: a serial restore would
    // never reach the second open before the first resolves.
    const issued: string[] = [];
    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));
    mockOpenRepo.mockImplementation((path: string) => {
      issued.push(path);
      return gate.then(() => (path === ALPHA.root ? ALPHA : BETA));
    });

    const restored = store.restoreSession();
    await vi.waitFor(() => expect(issued).toHaveLength(2));
    release();
    await restored;

    expect(store.tabs).toHaveLength(2);
    expect(store.activeId).toBe("repo-2");
  });

  it("starts held in the launch state until the stored session settles", async () => {
    const storage = memSessionStorage();
    storage.data.set(
      "mygitui.session",
      JSON.stringify({ roots: [ALPHA.root], activeRoot: ALPHA.root }),
    );
    const store = new TabStore(new RecentRepoStore(noStorage), storage);
    // Decided at construction so the first paint already shows the launch
    // view instead of the recents home.
    expect(store.restoring).toBe(true);
    expect(store.restoreRoot).toBe(ALPHA.root);

    let release!: (info: typeof ALPHA) => void;
    mockOpenRepo.mockImplementation(
      () => new Promise((resolve) => (release = resolve)),
    );
    const restored = store.restoreSession();
    expect(store.restoring).toBe(true);
    release(ALPHA);
    await restored;

    expect(store.restoring).toBe(false);
    expect(store.restoreRoot).toBeNull();
    expect(store.activeId).toBe("repo-1");
  });

  it("starts ready (no launch state) without a stored session", () => {
    const store = new TabStore(new RecentRepoStore(noStorage), memSessionStorage());
    expect(store.restoring).toBe(false);
    expect(store.restoreRoot).toBeNull();
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
    // The stored active root (alpha) is gone — fall back to the first
    // restored tab instead of landing on the home view.
    expect(store.activeId).toBe("repo-2");
  });
});

describe("recents overlay", () => {
  it("keeps tabs open while showing recents; activating a tab returns", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    await store.openTab(ALPHA.root);

    store.showRecentRepos();
    expect(store.recentsOpen).toBe(true);
    expect(store.tabs).toHaveLength(1);

    store.setActive("repo-1");
    expect(store.recentsOpen).toBe(false);

    store.showRecentRepos();
    await store.openTab(ALPHA.root); // dedupes into the open tab
    expect(store.recentsOpen).toBe(false);
  });

  it("hideRecentRepos closes the overlay without touching tabs", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    await store.openTab(ALPHA.root);
    store.showRecentRepos();
    store.hideRecentRepos();
    expect(store.recentsOpen).toBe(false);
    expect(store.tabs).toHaveLength(1);
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

// ---------------------------------------------------------------------------
// openRemoteTab / retryRemoteTab (state-machine audit regressions)
// ---------------------------------------------------------------------------

describe("openRemoteTab", () => {
  const URI = "ssh://dev-1/srv/app";
  function remoteInfo(id = "remote-repo-1") {
    return {
      repo_id: id,
      root: URI,
      name: "app",
      bare: false,
      git_dir: "/srv/app/.git",
      device_id: "dev-1",
    };
  }

  it("opens a remote repo as a tab keyed by the ssh:// URI", async () => {
    const store = freshStore();
    mockRemoteRepoOpen.mockResolvedValue(remoteInfo());
    mockRepoStatus.mockResolvedValue(status("main", 0, 0));

    const tab = await store.openRemoteTab("dev-1", "/srv/app");

    expect(mockRemoteRepoOpen).toHaveBeenCalledWith("dev-1", "/srv/app");
    expect(tab).toMatchObject({ id: "remote-repo-1", root: URI, deviceId: "dev-1" });
    expect(store.tabs).toHaveLength(1);
    expect(store.activeId).toBe("remote-repo-1");
  });

  it("a backend failure opens an offline tab; the retry upgrades it in place", async () => {
    const store = freshStore();
    mockRemoteRepoOpen.mockRejectedValueOnce(new Error("device offline"));
    const offline = await store.openRemoteTab("dev-1", "/srv/app");

    expect(offline.openError).toBe("device offline");
    expect(offline.id).toBe(URI); // no backend repo: the URI is the tab id
    expect(store.tabs).toHaveLength(1);

    mockRemoteRepoOpen.mockResolvedValueOnce(remoteInfo());
    await store.retryRemoteTab(URI);

    expect(store.tabs).toHaveLength(1); // same strip position, no duplicate
    expect(store.tabs[0]).toMatchObject({ id: "remote-repo-1", openError: null });
    expect(store.activeId).toBe("remote-repo-1");
  });

  it("a failed retry does not clobber a tab a concurrent call opened (old code spliced at -1)", async () => {
    const store = freshStore();
    // Seed: a local tab FIRST, then the offline remote tab. The local tab
    // is the victim the old stale-reference `splice(-1, 1, …)` deleted.
    mockOpenRepo.mockResolvedValue(BETA);
    await store.openTab("C:/repos/beta");
    mockRemoteRepoOpen.mockRejectedValueOnce(new Error("device offline"));
    await store.openRemoteTab("dev-1", "/srv/app");
    expect(store.tabs.map((t) => t.id)).toEqual(["repo-2", URI]);

    // Double-clicked Retry: call A hangs, call B fails fast.
    let resolveA!: (value: ReturnType<typeof remoteInfo>) => void;
    mockRemoteRepoOpen.mockImplementationOnce(
      () => new Promise((resolve) => (resolveA = resolve)),
    );
    mockRemoteRepoOpen.mockRejectedValueOnce(new Error("still offline"));
    const attemptA = store.openRemoteTab("dev-1", "/srv/app");
    const attemptB = store.openRemoteTab("dev-1", "/srv/app");
    resolveA(remoteInfo());
    await Promise.all([attemptA, attemptB]);

    // Exactly one remote tab, open, and the unrelated local tab survived.
    expect(store.tabs.map((t) => t.id)).toEqual(["repo-2", "remote-repo-1"]);
    expect(store.tabs[1]?.openError).toBeNull();
  });

  it("retries on a tab that is already open are no-ops", async () => {
    const store = freshStore();
    mockRemoteRepoOpen.mockResolvedValue(remoteInfo());
    await store.openRemoteTab("dev-1", "/srv/app");

    await store.retryRemoteTab(URI);

    expect(mockRemoteRepoOpen).toHaveBeenCalledTimes(1);
    expect(store.tabs).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// statusError (the no-error-state machine: stuck "Loading changes…" spinner)
// ---------------------------------------------------------------------------

describe("refreshStatus statusError", () => {
  it("a failed first status lands in statusError; a later success clears it", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    mockRepoStatus.mockRejectedValueOnce(new Error("op queue wedged"));

    await store.openTab("C:/repos/alpha");

    await vi.waitFor(() => {
      expect(store.tabs[0]?.statusError).toBe("op queue wedged");
    });
    expect(store.tabs[0]?.status).toBeNull();

    mockRepoStatus.mockResolvedValue(STATUS_1);
    await store.refreshStatus("repo-1");
    expect(store.tabs[0]?.statusError).toBeNull();
    expect(store.tabs[0]?.status).toEqual(STATUS_1);
  });

  it("a failed refresh on a loaded tab keeps the status and sets no error", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    mockRepoStatus.mockResolvedValueOnce(STATUS_1);
    await store.openTab("C:/repos/alpha");
    await vi.waitFor(() => expect(store.tabs[0]?.status).toEqual(STATUS_1));

    mockRepoStatus.mockRejectedValueOnce(new Error("boom"));
    await store.refreshStatus("repo-1");

    expect(store.tabs[0]?.status).toEqual(STATUS_1);
    expect(store.tabs[0]?.statusError).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// closeTab terminal teardown (M5 contract: the FE must pty_kill per closed tab)
// ---------------------------------------------------------------------------

describe("closeTab terminal teardown", () => {
  it("closing a tab disposes the repo's terminal session", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    await store.openTab("C:/repos/alpha");
    const dispose = vi.spyOn(terminals, "dispose").mockImplementation(() => {});

    await store.closeTab("repo-1");

    expect(dispose).toHaveBeenCalledWith("repo-1");
    dispose.mockRestore();
  });

  it("closing a tab with no terminal session still closes cleanly", async () => {
    const store = freshStore();
    mockOpenRepo.mockResolvedValue(ALPHA);
    await store.openTab("C:/repos/alpha");

    await store.closeTab("repo-1");

    expect(mockCloseRepo).toHaveBeenCalledWith("repo-1");
    expect(store.tabs).toHaveLength(0);
  });
});
