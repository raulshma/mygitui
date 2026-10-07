/**
 * Unit tests for `HistoryStore` (`$lib/stores/history.svelte`). The IPC
 * client is fully mocked (`vi.mock`) — no Tauri runtime is touched. The mock
 * `streamLog` records every call and hands back per-call `deliver`/`finish`
 * handles so tests can push pages (including stale ones) on demand.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { onRepoChanged, streamLog } from "$lib/ipc/client";
import type { RepoChangedEvent } from "$lib/ipc/client";
import type { CommitInfo, GraphRow, LogFilter, LogPage } from "$lib/ipc/types";
import { HistoryStore } from "$lib/stores/history.svelte";

vi.mock("$lib/ipc/client", () => ({
  streamLog: vi.fn(),
  onRepoChanged: vi.fn(),
}));

const mockStreamLog = vi.mocked(streamLog);
const mockOnRepoChanged = vi.mocked(onRepoChanged);

// ---------------------------------------------------------------------------
// Fixtures + stream harness
// ---------------------------------------------------------------------------

function sha(i: number): string {
  return (i + 1).toString(16).padStart(40, "0");
}

function commit(i: number): CommitInfo {
  return {
    sha: sha(i),
    parents: i > 0 ? [sha(i - 1)] : [],
    author: { name: `A ${i}`, email: `a${i}@x.com`, time: 1_700_000 + i, offset_minutes: 0 },
    committer: { name: `A ${i}`, email: `a${i}@x.com`, time: 1_700_000 + i, offset_minutes: 0 },
    message: `commit ${i}\n`,
    summary: `commit ${i}`,
    refs: [],
  };
}

function makePage(
  from: number,
  count: number,
  generation = 0,
  overlap = 0,
): LogPage {
  const local = Array.from({ length: count }, (_, k) => commit(from + k));
  const commits = overlap > 0 ? [commit(from - 1), ...local] : local;
  const rows: GraphRow[] = commits.map((c) => ({ sha: c.sha, lane: 0, edges: [], lane_count: 1 }));
  return { commits, rows, next_cursor: null, generation };
}

interface StreamCall {
  repoId: string;
  filter: LogFilter;
  deliver: (page: LogPage) => void;
  finish: () => void;
}

const calls: StreamCall[] = [];
const watcherHandlers: Array<(event: RepoChangedEvent) => void> = [];
let unlistenSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  calls.length = 0;
  watcherHandlers.length = 0;
  unlistenSpy = vi.fn();
  mockStreamLog.mockReset();
  mockOnRepoChanged.mockReset();
  mockStreamLog.mockImplementation(((repoId: string, filter: LogFilter, onPage: (p: LogPage) => void) => {
    let finish!: () => void;
    const done = new Promise<void>((resolve) => {
      finish = resolve;
    });
    calls.push({ repoId, filter, deliver: onPage, finish });
    return done;
  }) as typeof streamLog);
  mockOnRepoChanged.mockImplementation(((cb: (event: RepoChangedEvent) => void) => {
    watcherHandlers.push(cb);
    return Promise.resolve(unlistenSpy);
  }) as typeof onRepoChanged);
});

afterEach(() => {
  vi.useRealTimers();
});

/** Flushes pending microtasks (stream completion callbacks). */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

// ---------------------------------------------------------------------------
// start / page append / dedupe
// ---------------------------------------------------------------------------

describe("start + page append", () => {
  it("starts a stream with the default filter and appends pages", async () => {
    const store = new HistoryStore();
    store.start("repo-1");

    expect(calls).toHaveLength(1);
    expect(calls[0]!.repoId).toBe("repo-1");
    expect(calls[0]!.filter).toEqual({
      text: null,
      regex: false,
      author: null,
      path: null,
      after_unix: null,
      before_unix: null,
      refs: [],
      follow: false,
    });
    expect(store.loading).toBe(true);

    calls[0]!.deliver(makePage(0, 3, 0));
    expect(store.pages).toHaveLength(1);
    expect(store.flat.commits).toHaveLength(3);
    expect(store.flat.rows).toHaveLength(3);
    expect(store.count).toBe(3);

    calls[0]!.finish();
    await flush();
    expect(store.loading).toBe(false);
  });

  it("dedupes boundary-overlap shas across pages", () => {
    const store = new HistoryStore();
    store.start("repo-1");

    calls[0]!.deliver(makePage(0, 3, 0));
    calls[0]!.deliver(makePage(3, 2, 0, 1)); // first sha repeats sha(2)

    expect(store.flat.commits).toHaveLength(5);
    expect(store.flat.commits.filter((c) => c.sha === sha(2))).toHaveLength(1);
    expect(store.flat.rows).toHaveLength(5);
    expect(store.indexOfSha(sha(4))).toBe(4);
  });

  it("is idempotent: starting the same repo twice does not re-stream", () => {
    const store = new HistoryStore();
    store.start("repo-1");
    store.start("repo-1");
    expect(calls).toHaveLength(1);
  });

  it("switching repos resets state and restarts (generation counter too)", () => {
    const store = new HistoryStore();
    store.start("repo-1");
    calls[0]!.deliver(makePage(0, 2, 3)); // generation 3 accepted
    expect(store.generation).toBe(3);

    store.start("repo-2");
    expect(calls).toHaveLength(2);
    expect(store.pages).toHaveLength(0);
    expect(store.flat.commits).toHaveLength(0);
    expect(store.generation).toBe(0);

    // Cross-repo: a "lower" generation is fresh for the new repo, not stale.
    calls[1]!.deliver(makePage(0, 1, 2));
    expect(store.flat.commits).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// Release window / loadMore
// ---------------------------------------------------------------------------

describe("release window + loadMore", () => {
  it("releases pages in 1000-commit chunks and loadMore() releases the rest", () => {
    const store = new HistoryStore();
    store.start("repo-1");

    calls[0]!.deliver(makePage(0, 500, 0));
    calls[0]!.deliver(makePage(500, 500, 0));
    calls[0]!.deliver(makePage(1000, 500, 0));

    expect(store.pages).toHaveLength(2);
    expect(store.flat.commits).toHaveLength(1_000);
    expect(store.hasMore).toBe(true);
    expect(store.pendingCount).toBe(1);

    store.loadMore();
    expect(store.pages).toHaveLength(3);
    expect(store.flat.commits).toHaveLength(1_500);
    expect(store.hasMore).toBe(false);

    store.loadMore(); // no-op when nothing is buffered
    expect(store.pages).toHaveLength(3);
  });

  it("revealSha releases buffered pages until the sha is present", () => {
    const store = new HistoryStore();
    store.start("repo-1");
    calls[0]!.deliver(makePage(0, 500, 0));
    calls[0]!.deliver(makePage(500, 500, 0));
    calls[0]!.deliver(makePage(1000, 500, 0));

    expect(store.revealSha(sha(1_200))).toBe(true);
    expect(store.flat.commits).toHaveLength(1_500);
    expect(store.indexOfSha(sha(1_200))).toBe(1_200);

    expect(store.revealSha(sha(99_999))).toBe(false); // not buffered, terminates
  });
});

// ---------------------------------------------------------------------------
// Filter edits (debounced restart)
// ---------------------------------------------------------------------------

describe("filter debounce", () => {
  it("restarts the stream 200ms after a filter edit with the serialized filter", async () => {
    vi.useFakeTimers();
    const store = new HistoryStore();
    store.start("repo-1");
    calls[0]!.deliver(makePage(0, 2, 0));
    expect(store.flat.commits).toHaveLength(2);

    store.setFilter({ text: "fix" });
    store.setFilter({ author: "alice", after: "2026-01-02" });
    expect(calls).toHaveLength(1); // debounced — nothing yet
    expect(store.flat.commits).toHaveLength(2); // old results stay visible

    await vi.advanceTimersByTimeAsync(200);

    expect(calls).toHaveLength(2);
    expect(calls[1]!.filter).toEqual({
      text: "fix",
      regex: false,
      author: "alice",
      path: null,
      after_unix: 1_767_312_000,
      before_unix: null,
      refs: [],
      follow: false,
    });
    // Restart resets the released pages until fresh ones arrive.
    expect(store.pages).toHaveLength(0);
    expect(store.loading).toBe(true);

    calls[1]!.deliver(makePage(10, 1, 0));
    expect(store.flat.commits.map((c) => c.summary)).toEqual(["commit 10"]);
  });

  it("clearFilter empties every field and restarts", async () => {
    vi.useFakeTimers();
    const store = new HistoryStore();
    store.start("repo-1");
    store.setFilter({ text: "x", path: "src/" });
    await vi.advanceTimersByTimeAsync(200);
    expect(calls).toHaveLength(2);

    store.clearFilter();
    await vi.advanceTimersByTimeAsync(200);
    expect(calls).toHaveLength(3);
    expect(calls[2]!.filter.text).toBeNull();
    expect(calls[2]!.filter.path).toBeNull();
  });

  it("exposes filterActive for the clear button", () => {
    const store = new HistoryStore();
    expect(store.filterActive).toBe(false);
    store.setFilter({ text: "a" });
    expect(store.filterActive).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Staleness (frontend token + backend generation)
// ---------------------------------------------------------------------------

describe("staleness", () => {
  it("ignores pages delivered by a superseded stream (stale token)", async () => {
    vi.useFakeTimers();
    const store = new HistoryStore();
    store.start("repo-1");
    calls[0]!.deliver(makePage(0, 2, 0));

    store.setFilter({ text: "z" });
    await vi.advanceTimersByTimeAsync(200);
    expect(calls).toHaveLength(2);

    calls[0]!.deliver(makePage(50, 1, 0)); // old stream, stale token
    expect(store.flat.commits).toHaveLength(0);

    calls[0]!.finish(); // old stream completing must not clear loading
    await flush();
    expect(store.loading).toBe(true);

    calls[1]!.deliver(makePage(60, 1, 0));
    expect(store.flat.commits).toHaveLength(1);
  });

  it("ignores pages with a stale backend generation", () => {
    const store = new HistoryStore();
    store.start("repo-1");

    calls[0]!.deliver(makePage(0, 2, 5));
    expect(store.flat.commits).toHaveLength(2);
    expect(store.generation).toBe(5);

    calls[0]!.deliver(makePage(10, 1, 4)); // older backend stream
    expect(store.flat.commits).toHaveLength(2);

    calls[0]!.deliver(makePage(20, 1, 5)); // same generation, fine
    expect(store.flat.commits).toHaveLength(3);

    calls[0]!.deliver(makePage(30, 1, 6)); // bumped generation, fine
    expect(store.flat.commits).toHaveLength(4);
    expect(store.generation).toBe(6);
  });
});

// ---------------------------------------------------------------------------
// repo-changed restart
// ---------------------------------------------------------------------------

describe("repo-changed watcher", () => {
  it("restarts the stream for the bound repo and ignores other repos", async () => {
    const store = new HistoryStore();
    store.start("repo-1");
    await flush();
    expect(mockOnRepoChanged).toHaveBeenCalledTimes(1);
    expect(watcherHandlers).toHaveLength(1);

    calls[0]!.deliver(makePage(0, 2, 1));
    expect(store.flat.commits).toHaveLength(2);

    watcherHandlers[0]!({ repo_id: "repo-1", paths: [], head_moved: true, full: false });
    expect(calls).toHaveLength(2);
    expect(store.pages).toHaveLength(0); // fresh stream state

    watcherHandlers[0]!({ repo_id: "other", paths: [], head_moved: true, full: false });
    expect(calls).toHaveLength(2); // not our repo → no restart
  });
});

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

describe("errors", () => {
  it("records stream failures and clears loading", async () => {
    mockStreamLog.mockReset();
    mockStreamLog.mockRejectedValue(new Error("boom") as never);
    const store = new HistoryStore();
    store.start("repo-1");
    await flush();
    expect(store.error).toBe("boom");
    expect(store.loading).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// destroy (cancel-on-close)
// ---------------------------------------------------------------------------

describe("destroy", () => {
  it("cancels everything: unsubscribes, invalidates pages/timers", async () => {
    vi.useFakeTimers();
    const store = new HistoryStore();
    store.start("repo-1");
    await flush();
    calls[0]!.deliver(makePage(0, 2, 0));
    expect(store.flat.commits).toHaveLength(2);

    store.setFilter({ text: "soon-destroyed" });
    store.destroy();

    expect(unlistenSpy).toHaveBeenCalledTimes(1);
    expect(store.pages).toHaveLength(0);
    expect(store.repoId).toBeNull();

    calls[0]!.deliver(makePage(90, 1, 0)); // stale stream → ignored
    calls[0]!.finish();
    await flush();
    expect(store.flat.commits).toHaveLength(0);
    expect(store.loading).toBe(false);

    await vi.advanceTimersByTimeAsync(500); // debounced restart was cancelled
    expect(calls).toHaveLength(1);
  });

  it("can be started again after destroy", async () => {
    const store = new HistoryStore();
    store.start("repo-1");
    store.destroy();
    store.start("repo-1");
    await flush();
    expect(calls).toHaveLength(2);
    expect(mockOnRepoChanged).toHaveBeenCalledTimes(2);
  });
});
