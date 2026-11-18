/**
 * Unit tests for the watch-health store (`watchHealth.svelte.ts`): event
 * application, the chip-visibility rule, eviction and the lazy single
 * `repo-watch-state` subscription.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { onRepoWatchState } from "$lib/ipc/client";
import { resetRepoEvents } from "$lib/events/repoEvents";
import { runRepoEvictors } from "$lib/state/repoEvictors";
import type { RepoWatchStateEvent } from "$lib/ipc/types";
import {
  startWatchHealth,
  watchChipKind,
  watchHealth,
  WatchHealthStore,
  type WatchHealth,
} from "./watchHealth.svelte";

vi.mock("$lib/ipc/client", () => ({
  onRepoWatchState: vi.fn(),
  // $lib/events/repoEvents imports the client namespace lazily; only the
  // channel this suite exercises is called.
}));

const mockOnRepoWatchState = vi.mocked(onRepoWatchState);

/** A watch-state event at a fixed mode/alive combination. */
function event(
  repoId: string,
  alive: boolean,
  mode: RepoWatchStateEvent["mode"],
): RepoWatchStateEvent {
  return { repo_id: repoId, alive, mode };
}

beforeEach(() => {
  vi.clearAllMocks();
  mockOnRepoWatchState.mockResolvedValue(() => {});
  resetRepoEvents();
  watchHealth.stop();
});

describe("WatchHealthStore.apply / state", () => {
  it("stores the repo's last reported state with a receive timestamp", () => {
    const store = new WatchHealthStore();
    const before = Date.now();
    store.apply(event("r1", true, "poll"));

    const state = store.state("r1");
    expect(state).not.toBeNull();
    expect(state!.alive).toBe(true);
    expect(state!.mode).toBe("poll");
    expect(state!.since).toBeGreaterThanOrEqual(before);
  });

  it("a fresh open reporting recommended replaces the previous state", () => {
    const store = new WatchHealthStore();
    store.apply(event("r1", true, "poll"));
    store.apply(event("r1", true, "recommended"));

    expect(store.state("r1")!.mode).toBe("recommended");
  });

  it("unknown repos read as null (healthy+recommended emits nothing)", () => {
    const store = new WatchHealthStore();
    expect(store.state("never-reported")).toBeNull();
  });

  it("death and revival both land as plain state updates", () => {
    const store = new WatchHealthStore();
    store.apply(event("r1", false, "dead"));
    expect(store.state("r1")).toMatchObject({ alive: false, mode: "dead" });

    store.apply(event("r1", true, "recommended"));
    expect(store.state("r1")).toMatchObject({ alive: true, mode: "recommended" });
  });
});

describe("watchChipKind (status-bar chip visibility rule)", () => {
  const state = (alive: boolean, mode: WatchHealth["mode"]): WatchHealth => ({
    alive,
    mode,
    since: 0,
  });

  it("healthy + recommended shows no chip (keep the bar calm)", () => {
    expect(watchChipKind(null)).toBeNull();
    expect(watchChipKind(state(true, "recommended"))).toBeNull();
  });

  it("the poll fallback shows the muted chip", () => {
    expect(watchChipKind(state(true, "poll"))).toBe("poll");
  });

  it("a dead watcher shows the danger chip (either signal)", () => {
    expect(watchChipKind(state(false, "dead"))).toBe("dead");
    // Defensive: any !alive state is dead regardless of the mode string.
    expect(watchChipKind(state(false, "recommended"))).toBe("dead");
  });
});

describe("eviction + subscription", () => {
  it("clear drops a closed repo's entry and leaves the others", () => {
    const store = new WatchHealthStore();
    store.apply(event("r1", true, "poll"));
    store.apply(event("r2", false, "dead"));

    store.clear("r1");

    expect(store.state("r1")).toBeNull();
    expect(store.state("r2")).not.toBeNull();
  });

  it("registers with the eviction registry (closeTab choke point clears it)", () => {
    watchHealth.apply(event("r1", true, "poll"));

    // The store module registered its evictor at import time; running the
    // registry (what closeTab does) must drop the singleton's entry.
    runRepoEvictors("r1");

    expect(watchHealth.state("r1")).toBeNull();
  });

  it("clear for an unknown repo is a no-op", () => {
    const store = new WatchHealthStore();
    expect(() => store.clear("ghost")).not.toThrow();
  });

  it("start subscribes once through the event module and applies events", async () => {
    const store = new WatchHealthStore();
    let handler: ((event: RepoWatchStateEvent) => void) | null = null;
    mockOnRepoWatchState.mockImplementation((cb) => {
      handler = cb;
      return Promise.resolve(() => {});
    });

    store.start();
    store.start(); // idempotent — one subscription
    expect(mockOnRepoWatchState).toHaveBeenCalledTimes(1);

    handler!(event("r1", true, "poll"));
    expect(store.state("r1")).toMatchObject({ mode: "poll" });

    store.stop();
    expect(store.state("r1")).toBeNull();
    // A re-start after stop re-registers the listener but does NOT re-open
    // the transport: the module owns the subscription for its lifetime
    // (only resetRepoEvents tears the channel down).
    store.start();
    expect(mockOnRepoWatchState).toHaveBeenCalledTimes(1);

    handler!(event("r1", true, "recommended"));
    expect(store.state("r1")).toMatchObject({ mode: "recommended" });
  });

  it("the singleton's start helper is idempotent", () => {
    startWatchHealth();
    startWatchHealth();
    expect(mockOnRepoWatchState).toHaveBeenCalledTimes(1);
    watchHealth.stop();
  });
});
