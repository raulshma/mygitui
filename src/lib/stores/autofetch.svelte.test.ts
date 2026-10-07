/**
 * Unit tests for the auto-fetch store (`$lib/stores/autofetch.svelte`).
 * The IPC client is fully mocked (`vi.mock`); timers are vitest fakes.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fetchRepo, remotes } from "$lib/ipc/client";
import type { NetStats, RemoteInfo } from "$lib/ipc/types";
import { getToasts } from "$lib/toast";
import {
  AutofetchStore,
  type StorageLike,
} from "$lib/stores/autofetch.svelte";

vi.mock("$lib/ipc/client", () => ({
  fetchRepo: vi.fn(),
  remotes: vi.fn(),
}));

const mockFetchRepo = vi.mocked(fetchRepo);
const mockRemotes = vi.mocked(remotes);

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
  mockFetchRepo.mockReset();
  mockRemotes.mockReset();
  mockFetchRepo.mockResolvedValue(STATS_EMPTY);
  mockRemotes.mockResolvedValue(REMOTES);
});

afterEach(() => {
  vi.useRealTimers();
});

describe("AutofetchStore", () => {
  it("fetches on the configured cadence with prune and the default remote", async () => {
    const storage = memStorage({
      "mygitui.autofetch": JSON.stringify({ "repo-1": 5 }),
    });
    const store = new AutofetchStore(storage);

    expect(store.getInterval("repo-1")).toBe(5);
    await store.start("repo-1");

    expect(mockRemotes).toHaveBeenCalledWith("repo-1");
    expect(mockFetchRepo).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(5 * 60_000);
    expect(mockFetchRepo).toHaveBeenCalledTimes(1);
    expect(mockFetchRepo).toHaveBeenCalledWith("repo-1", {
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
    await store.start("repo-1");

    expect(mockRemotes).not.toHaveBeenCalled();
    expect(store.isRunning("repo-1")).toBe(false);

    await vi.advanceTimersByTimeAsync(60 * 60_000);
    expect(mockFetchRepo).not.toHaveBeenCalled();
  });

  it("toasts only when refs were updated (silent when up to date)", async () => {
    const storage = memStorage({
      "mygitui.autofetch": JSON.stringify({ "repo-1": 1 }),
    });
    const store = new AutofetchStore(storage);
    await store.start("repo-1");

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
      "mygitui.autofetch": JSON.stringify({ "repo-1": 1 }),
    });
    const store = new AutofetchStore(storage);
    await store.start("repo-1");

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
      "mygitui.autofetch": JSON.stringify({ "repo-1": 10 }),
    });
    const store = new AutofetchStore(storage);
    await store.start("repo-1");

    await vi.advanceTimersByTimeAsync(10 * 60_000);
    expect(mockFetchRepo).toHaveBeenCalledWith("repo-1", {
      remote: "upstream",
      prune: true,
      refs: [],
      depth: null,
    });

    store.stopAll();
  });

  it("stop clears the interval; set to 0 stops a running fetch too", async () => {
    const storage = memStorage({
      "mygitui.autofetch": JSON.stringify({ "repo-1": 1 }),
    });
    const store = new AutofetchStore(storage);
    await store.start("repo-1");
    expect(store.isRunning("repo-1")).toBe(true);

    store.stop("repo-1");
    expect(store.isRunning("repo-1")).toBe(false);

    await vi.advanceTimersByTimeAsync(5 * 60_000);
    expect(mockFetchRepo).not.toHaveBeenCalled();

    await store.start("repo-1");
    store.setInterval("repo-1", 0); // disables while running
    expect(store.isRunning("repo-1")).toBe(false);
    expect(store.getInterval("repo-1")).toBe(0);

    await vi.advanceTimersByTimeAsync(5 * 60_000);
    expect(mockFetchRepo).not.toHaveBeenCalled();
  });

  it("setInterval persists config and restarts with the new cadence", async () => {
    const storage = memStorage();
    const store = new AutofetchStore(storage);

    store.setInterval("repo-1", 5);
    expect(store.getInterval("repo-1")).toBe(5);
    expect(JSON.parse(storage.dump()["mygitui.autofetch"]!)).toEqual({
      "repo-1": 5,
    });

    await store.start("repo-1");
    store.setInterval("repo-1", 1); // restart while running
    await flush(); // the internal restart resolves the remote asynchronously
    expect(store.isRunning("repo-1")).toBe(true);

    await vi.advanceTimersByTimeAsync(60_000);
    expect(mockFetchRepo).toHaveBeenCalledTimes(1);

    // Malformed persisted values are dropped on load.
    const dirty = memStorage({ "mygitui.autofetch": '{"repo-1": "soon", "repo-2": -3}' });
    const dirtyStore = new AutofetchStore(dirty);
    expect(dirtyStore.getInterval("repo-1")).toBe(0);
    expect(dirtyStore.getInterval("repo-2")).toBe(0);

    store.stopAll();
  });
});
