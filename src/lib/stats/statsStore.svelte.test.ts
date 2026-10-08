/**
 * Unit tests for the stats store (`$lib/stats/statsStore.svelte`). The IPC
 * client is fully mocked (`vi.mock`) — no Tauri runtime is touched.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { commitActivity, contributorStats } from "$lib/ipc/client";
import type { DayCount } from "$lib/ipc/client";
import { DEFAULT_MAX_DAYS, StatsStore, statsStore } from "./statsStore.svelte";

vi.mock("$lib/ipc/client", () => ({
  commitActivity: vi.fn(),
  contributorStats: vi.fn(),
}));

const mockActivity = vi.mocked(commitActivity);
const mockContributors = vi.mocked(contributorStats);

/** Deferred promise controls (assert intermediate loading states). */
function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => (resolve = res));
  return { promise, resolve };
}

const ACTIVITY: DayCount[] = [
  { day: "2026-10-06", count: 3 },
  { day: "2026-10-07", count: 1 },
];

const CONTRIBUTORS = [
  {
    name: "Alice",
    email: "alice@acme.dev",
    count: 3,
    first_day: "2026-10-06",
    last_day: "2026-10-07",
  },
];

describe("StatsStore", () => {
  beforeEach(() => {
    mockActivity.mockReset();
    mockContributors.mockReset();
    statsStore.clearAll();
  });

  it("starts empty and loads both rollups into the entry", async () => {
    const store = new StatsStore();
    mockActivity.mockResolvedValue(ACTIVITY);
    mockContributors.mockResolvedValue(CONTRIBUTORS);

    await store.load("r1");

    expect(mockActivity).toHaveBeenCalledWith("r1", DEFAULT_MAX_DAYS, null);
    expect(mockContributors).toHaveBeenCalledWith("r1", DEFAULT_MAX_DAYS);
    expect(store.entry("r1")).toMatchObject({
      activity: ACTIVITY,
      contributors: CONTRIBUTORS,
      loading: false,
      error: null,
      author: null,
      maxDays: DEFAULT_MAX_DAYS,
    });
    // $state proxies deep-clone assignments: compare structurally.
    expect(store.activity("r1")).toEqual(ACTIVITY);
    expect(store.contributors("r1")).toEqual(CONTRIBUTORS);
  });

  it("surfaces the loading flag while in flight", async () => {
    const store = new StatsStore();
    const gate = deferred<DayCount[]>();
    mockActivity.mockReturnValue(gate.promise);
    mockContributors.mockResolvedValue([]);

    const pending = store.load("r1");
    expect(store.isLoading("r1")).toBe(true);
    gate.resolve([]);
    await pending;
    expect(store.isLoading("r1")).toBe(false);
  });

  it("forwards a trimmed author filter and records it on the entry", async () => {
    const store = new StatsStore();
    mockActivity.mockResolvedValue([]);
    mockContributors.mockResolvedValue([]);

    await store.load("r1", { author: "  alice  ", maxDays: 30 });

    expect(mockActivity).toHaveBeenCalledWith("r1", 30, "alice");
    expect(store.entry("r1")).toMatchObject({ author: "alice", maxDays: 30 });
  });

  it("treats a blank author as no filter and clamps the window", async () => {
    const store = new StatsStore();
    mockActivity.mockResolvedValue([]);
    mockContributors.mockResolvedValue([]);

    await store.load("r1", { author: "   ", maxDays: 0 });

    expect(mockActivity).toHaveBeenCalledWith("r1", 1, null);
    expect(store.entry("r1").author).toBeNull();
  });

  it("keeps previous data and stores the error on failure", async () => {
    const store = new StatsStore();
    mockActivity.mockResolvedValue(ACTIVITY);
    mockContributors.mockResolvedValue(CONTRIBUTORS);
    await store.load("r1");

    mockActivity.mockRejectedValue(new Error("repo not open"));
    mockContributors.mockRejectedValue("boom");
    await store.load("r1", { author: "alice" });

    expect(store.entry("r1")).toMatchObject({
      activity: ACTIVITY,
      contributors: CONTRIBUTORS,
      loading: false,
      error: "repo not open",
      author: "alice",
    });
  });

  it("dedupes identical in-flight loads", async () => {
    const store = new StatsStore();
    const gate = deferred<DayCount[]>();
    mockActivity.mockReturnValue(gate.promise);
    mockContributors.mockResolvedValue([]);

    const a = store.load("r1");
    const b = store.load("r1");
    expect(a).toBe(b);
    expect(mockActivity).toHaveBeenCalledTimes(1);

    // A different filter is a different key: not deduped.
    void store.load("r1", { author: "alice" });
    expect(mockActivity).toHaveBeenCalledTimes(2);

    gate.resolve([]);
    await Promise.all([a, b]);
  });

  it("clear drops the repo entry", async () => {
    const store = new StatsStore();
    mockActivity.mockResolvedValue([]);
    mockContributors.mockResolvedValue([]);
    await store.load("r1");

    store.clear("r1");
    expect(store.entry("r1").activity).toEqual([]);
    expect(store.isLoading("r1")).toBe(false);
  });
});
