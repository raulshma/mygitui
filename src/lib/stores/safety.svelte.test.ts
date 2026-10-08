/**
 * Unit tests for the safety/undo store (`$lib/stores/safety.svelte`).
 * The IPC client is fully mocked (`vi.mock`) — no Tauri runtime touched.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { checkpoints, guardCheckpoint } from "$lib/ipc/client";
import type { CheckpointInfo } from "$lib/ipc/types";
import {
  guardNow,
  loadUndo,
  safetyStore,
  SafetyStore,
  undoState,
} from "$lib/stores/safety.svelte";

vi.mock("$lib/ipc/client", () => ({
  checkpoints: vi.fn(),
  guardCheckpoint: vi.fn(),
}));

const mockCheckpoints = vi.mocked(checkpoints);
const mockGuardCheckpoint = vi.mocked(guardCheckpoint);

const NOW = 1_789_761_600;

function cp(id: string, createdAt: number): CheckpointInfo {
  return {
    id,
    reason: `reason-${id}`,
    ref_name: `refs/mygitui/checkpoints/${id}`,
    created_at: createdAt,
    branch: "main",
    has_worktree_state: true,
  };
}

beforeEach(() => {
  mockCheckpoints.mockReset();
  mockGuardCheckpoint.mockReset();
});

afterEach(() => {
  safetyStore.drop("repo-1");
});

describe("SafetyStore", () => {
  it("starts with an empty shape for unknown repos", () => {
    const store = new SafetyStore();
    expect(store.stateFor("nope")).toEqual({
      checkpoints: [],
      loading: false,
      error: null,
      lastGuard: null,
    });
  });

  it("load fetches checkpoints and stores them newest-first", async () => {
    const store = new SafetyStore();
    mockCheckpoints.mockResolvedValue([cp("old", 100), cp("new", 200)]);

    await store.load("repo-1");

    expect(mockCheckpoints).toHaveBeenCalledWith("repo-1");
    const state = store.stateFor("repo-1");
    expect(state.checkpoints.map((c) => c.id)).toEqual(["new", "old"]);
    expect(state.loading).toBe(false);
    expect(state.error).toBeNull();
  });

  it("load flips loading while in flight", async () => {
    const store = new SafetyStore();
    let resolveList: (v: CheckpointInfo[]) => void = () => {};
    mockCheckpoints.mockReturnValue(
      new Promise((resolve) => {
        resolveList = resolve;
      }),
    );

    const pending = store.load("repo-1");
    expect(store.stateFor("repo-1").loading).toBe(true);

    resolveList([]);
    await pending;
    expect(store.stateFor("repo-1").loading).toBe(false);
  });

  it("load records failures in error (never throws)", async () => {
    const store = new SafetyStore();
    mockCheckpoints.mockRejectedValue(new Error("backend down"));

    await store.load("repo-1");

    const state = store.stateFor("repo-1");
    expect(state.loading).toBe(false);
    expect(state.error).toBe("backend down");
    expect(state.checkpoints).toEqual([]);

    // A later success clears the error.
    mockCheckpoints.mockResolvedValue([cp("a", 1)]);
    await store.load("repo-1");
    expect(store.stateFor("repo-1").error).toBeNull();
    expect(store.stateFor("repo-1").checkpoints).toHaveLength(1);
  });

  it("guard calls guard_checkpoint, remembers lastGuard and returns it", async () => {
    const store = new SafetyStore();
    const guardCp = cp("guard-1", NOW);
    mockGuardCheckpoint.mockResolvedValue(guardCp);

    const returned = await store.guard("repo-1", "pre-hard-reset");

    expect(mockGuardCheckpoint).toHaveBeenCalledWith("repo-1", "pre-hard-reset");
    expect(returned).toEqual(guardCp);
    expect(store.stateFor("repo-1").lastGuard).toEqual(guardCp);
  });

  it("guard rethrows so callers can abort the guarded op", async () => {
    const store = new SafetyStore();
    mockGuardCheckpoint.mockRejectedValue(new Error("snapshot failed"));

    await expect(store.guard("repo-1", "pre-hard-reset")).rejects.toThrow(
      "snapshot failed",
    );
    expect(store.stateFor("repo-1").lastGuard).toBeNull();
  });

  it("clearGuard and drop reset the per-repo state", async () => {
    const store = new SafetyStore();
    mockGuardCheckpoint.mockResolvedValue(cp("g", NOW));
    await store.guard("repo-1", "x");

    store.clearGuard("repo-1");
    expect(store.stateFor("repo-1").lastGuard).toBeNull();

    store.drop("repo-1");
    expect(store.stateFor("repo-1")).toEqual({
      checkpoints: [],
      loading: false,
      error: null,
      lastGuard: null,
    });
    expect("repo-1" in store.states).toBe(false);
  });

  it("keeps repos isolated", async () => {
    const store = new SafetyStore();
    mockCheckpoints.mockResolvedValue([cp("a", 1)]);
    await store.load("repo-1");
    mockCheckpoints.mockResolvedValue([]);
    await store.load("repo-2");

    expect(store.stateFor("repo-1").checkpoints).toHaveLength(1);
    expect(store.stateFor("repo-2").checkpoints).toEqual([]);
  });
});

describe("standalone delegates", () => {
  it("undoState / loadUndo / guardNow mirror the singleton", async () => {
    mockCheckpoints.mockResolvedValue([cp("z", 9)]);
    await loadUndo("repo-1");
    expect(undoState("repo-1").checkpoints[0]?.id).toBe("z");

    mockGuardCheckpoint.mockResolvedValue(cp("g9", NOW));
    const guardCp = await guardNow("repo-1", "pre-branch-delete");
    expect(guardCp.id).toBe("g9");
    expect(undoState("repo-1").lastGuard?.id).toBe("g9");

    safetyStore.drop("repo-1");
  });
});
