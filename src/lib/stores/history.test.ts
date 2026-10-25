/**
 * Unit tests for `HistoryStore` (`$lib/stores/history.svelte`). The IPC
 * client is fully mocked (`vi.mock`) — no Tauri runtime is touched. The mock
 * `streamLog` records every call and hands back per-call `deliver`/`finish`
 * handles so tests can push pages (including stale ones) on demand.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { onRepoChanged, streamLog } from "$lib/ipc/client";
import type { RepoChangedEvent } from "$lib/ipc/types";
import { resetRepoEvents } from "$lib/events/repoEvents";
import type { CommitInfo, GraphRow, LogFilter, LogPage } from "$lib/ipc/types";
import { EMPTY_FILTER } from "./history-logic";
import { HISTORY_PREFS_PREFIX } from "./historyPrefs";
import { HistoryStore } from "$lib/stores/history.svelte";

vi.mock("$lib/ipc/client", () => ({
  streamLog: vi.fn(),
  onRepoChanged: vi.fn(),
  // $lib/events/repoEvents (imported by history.svelte) pulls the other
  // event seams in transitively; they are never called in this suite.
  onCloneProgress: vi.fn(),
  onOpProgress: vi.fn(),
  onOpLog: vi.fn(),
  onAuthRequest: vi.fn(),
  onActionOutput: vi.fn(),
  onPtyOutput: vi.fn(),
  onPtyExit: vi.fn(),
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
  resetRepoEvents(); // drops the module-owned backend subscription
  localStorage.clear(); // drop seeded per-root history prefs
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
      pickaxe: null,
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
      pickaxe: null,
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

  it("ignores index-only events (stage/unstage) but restarts on full resync", async () => {
    const store = new HistoryStore();
    store.start("repo-1");
    await flush();
    expect(watcherHandlers).toHaveLength(1);

    calls[0]!.deliver(makePage(0, 2, 1));
    expect(store.flat.commits).toHaveLength(2);

    watcherHandlers[0]!({ repo_id: "repo-1", paths: [".git/index"], head_moved: false, full: false });
    expect(calls).toHaveLength(1); // index churn → no restart
    expect(store.pages).toHaveLength(1); // list untouched, no flash

    watcherHandlers[0]!({ repo_id: "repo-1", paths: [], head_moved: false, full: true });
    expect(calls).toHaveLength(2); // watcher resync → restart
    expect(store.pages).toHaveLength(0);
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
  it("cancels everything: deregisters the watcher, invalidates pages/timers", async () => {
    vi.useFakeTimers();
    const store = new HistoryStore();
    store.start("repo-1");
    await flush();
    calls[0]!.deliver(makePage(0, 2, 0));
    expect(store.flat.commits).toHaveLength(2);

    store.setFilter({ text: "soon-destroyed" });
    store.destroy();

    expect(store.pages).toHaveLength(0);
    expect(store.repoId).toBeNull();

    // The store's listener is gone: watcher events no longer restart it
    // (the backend subscription itself is module-owned and stays).
    watcherHandlers[0]!({ repo_id: "repo-1", paths: [], head_moved: true, full: false });
    expect(calls).toHaveLength(1);
    expect(unlistenSpy).not.toHaveBeenCalled();

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
    // The backend subscription is module-owned (one `listen` per event
    // name); restarting only re-registers the store's listener.
    expect(mockOnRepoChanged).toHaveBeenCalledTimes(1);

    // ...and the re-registered store still restarts on significant events.
    calls[1]!.deliver(makePage(0, 2, 1));
    watcherHandlers[0]!({ repo_id: "repo-1", paths: [], head_moved: true, full: false });
    expect(calls).toHaveLength(3);
    expect(store.pages).toHaveLength(0); // fresh stream state
  });
});

// ---------------------------------------------------------------------------
// start() restore ordering (persisted per-root filter → first stream request)
// ---------------------------------------------------------------------------

describe("start() restore ordering", () => {
  /** Seeds persisted prefs for a root, exactly as historyPrefs writes them. */
  function seedPrefs(root: string, filter: Partial<typeof EMPTY_FILTER>): void {
    localStorage.setItem(
      HISTORY_PREFS_PREFIX + root,
      JSON.stringify({ filter: { ...EMPTY_FILTER, ...filter }, selectedSha: null, detailOpen: false }),
    );
  }

  it("applies the persisted filter BEFORE the first stream request", () => {
    seedPrefs("/repo", { text: "fix", author: "Ann", path: "src/" });

    const store = new HistoryStore();
    store.start("repo-1", { root: "/repo" });

    expect(calls).toHaveLength(1);
    // The first repo_log_stream already carries the restored filter —
    // code order inside start(), not view effect order.
    expect(calls[0]!.filter.text).toBe("fix");
    expect(calls[0]!.filter.author).toBe("Ann");
    expect(calls[0]!.filter.path).toBe("src/");
    // The filter bar state matches what was streamed (the view saves it back).
    expect(store.filter.text).toBe("fix");
    expect(store.filterActive).toBe(true);
  });

  it("starts with the empty filter for a root without persisted prefs", () => {
    const store = new HistoryStore();
    store.start("repo-1", { root: "/fresh" });
    expect(calls).toHaveLength(1);
    expect(calls[0]!.filter).toEqual({
      text: null,
      regex: false,
      author: null,
      path: null,
      after_unix: null,
      before_unix: null,
      refs: [],
      follow: false,
      pickaxe: null,
    });
  });

  it("re-applies the OTHER root's persisted filter on repo switch", () => {
    seedPrefs("/a", { text: "aa" });
    seedPrefs("/b", { author: "Bee" });

    const store = new HistoryStore();
    store.start("repo-1", { root: "/a" });
    store.start("repo-2", { root: "/b" });

    expect(calls).toHaveLength(2);
    expect(calls[1]!.filter.text).toBeNull();
    expect(calls[1]!.filter.author).toBe("Bee");
  });

  it("keeps a caller-set filter when started without a root (file-history mode)", () => {
    const store = new HistoryStore({ follow: true });
    store.setFilter({ path: "src/x.ts" });
    store.start("repo-1");
    expect(calls).toHaveLength(1);
    expect(calls[0]!.filter.path).toBe("src/x.ts");
    expect(calls[0]!.filter.follow).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Restored-selection resolution (store wrapper over the pure logic)
// ---------------------------------------------------------------------------

describe("resolveRestoredSelection", () => {
  it("releases buffered pages to resolve a sha beyond the loaded window", () => {
    const store = new HistoryStore();
    store.start("repo-1");
    calls[0]!.deliver(makePage(0, 500, 0));
    calls[0]!.deliver(makePage(500, 500, 0));
    calls[0]!.deliver(makePage(1000, 500, 0)); // buffered (release cap 1000)

    expect(store.resolveRestoredSelection(sha(1_200))).toEqual({
      kind: "resolved",
      index: 1_200,
    });
    expect(store.flat.commits).toHaveLength(1_500); // reveal released the rest
  });

  it("stays pending while the stream may still deliver the commit", () => {
    const store = new HistoryStore();
    store.start("repo-1");
    calls[0]!.deliver(makePage(0, 2, 0));

    // Stream still in flight (loading) and nothing buffered to reveal.
    expect(store.resolveRestoredSelection(sha(99_999))).toEqual({ kind: "pending" });
  });

  it("drops a commit the exhausted stream never delivered", async () => {
    const store = new HistoryStore();
    store.start("repo-1");
    calls[0]!.deliver(makePage(0, 2, 0));
    calls[0]!.finish();
    await flush(); // loading=false, buffer empty → stream exhausted

    expect(store.resolveRestoredSelection(sha(99_999))).toEqual({ kind: "dropped" });
  });
});
