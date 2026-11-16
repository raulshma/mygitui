/**
 * Effect-graph safety tests for `HistoryStore`.
 *
 * `history.test.ts` never calls the store from inside a tracked context —
 * which is exactly how the `effect_update_depth_exceeded` regression
 * slipped through: `#beginStream` began reading AND writing `pages`/`flat`
 * (stale-render retention) while `HistoryView`'s start-effect was tracking
 * them, so the effect re-ran on its own write → cleanup destroyed → start
 * re-began → forever (and the log never loaded). These tests run the store
 * inside a REAL `$effect` so any tracked read of store state in
 * `start()`/`#beginStream` reproduces as a runaway effect here.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync } from "svelte";
import { onRepoChanged, streamLog } from "$lib/ipc/client";
import { resetRepoEvents } from "$lib/events/repoEvents";
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

const streamCalls: string[] = [];
let unlistenSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  streamCalls.length = 0;
  unlistenSpy = vi.fn();
  mockStreamLog.mockReset();
  mockOnRepoChanged.mockReset();
  mockStreamLog.mockImplementation(((repoId: string) => {
    streamCalls.push(repoId);
    return new Promise<void>(() => {}); // walk never ends — stays in flight
  }) as typeof streamLog);
  mockOnRepoChanged.mockImplementation((() => Promise.resolve(unlistenSpy)) as typeof onRepoChanged);
});

afterEach(() => {
  resetRepoEvents(); // drops the module-owned backend subscription
  localStorage.clear();
});

describe("start() inside a tracked $effect", () => {
  it("does not feed back through pages/flat — the effect settles after one run", () => {
    const store = new HistoryStore();
    let runs = 0;
    const dispose = $effect.root(() => {
      // Mirrors HistoryView's start effect exactly, cleanup included: the
      // destroy() in the cleanup is what turns a tracked self-write into an
      // endless destroy→start cycle (each re-run takes start()'s full path).
      $effect(() => {
        runs += 1;
        store.start("repo-1");
        return () => store.destroy();
      });
    });
    try {
      // Pre-fix this flush spins: #beginStream reads #flat (tracked) and
      // writes #flat (invalidates) in the same pass, Svelte re-runs the
      // effect until it throws effect_update_depth_exceeded. Post-fix the
      // retention snapshot is untracked, so one run, one stream request.
      flushSync();
      expect(runs).toBe(1);
      expect(streamCalls).toEqual(["repo-1"]);
      expect(store.loading).toBe(true);
    } finally {
      dispose();
    }
  });

  it("survives a real re-run (repo switch) without spinning", () => {
    const store = new HistoryStore();
    let repo = $state("repo-1");
    let runs = 0;
    const dispose = $effect.root(() => {
      $effect(() => {
        runs += 1;
        store.start(repo);
        return () => store.destroy();
      });
    });
    try {
      flushSync();
      expect(runs).toBe(1);

      // The one legitimate reason the view's effect re-runs: a repo
      // re-target. Cleanup destroys, the full start path runs again —
      // resetState + #beginStream write pages/flat on every pass, and none
      // of that may re-trigger the effect.
      repo = "repo-2";
      flushSync();
      expect(runs).toBe(2);
      expect(streamCalls).toEqual(["repo-1", "repo-2"]);
      expect(store.repoId).toBe("repo-2");
      expect(store.loading).toBe(true);
    } finally {
      dispose();
    }
  });
});
