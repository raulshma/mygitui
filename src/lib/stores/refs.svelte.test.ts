/**
 * Unit tests for the shared refs cache (`$lib/stores/refs.svelte`). The
 * IPC client is fully mocked (`vi.mock`, the history.svelte suite's seam
 * list) — no Tauri runtime is touched. `onRepoChanged` handlers are
 * captured so tests drive the cache's invalidation wiring through the
 * real `$lib/events/repoEvents` fan-out (kind derivation + the cache's
 * `isSignificant` filter run for real); `resetRepoEvents` drops the
 * module-owned backend subscription between tests.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { branches, onRepoChanged, remotes } from "$lib/ipc/client";
import { resetRepoEvents } from "$lib/events/repoEvents";
import type { BranchInfo, RepoChangedEvent } from "$lib/ipc/types";
import { RefsCache, REFS_CACHE_TTL_MS } from "$lib/stores/refs.svelte";

vi.mock("$lib/ipc/client", () => ({
  branches: vi.fn(),
  remotes: vi.fn(),
  onRepoChanged: vi.fn(),
  // $lib/events/repoEvents (imported by refs.svelte) pulls the other event
  // seams in transitively; they are never called in this suite.
  onCloneProgress: vi.fn(),
  onOpProgress: vi.fn(),
  onOpLog: vi.fn(),
  onAuthRequest: vi.fn(),
  onActionOutput: vi.fn(),
  onPtyOutput: vi.fn(),
  onPtyExit: vi.fn(),
}));

const mockBranches = vi.mocked(branches);
const mockRemotes = vi.mocked(remotes);
const mockOnRepoChanged = vi.mocked(onRepoChanged);

/** Raw `repo-changed` callbacks captured from the mocked client seam. */
const wireHandlers: Array<(event: RepoChangedEvent) => void> = [];

function branchList(...names: string[]) {
  return names.map((name, i) => ({
    name,
    sha: `${i}${"a".repeat(39)}`,
    upstream: null,
    ahead: 0,
    behind: 0,
    gone: false,
    is_head: i === 0,
  }));
}

function remoteList(...names: string[]) {
  return names.map((name) => ({ name, url: `https://${name}/x`, push_url: null }));
}

/**
 * A manually settled `branches` promise, typed for the mock so the
 * `mockReturnValue` call sites need no cast. Tests park a scan mid-flight
 * by holding the settle handles and resolving/rejecting it explicitly.
 */
function parkedBranches(): {
  promise: Promise<BranchInfo[]>;
  resolve: (list: BranchInfo[]) => void;
  reject: (err: Error) => void;
} {
  let resolve!: (list: BranchInfo[]) => void;
  let reject!: (err: Error) => void;
  const promise = new Promise<BranchInfo[]>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Delivers one watcher payload through the repoEvents fan-out. */
function emitChange(patch: Partial<RepoChangedEvent> = {}): void {
  const event: RepoChangedEvent = {
    repo_id: "r1",
    paths: [],
    head_moved: false,
    full: false,
    ...patch,
  };
  for (const handler of wireHandlers) handler(event);
}

beforeEach(() => {
  wireHandlers.length = 0;
  mockBranches.mockReset();
  mockRemotes.mockReset();
  mockOnRepoChanged.mockReset();
  mockBranches.mockResolvedValue(branchList("main"));
  mockRemotes.mockResolvedValue(remoteList("origin"));
  mockOnRepoChanged.mockImplementation(
    ((cb: (event: RepoChangedEvent) => void) => {
      wireHandlers.push(cb);
      return Promise.resolve(() => {});
    }) as typeof onRepoChanged,
  );
});

afterEach(() => {
  vi.useRealTimers();
  resetRepoEvents(); // drops the module-owned backend subscription
});

/** Flushes pending microtasks (subscription promise chain). */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

// ---------------------------------------------------------------------------
// In-flight sharing (one IPC call per burst)
// ---------------------------------------------------------------------------

describe("in-flight dedup", () => {
  it("simultaneous callers share ONE branches() call and the same list", async () => {
    const parked = parkedBranches();
    mockBranches.mockReturnValue(parked.promise);
    const cache = new RefsCache();

    const first = cache.branchesFor("r1");
    const second = cache.branchesFor("r1");
    const third = cache.branchesFor("r1");

    expect(mockBranches).toHaveBeenCalledTimes(1);
    const list = branchList("main", "dev");
    parked.resolve(list);
    await expect(first).resolves.toBe(list);
    await expect(second).resolves.toBe(list);
    await expect(third).resolves.toBe(list);
    expect(mockBranches).toHaveBeenCalledTimes(1);
  });

  it("branches and remotes use separate slots (one call each per burst)", async () => {
    const cache = new RefsCache();
    await Promise.all([cache.branchesFor("r1"), cache.remotesFor("r1")]);
    expect(mockBranches).toHaveBeenCalledTimes(1);
    expect(mockRemotes).toHaveBeenCalledTimes(1);
  });

  it("repos do not share slots", async () => {
    const cache = new RefsCache();
    await Promise.all([cache.branchesFor("r1"), cache.branchesFor("r2")]);
    expect(mockBranches).toHaveBeenCalledTimes(2);
    expect(mockBranches).toHaveBeenNthCalledWith(1, "r1");
    expect(mockBranches).toHaveBeenNthCalledWith(2, "r2");
  });
});

// ---------------------------------------------------------------------------
// TTL (landed lists are reused briefly, then refetched)
// ---------------------------------------------------------------------------

describe("TTL", () => {
  it("reuses the landed list inside the window and refetches after it", async () => {
    vi.useFakeTimers();
    const cache = new RefsCache();
    const first = branchList("main");
    mockBranches.mockResolvedValueOnce(first);

    await expect(cache.branchesFor("r1")).resolves.toBe(first);
    // Staggered mount inside the window: same scan, same array.
    await expect(cache.branchesFor("r1")).resolves.toBe(first);
    expect(mockBranches).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(REFS_CACHE_TTL_MS + 1);
    const second = branchList("main", "new");
    mockBranches.mockResolvedValueOnce(second);
    await expect(cache.branchesFor("r1")).resolves.toBe(second);
    expect(mockBranches).toHaveBeenCalledTimes(2);
  });

  it("arms the TTL at landing time, not at request time", async () => {
    vi.useFakeTimers();
    const cache = new RefsCache();
    // A slow scan: resolves ~halfway into the TTL window.
    const parked = parkedBranches();
    mockBranches.mockReturnValue(parked.promise);
    const list = branchList("main");
    setTimeout(() => parked.resolve(list), 8_000);

    const promise = cache.branchesFor("r1");
    await vi.advanceTimersByTimeAsync(8_000);
    await expect(promise).resolves.toBe(list);

    // 8 s after landing (16 s after the request) the list is still fresh.
    await vi.advanceTimersByTimeAsync(8_000);
    mockBranches.mockResolvedValueOnce(branchList("stale-should-not-load"));
    await expect(cache.branchesFor("r1")).resolves.toBe(list);
    expect(mockBranches).toHaveBeenCalledTimes(1);
  });
});

// ---------------------------------------------------------------------------
// Invalidation (explicit + repo-changed driven)
// ---------------------------------------------------------------------------

describe("invalidation", () => {
  it("invalidateRefs drops the repo's branches AND remotes, not other repos'", async () => {
    const cache = new RefsCache();
    await cache.branchesFor("r1");
    await cache.remotesFor("r1");
    await cache.branchesFor("r2");

    cache.invalidate("r1");

    await cache.branchesFor("r1");
    await cache.remotesFor("r1");
    await cache.branchesFor("r2");
    expect(mockBranches).toHaveBeenCalledTimes(3); // r1 twice, r2 once
    expect(mockRemotes).toHaveBeenCalledTimes(2);
  });

  it("a significant repo-changed event drops the affected repo's entries", async () => {
    const cache = new RefsCache();
    await cache.branchesFor("r1");
    await cache.remotesFor("r1");
    expect(wireHandlers).toHaveLength(1); // invalidation wiring subscribed

    emitChange({ repo_id: "r1", head_moved: true });

    await cache.branchesFor("r1");
    await cache.remotesFor("r1");
    expect(mockBranches).toHaveBeenCalledTimes(2);
    expect(mockRemotes).toHaveBeenCalledTimes(2);
  });

  it("a watcher-forced full resync drops entries too", async () => {
    const cache = new RefsCache();
    await cache.branchesFor("r1");

    emitChange({ repo_id: "r1", full: true });

    await cache.branchesFor("r1");
    expect(mockBranches).toHaveBeenCalledTimes(2);
  });

  it("a non-significant (paths-only) event keeps the entries", async () => {
    vi.useFakeTimers();
    const cache = new RefsCache();
    const list = branchList("main");
    mockBranches.mockResolvedValueOnce(list);
    await expect(cache.branchesFor("r1")).resolves.toBe(list);

    emitChange({ repo_id: "r1", paths: ["src/a.ts"] });
    emitChange({ repo_id: "r1", paths: [".git/index"] });

    await expect(cache.branchesFor("r1")).resolves.toBe(list);
    expect(mockBranches).toHaveBeenCalledTimes(1);
  });

  it("a significant event for ANOTHER repo leaves these entries alone", async () => {
    const cache = new RefsCache();
    const list = branchList("main");
    mockBranches.mockResolvedValueOnce(list);
    await expect(cache.branchesFor("r1")).resolves.toBe(list);

    emitChange({ repo_id: "other", head_moved: true });

    await expect(cache.branchesFor("r1")).resolves.toBe(list);
    expect(mockBranches).toHaveBeenCalledTimes(1);
  });

  it("an invalidation during an in-flight fetch does not re-cache its result", async () => {
    const parked = parkedBranches();
    mockBranches.mockReturnValue(parked.promise);
    const cache = new RefsCache();
    const inFlight = cache.branchesFor("r1");

    // Refs moved while the scan was running: the landing data predates
    // the invalidation and must not arm a fresh TTL slot.
    cache.invalidate("r1");
    const stale = branchList("stale");
    parked.resolve(stale);
    await expect(inFlight).resolves.toBe(stale); // the caller keeps its data

    const fresh = branchList("fresh");
    mockBranches.mockResolvedValueOnce(fresh);
    await expect(cache.branchesFor("r1")).resolves.toBe(fresh);
    expect(mockBranches).toHaveBeenCalledTimes(2);
  });
});

// ---------------------------------------------------------------------------
// Failures are never cached
// ---------------------------------------------------------------------------

describe("errors", () => {
  it("a rejected scan is dropped: every caller joined it, the next one retries", async () => {
    const parked = parkedBranches();
    mockBranches.mockReturnValue(parked.promise);
    const cache = new RefsCache();
    const first = cache.branchesFor("r1");
    const second = cache.branchesFor("r1"); // joined the in-flight call

    parked.reject(new Error("boom"));
    await expect(first).rejects.toThrow("boom");
    await expect(second).rejects.toThrow("boom");
    expect(mockBranches).toHaveBeenCalledTimes(1);

    const list = branchList("main");
    mockBranches.mockResolvedValueOnce(list);
    await expect(cache.branchesFor("r1")).resolves.toBe(list); // retried
    expect(mockBranches).toHaveBeenCalledTimes(2);
  });
});

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

describe("lifecycle", () => {
  it("subscribes the invalidation listener once (lazily) and stop() clears everything", async () => {
    const cache = new RefsCache();
    expect(wireHandlers).toHaveLength(0); // import alone is side-effect-free

    await cache.branchesFor("r1");
    await cache.remotesFor("r1");
    expect(wireHandlers).toHaveLength(1); // ONE registration, both reads
    // (the backend subscription itself is module-owned by repoEvents; it
    // outlives stop() and is only re-armed by resetRepoEvents in teardown)

    cache.stop();
    emitChange({ repo_id: "r1", head_moved: true }); // listener is gone

    const list = branchList("main");
    mockBranches.mockResolvedValueOnce(list);
    await expect(cache.branchesFor("r1")).resolves.toBe(list); // slots were cleared too
    expect(mockBranches).toHaveBeenCalledTimes(2);

    // ...and the next read re-registered the invalidation listener.
    emitChange({ repo_id: "r1", head_moved: true });
    mockBranches.mockResolvedValueOnce(list);
    await expect(cache.branchesFor("r1")).resolves.toBe(list);
    expect(mockBranches).toHaveBeenCalledTimes(3);
  });
});
