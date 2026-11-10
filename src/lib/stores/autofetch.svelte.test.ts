/**
 * Unit tests for the auto-fetch store (`$lib/stores/autofetch.svelte`).
 * The IPC client is fully mocked (`vi.mock`); timers are vitest fakes.
 * Config is root-keyed (persistence survives restarts); `start` gets the
 * root as a hint because the tabs store is empty under test.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { branches, fetchRepo, remotes } from "$lib/ipc/client";
import type { BranchInfo, NetStats, RemoteInfo } from "$lib/ipc/types";
import { getToasts } from "$lib/toast";
import {
  AutofetchStore,
  upstreamMoved,
  UPSTREAM_MOVED_TOAST_CAP,
  type StorageLike,
} from "$lib/stores/autofetch.svelte";

// tabs.svelte (imported for the id → root lookup) needs these at module
// evaluation; the autofetch tests never drive them.
vi.mock("$lib/ipc/client", () => ({
  branches: vi.fn(),
  fetchRepo: vi.fn(),
  remotes: vi.fn(),
  openRepo: vi.fn(),
  closeRepo: vi.fn(),
  remoteRepoOpen: vi.fn(),
  repoStatus: vi.fn(),
}));

const mockBranches = vi.mocked(branches);
const mockFetchRepo = vi.mocked(fetchRepo);
const mockRemotes = vi.mocked(remotes);

/** Test repo (id minted per session, like production) and its root key. */
const REPO_ID = "repo-1";
const ROOT = "/repos/alpha";

/** In-memory storage (hermetic; records writes). */
function memStorage(initial: Record<string, string> = {}): StorageLike & {
  dump(): Record<string, string>;
} {
  const data = { ...initial };
  return {
    getItem: (key) => data[key] ?? null,
    setItem: (key, value) => {
      data[key] = value;
    },
    dump: () => data,
  };
}

const REMOTES: RemoteInfo[] = [
  { name: "origin", url: "https://host/o.git", push_url: null },
];

const STATS_UPDATED: NetStats = {
  received_bytes: 1024,
  objects: 7,
  updated_refs: [
    ["refs/heads/main", "abc"],
    ["refs/heads/feature", "def"],
  ],
};

const STATS_EMPTY: NetStats = {
  received_bytes: 0,
  objects: 0,
  updated_refs: [],
};

function errorToasts(): number {
  return getToasts().filter((t) => t.kind === "error").length;
}

/** Flushes pending microtasks (remote resolution inside start/restart). */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  vi.useFakeTimers();
  mockBranches.mockReset();
  mockFetchRepo.mockReset();
  mockRemotes.mockReset();
  mockBranches.mockResolvedValue([]);
  mockFetchRepo.mockResolvedValue(STATS_EMPTY);
  mockRemotes.mockResolvedValue(REMOTES);
});

afterEach(() => {
  vi.useRealTimers();
});

describe("AutofetchStore", () => {
  it("fetches on the configured cadence with prune and the default remote", async () => {
    const storage = memStorage({
      "mygitui.autofetch": JSON.stringify({ [ROOT]: 5 }),
    });
    const store = new AutofetchStore(storage);

    expect(store.getInterval(ROOT)).toBe(5);
    await store.start(REPO_ID, ROOT);

    expect(mockRemotes).toHaveBeenCalledWith(REPO_ID);
    expect(mockFetchRepo).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(5 * 60_000);
    expect(mockFetchRepo).toHaveBeenCalledTimes(1);
    expect(mockFetchRepo).toHaveBeenCalledWith(REPO_ID, {
      remote: "origin",
      prune: true,
      refs: [],
      depth: null,
    });

    await vi.advanceTimersByTimeAsync(5 * 60_000);
    expect(mockFetchRepo).toHaveBeenCalledTimes(2);

    store.stopAll();
  });

  it("start is a no-op without a configured interval", async () => {
    const store = new AutofetchStore(memStorage());
    await store.start(REPO_ID, ROOT);

    expect(mockRemotes).not.toHaveBeenCalled();
    expect(store.isRunning(REPO_ID)).toBe(false);

    await vi.advanceTimersByTimeAsync(60 * 60_000);
    expect(mockFetchRepo).not.toHaveBeenCalled();
  });

  it("toasts only when refs were updated (silent when up to date)", async () => {
    const storage = memStorage({
      "mygitui.autofetch": JSON.stringify({ [ROOT]: 1 }),
    });
    const store = new AutofetchStore(storage);
    await store.start(REPO_ID, ROOT);

    mockFetchRepo.mockResolvedValueOnce(STATS_UPDATED);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(
      getToasts().some((t) => t.message === "origin updated: 2 refs"),
    ).toBe(true);

    // Up-to-date fetch → no NEW toast (fake-time advancement may have
    // auto-dismissed earlier toasts, so compare against known ids).
    const idsBefore = new Set(getToasts().map((t) => t.id));
    mockFetchRepo.mockResolvedValueOnce(STATS_EMPTY);
    await vi.advanceTimersByTimeAsync(60_000);
    const fresh = getToasts().filter((t) => !idsBefore.has(t.id));
    expect(fresh).toEqual([]);

    store.stopAll();
  });

  it("toasts errors when the fetch fails", async () => {
    const storage = memStorage({
      "mygitui.autofetch": JSON.stringify({ [ROOT]: 1 }),
    });
    const store = new AutofetchStore(storage);
    await store.start(REPO_ID, ROOT);

    const before = errorToasts();
    mockFetchRepo.mockRejectedValueOnce(new Error("no network") as never);
    await vi.advanceTimersByTimeAsync(60_000);

    expect(errorToasts()).toBe(before + 1);
    expect(
      getToasts().some((t) => t.message.includes("Auto-fetch failed: no network")),
    ).toBe(true);

    store.stopAll();
  });

  it("falls back to the first remote when origin is absent", async () => {
    mockRemotes.mockResolvedValue([
      { name: "upstream", url: "https://host/u.git", push_url: null },
    ]);
    const storage = memStorage({
      "mygitui.autofetch": JSON.stringify({ [ROOT]: 10 }),
    });
    const store = new AutofetchStore(storage);
    await store.start(REPO_ID, ROOT);

    await vi.advanceTimersByTimeAsync(10 * 60_000);
    expect(mockFetchRepo).toHaveBeenCalledWith(REPO_ID, {
      remote: "upstream",
      prune: true,
      refs: [],
      depth: null,
    });

    store.stopAll();
  });

  it("stop clears the interval; set to 0 stops a running fetch too", async () => {
    const storage = memStorage({
      "mygitui.autofetch": JSON.stringify({ [ROOT]: 1 }),
    });
    const store = new AutofetchStore(storage);
    await store.start(REPO_ID, ROOT);
    expect(store.isRunning(REPO_ID)).toBe(true);

    store.stop(REPO_ID);
    expect(store.isRunning(REPO_ID)).toBe(false);

    await vi.advanceTimersByTimeAsync(5 * 60_000);
    expect(mockFetchRepo).not.toHaveBeenCalled();

    await store.start(REPO_ID, ROOT);
    store.setInterval(ROOT, 0); // disables while running
    expect(store.isRunning(REPO_ID)).toBe(false);
    expect(store.getInterval(ROOT)).toBe(0);

    await vi.advanceTimersByTimeAsync(5 * 60_000);
    expect(mockFetchRepo).not.toHaveBeenCalled();
  });

  it("setInterval persists config and restarts with the new cadence", async () => {
    const storage = memStorage();
    const store = new AutofetchStore(storage);

    store.setInterval(ROOT, 5);
    expect(store.getInterval(ROOT)).toBe(5);
    expect(JSON.parse(storage.dump()["mygitui.autofetch"]!)).toEqual({
      [ROOT]: 5,
    });

    await store.start(REPO_ID, ROOT);
    store.setInterval(ROOT, 1); // restart while running
    await flush(); // the internal restart resolves the remote asynchronously
    expect(store.isRunning(REPO_ID)).toBe(true);

    await vi.advanceTimersByTimeAsync(60_000);
    expect(mockFetchRepo).toHaveBeenCalledTimes(1);

    // Malformed persisted values are dropped on load.
    const dirty = memStorage({ "mygitui.autofetch": `{"${ROOT}": "soon", "/repos/beta": -3}` });
    const dirtyStore = new AutofetchStore(dirty);
    expect(dirtyStore.getInterval(ROOT)).toBe(0);
    expect(dirtyStore.getInterval("/repos/beta")).toBe(0);

    store.stopAll();
  });

  it("drops legacy per-session repo-id keys (settings are root-keyed now)", () => {
    // Pre-rekey configs stored `{ [repoId]: minutes }`; ids don't survive a
    // restart, so those entries are unreachable garbage — drop them.
    const storage = memStorage({
      "mygitui.autofetch": JSON.stringify({ "repo-1": 5, "repo-2": 15 }),
    });
    const store = new AutofetchStore(storage);
    expect(store.getInterval("repo-1")).toBe(0);
    expect(store.getInterval("repo-2")).toBe(0);
    expect(store.getInterval(ROOT)).toBe(0);
  });

  it("keeps ssh:// tab URIs as persistence keys (remote repos)", async () => {
    const uri = "ssh://device-1/srv/git/alpha";
    const storage = memStorage({
      "mygitui.autofetch": JSON.stringify({ [uri]: 30 }),
    });
    const store = new AutofetchStore(storage);

    expect(store.getInterval(uri)).toBe(30);
    await store.start(REPO_ID, uri);
    expect(store.isRunning(REPO_ID)).toBe(true);

    store.stopAll();
  });
});

// ---------------------------------------------------------------------------
// M12: upstream-moved toasts
// ---------------------------------------------------------------------------

function branch(name: string, behind: number, gone = false): BranchInfo {
  return {
    name,
    sha: "a".repeat(40),
    upstream: `origin/${name}`,
    ahead: 0,
    behind,
    gone,
    is_head: name === "main",
  };
}

describe("upstreamMoved (M12 pure)", () => {
  it("toasts branches that appear behind or increased, skips equal/decreasing", () => {
    const previous = new Map<string, number>([
      ["same", 2],
      ["more", 1],
      ["less", 5],
    ]);
    const list = [
      branch("same", 2), // unchanged → silent
      branch("more", 4), // increased → toast
      branch("less", 3), // decreased → silent
      branch("new", 1), // appeared → toast
      branch("gone", 9, true), // upstream gone → silent
      branch("synced", 0), // not behind → silent
    ];

    const { messages, current } = upstreamMoved(previous, list);
    expect(messages).toEqual([
      "upstream moved: more is 4 behind",
      "upstream moved: new is 1 behind",
    ]);
    // Current counts keep only still-behind branches (baseline for next time).
    expect([...current.entries()].sort()).toEqual([
      ["less", 3],
      ["more", 4],
      ["new", 1],
      ["same", 2],
    ]);
  });

  it("caps messages at the per-cycle toast cap", () => {
    const list = Array.from({ length: UPSTREAM_MOVED_TOAST_CAP + 2 }, (_, i) =>
      branch(`b${i}`, 1),
    );
    const { messages } = upstreamMoved(new Map(), list);
    expect(messages).toHaveLength(UPSTREAM_MOVED_TOAST_CAP);
  });

  it("a branch that disappears from the baseline toasts again when it reappears", () => {
    const first = upstreamMoved(new Map(), [branch("x", 2)]);
    expect(first.messages).toEqual(["upstream moved: x is 2 behind"]);

    // Next cycle: branch caught up (no toast)…
    const second = upstreamMoved(first.current, [branch("x", 0)]);
    expect(second.messages).toEqual([]);

    // …then fell behind again → toast fires (count "reappeared").
    const third = upstreamMoved(second.current, [branch("x", 1)]);
    expect(third.messages).toEqual(["upstream moved: x is 1 behind"]);
  });
});

describe("reportUpstreamMoved (store integration)", () => {
  it("toasts upstream-moved after a fetch that updated refs", async () => {
    const store = new AutofetchStore(memStorage({
      "mygitui.autofetch": JSON.stringify({ [ROOT]: 1 }),
    }));
    await store.start(REPO_ID, ROOT);

    mockFetchRepo.mockResolvedValueOnce(STATS_UPDATED);
    mockBranches.mockResolvedValueOnce([
      branch("main", 0),
      branch("feature", 3),
    ]);

    await vi.advanceTimersByTimeAsync(60_000);
    expect(mockBranches).toHaveBeenCalledWith(REPO_ID);
    expect(
      getToasts().some((t) => t.message === "upstream moved: feature is 3 behind"),
    ).toBe(true);

    // Same behind-count next cycle → no repeat "upstream moved" toast
    // (the plain "origin updated" toast still fires — that is by design).
    const idsBefore = new Set(getToasts().map((t) => t.id));
    mockFetchRepo.mockResolvedValueOnce(STATS_UPDATED);
    mockBranches.mockResolvedValueOnce([branch("feature", 3)]);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(
      getToasts()
        .filter((t) => !idsBefore.has(t.id))
        .filter((t) => t.message.startsWith("upstream moved")),
    ).toEqual([]);

    store.stopAll();
  });

  it("stays silent when the branch list cannot be read", async () => {
    const store = new AutofetchStore(memStorage({
      "mygitui.autofetch": JSON.stringify({ [ROOT]: 1 }),
    }));
    await store.start(REPO_ID, ROOT);

    mockFetchRepo.mockResolvedValueOnce(STATS_UPDATED);
    mockBranches.mockRejectedValueOnce(new Error("repo gone") as never);

    await vi.advanceTimersByTimeAsync(60_000);
    // The "origin updated" toast still fires; no crash, no upstream toast.
    expect(
      getToasts().some((t) => t.message === "origin updated: 2 refs"),
    ).toBe(true);
    expect(
      getToasts().some((t) => t.message.startsWith("upstream moved")),
    ).toBe(false);

    store.stopAll();
  });
});

describe("AutofetchStore start/stop races", () => {
  /** Storage with a 1-minute cadence for ROOT. */
  function armed(): AutofetchStore {
    return new AutofetchStore(memStorage({
      "mygitui.autofetch": JSON.stringify({ [ROOT]: 1 }),
    }));
  }

  it("a stop during the remote lookup does not leave an orphaned timer", async () => {
    const store = armed();
    let release: (value: RemoteInfo[]) => void = () => {};
    mockRemotes.mockImplementationOnce(
      () => new Promise<RemoteInfo[]>((resolve) => { release = resolve; }),
    );

    const starting = store.start(REPO_ID, ROOT);
    store.stop(REPO_ID); // nothing registered yet — must still tombstone
    release(REMOTES);
    await starting;

    expect(store.isRunning(REPO_ID)).toBe(false);
    await vi.advanceTimersByTimeAsync(10 * 60_000);
    expect(mockFetchRepo).not.toHaveBeenCalled();
  });

  it("a stop during an in-flight fetch suppresses its toast", async () => {
    const store = armed();
    await store.start(REPO_ID, ROOT);
    const before = getToasts().length;

    let release: (value: NetStats) => void = () => {};
    mockFetchRepo.mockImplementationOnce(
      () => new Promise<NetStats>((resolve) => { release = resolve; }),
    );
    await vi.advanceTimersByTimeAsync(60_000);
    store.stop(REPO_ID);
    release(STATS_UPDATED);
    await flush();

    expect(getToasts().length).toBe(before);
    expect(mockBranches).not.toHaveBeenCalled();
  });

  it("a failed fetch after stop stays silent", async () => {
    const store = armed();
    await store.start(REPO_ID, ROOT);
    const before = errorToasts();

    let fail: (err: Error) => void = () => {};
    mockFetchRepo.mockImplementationOnce(
      () => new Promise<NetStats>((_resolve, reject) => { fail = reject; }),
    );
    await vi.advanceTimersByTimeAsync(60_000);
    store.stop(REPO_ID);
    fail(new Error("repo gone"));
    await flush();

    expect(errorToasts()).toBe(before);
  });

  it("of two concurrent starts only the newer one registers", async () => {
    const store = armed();
    const resolvers: Array<(value: RemoteInfo[]) => void> = [];
    mockRemotes.mockImplementation(
      () => new Promise<RemoteInfo[]>((resolve) => { resolvers.push(resolve); }),
    );

    const first = store.start(REPO_ID, ROOT);
    const second = store.start(REPO_ID, ROOT);
    for (const resolve of resolvers) resolve(REMOTES);
    await Promise.all([first, second]);

    expect(store.isRunning(REPO_ID)).toBe(true);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(mockFetchRepo).toHaveBeenCalledTimes(1);
    store.stopAll();
  });

  it("a stop during the branch read does not resurrect the behind map", async () => {
    const store = armed();
    await store.start(REPO_ID, ROOT);
    mockFetchRepo.mockResolvedValueOnce(STATS_UPDATED);
    let release: (value: BranchInfo[]) => void = () => {};
    mockBranches.mockImplementationOnce(
      () => new Promise<BranchInfo[]>((resolve) => { release = resolve; }),
    );
    await vi.advanceTimersByTimeAsync(60_000);
    await flush();
    store.stop(REPO_ID);
    const before = getToasts().length;
    release([{ name: "feature", behind: 3 } as BranchInfo]);
    await flush();

    expect(getToasts().length).toBe(before);
    store.stopAll();
  });
});
