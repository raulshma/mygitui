/**
 * Unit tests for the actions store (`$lib/stores/actions.svelte`). The IPC
 * client is fully mocked (`vi.mock`) — no Tauri runtime is touched — and a
 * map-backed storage keeps persistence hermetic.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actionCancel, actionRun, onActionOutput } from "$lib/ipc/client";
import type { ActionOutputEvent } from "$lib/ipc/types";
import { getToasts } from "$lib/toast";
import {
  ActionsStore,
  actionsStore,
  ACTIONS_STORAGE_KEY,
  type ActionDef,
  type StorageLike,
} from "$lib/stores/actions.svelte";
import { MAX_RUNS, RUN_LINE_CAP } from "$lib/components/actions/actionsModel";

vi.mock("$lib/ipc/client", () => ({
  actionRun: vi.fn(),
  actionCancel: vi.fn(),
  onActionOutput: vi.fn(),
}));

const mockActionRun = vi.mocked(actionRun);
const mockActionCancel = vi.mocked(actionCancel);
const mockOnActionOutput = vi.mocked(onActionOutput);

/** Map-backed storage (never touches localStorage). */
function fakeStorage(): StorageLike & { dump: () => string } {
  const map = new Map<string, string>();
  return {
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => void map.set(key, value),
    dump: () => map.get(ACTIONS_STORAGE_KEY) ?? "",
  };
}

function outputEvent(
  patch: Partial<ActionOutputEvent> & { run_id: string },
): ActionOutputEvent {
  return {
    repo_id: "repo-1",
    name: "Test action",
    line: "",
    done: false,
    exit_code: null,
    ...patch,
  };
}

const DEF: ActionDef = {
  id: "def-1",
  name: "Test action",
  command: "echo hi",
  scope: "global",
};

const outputHandlers: Array<(event: ActionOutputEvent) => void> = [];
const unlistenSpy = vi.fn();

beforeEach(() => {
  outputHandlers.length = 0;
  unlistenSpy.mockReset();
  mockActionRun.mockReset();
  mockActionCancel.mockReset();
  mockOnActionOutput.mockReset();
  mockActionRun.mockResolvedValue("run-x");
  mockActionCancel.mockResolvedValue(true);
  mockOnActionOutput.mockImplementation(
    ((cb: (event: ActionOutputEvent) => void) => {
      outputHandlers.push(cb);
      return Promise.resolve(unlistenSpy);
    }) as typeof onActionOutput,
  );
});

afterEach(() => {
  actionsStore.stop();
});

/** Flushes pending microtasks (subscription chain). */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

describe("ActionsStore config", () => {
  it("starts empty without storage and persists added defs", () => {
    const storage = fakeStorage();
    const store = new ActionsStore(storage);
    expect(store.defs).toEqual([]);

    const def = store.addDef({
      name: "  List files  ",
      command: " ls -la ",
      scope: "global",
    });
    expect(def).not.toBeNull();
    expect(def?.name).toBe("List files");
    expect(def?.command).toBe("ls -la");
    expect(store.defs).toHaveLength(1);

    // The storage got a valid snapshot.
    const reloaded = new ActionsStore(storage);
    expect(reloaded.defs.map((d) => d.name)).toEqual(["List files"]);
  });

  it("rejects empty name or command without persisting", () => {
    const storage = fakeStorage();
    const store = new ActionsStore(storage);
    expect(store.addDef({ name: "  ", command: "ls", scope: "global" })).toBeNull();
    expect(store.addDef({ name: "x", command: "", scope: "global" })).toBeNull();
    expect(store.defs).toEqual([]);
    expect(storage.dump()).toBe("");
  });

  it("updates and removes defs by id", () => {
    const store = new ActionsStore(fakeStorage());
    const def = store.addDef({ name: "A", command: "a", scope: "global" })!;
    store.updateDef(def.id, { command: "b", scope: "repo", repoId: "repo-1" });
    expect(store.defs[0]).toMatchObject({ command: "b", scope: "repo", repoId: "repo-1" });

    store.removeDef(def.id);
    expect(store.defs).toEqual([]);
  });

  it("defsFor merges global defs with the repo's own scoped defs", () => {
    const store = new ActionsStore(fakeStorage());
    store.addDef({ name: "g", command: "g", scope: "global" });
    store.addDef({ name: "own", command: "o", scope: "repo", repoId: "repo-1" });
    store.addDef({ name: "other", command: "x", scope: "repo", repoId: "repo-2" });

    expect(store.defsFor("repo-1").map((d) => d.name)).toEqual(["g", "own"]);
    expect(store.defsFor("repo-2").map((d) => d.name)).toEqual(["g", "other"]);
  });

  it("drops malformed persisted entries", () => {
    const storage = fakeStorage();
    storage.setItem(ACTIONS_STORAGE_KEY, JSON.stringify({ defs: [
      { id: "ok", name: "n", command: "c", scope: "global" },
      { id: 7, name: "bad" },
      { id: "no-scope", name: "n", command: "c", scope: "weird" },
      "junk",
    ] }));
    storage.setItem("other-key", "not json at all");
    const store = new ActionsStore(storage);
    expect(store.defs.map((d) => d.id)).toEqual(["ok"]);
  });
});

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

describe("ActionsStore runs", () => {
  it("startActionsEvents subscribes once and routes events", async () => {
    const store = new ActionsStore(fakeStorage());
    store.startActionsEvents();
    store.startActionsEvents(); // idempotent
    await flush();

    expect(mockOnActionOutput).toHaveBeenCalledTimes(1);
    outputHandlers[0]!(outputEvent({ run_id: "r1", line: "hello" }));

    expect(store.runsFor("repo-1")).toHaveLength(1);
    expect(store.runs[0]).toMatchObject({ run_id: "r1", lines: ["hello"], done: false, status: "running" });
    store.stop();
  });

  it("appends lines, caps them, accumulates the dropped count and finishes", () => {
    // Small cap keeps this O(n²) worst case instant.
    const store = new ActionsStore(fakeStorage(), 50);
    for (let i = 0; i < 53; i += 1) {
      store.applyOutput(outputEvent({ run_id: "r1", line: `l${i}` }));
    }
    store.applyOutput(outputEvent({ run_id: "r1", done: true, exit_code: 0 }));

    const run = store.runs[0];
    expect(run.lines).toHaveLength(50);
    // Most recent lines kept.
    expect(run.lines[0]).toBe("l3");
    expect(run.dropped).toBe(3);
    expect(run.done).toBe(true);
    expect(run.exit_code).toBe(0);
    expect(run.status).toBe("success");
    expect(store.busy("repo-1")).toBe(false);
  });

  it("classifies failure and running states", () => {
    const store = new ActionsStore(fakeStorage());
    store.applyOutput(outputEvent({ run_id: "r1", line: "boom" }));
    store.applyOutput(outputEvent({ run_id: "r1", done: true, exit_code: 2 }));
    expect(store.runs[0].status).toBe("failed");

    store.applyOutput(outputEvent({ run_id: "r2", done: true, exit_code: null }));
    // Killed without a code = failed.
    expect(store.runs[0].status).toBe("failed");
  });

  it("creates runs for events of unknown runs and scopes them to their repo", () => {
    const store = new ActionsStore(fakeStorage());
    store.applyOutput(outputEvent({ run_id: "r1", repo_id: "repo-9", name: "other" }));
    expect(store.runsFor("repo-1")).toEqual([]);
    expect(store.runsFor("repo-9").map((r) => r.run_id)).toEqual(["r1"]);
    expect(store.busy("repo-9", "other")).toBe(true);
    expect(store.busy("repo-9", "nope")).toBe(false);
  });

  it("keeps at most MAX_RUNS runs (oldest dropped)", () => {
    const store = new ActionsStore(fakeStorage());
    for (let i = 0; i < MAX_RUNS + 5; i += 1) {
      store.applyOutput(outputEvent({ run_id: `r${i}` }));
    }
    expect(store.runs).toHaveLength(MAX_RUNS);
    // Newest first; the five oldest were evicted.
    expect(store.runs[0].run_id).toBe(`r${MAX_RUNS + 4}`);
    expect(store.runs.at(-1)?.run_id).toBe("r5");
  });

  it("run() launches via the client and pre-registers a running placeholder", async () => {
    const store = new ActionsStore(fakeStorage());
    const runId = await store.run("repo-1", DEF);
    expect(runId).toBe("run-x");
    expect(mockActionRun).toHaveBeenCalledWith("repo-1", "Test action", "echo hi");
    expect(store.runs[0]).toMatchObject({
      run_id: "run-x",
      repo_id: "repo-1",
      status: "running",
    });

    // Events for the same run merge into the placeholder (no duplicate).
    store.applyOutput(outputEvent({ run_id: "run-x", line: "hi", done: true, exit_code: 0 }));
    expect(store.runs.filter((r) => r.run_id === "run-x")).toHaveLength(1);
    expect(store.runs[0].status).toBe("success");
  });

  it("run() toasts instead of throwing on launch failure", async () => {
    mockActionRun.mockRejectedValueOnce(new Error("3 actions running") as never);
    const store = new ActionsStore(fakeStorage());
    await expect(store.run("repo-1", DEF)).resolves.toBeNull();
    expect(
      getToasts().some((t) => t.kind === "error" && t.message.includes("3 actions running")),
    ).toBe(true);
    expect(store.runs).toEqual([]);
  });

  it("run() rejects empty commands up front", async () => {
    const store = new ActionsStore(fakeStorage());
    await expect(store.run("repo-1", { ...DEF, command: "   " })).resolves.toBeNull();
    expect(mockActionRun).not.toHaveBeenCalled();
  });

  it("cancel() delegates and toasts no-ops", async () => {
    const store = new ActionsStore(fakeStorage());
    await expect(store.cancel("r1")).resolves.toBe(true);
    expect(mockActionCancel).toHaveBeenCalledWith("r1");

    mockActionCancel.mockResolvedValueOnce(false);
    await expect(store.cancel("r1")).resolves.toBe(false);
    expect(getToasts().some((t) => t.message.includes("already finished"))).toBe(true);
  });

  it("stop unsubscribes and clears runs; the store can restart", async () => {
    const store = new ActionsStore(fakeStorage());
    store.startActionsEvents();
    await flush();
    store.applyOutput(outputEvent({ run_id: "r1" }));

    store.stop();
    expect(unlistenSpy).toHaveBeenCalledTimes(1);
    expect(store.runs).toEqual([]);

    store.startActionsEvents();
    await flush();
    expect(mockOnActionOutput).toHaveBeenCalledTimes(2);
    store.stop();
  });

  it("the shared singleton participates in the same event stream", async () => {
    actionsStore.startActionsEvents();
    await flush();
    outputHandlers[0]!(outputEvent({ run_id: "s1" }));
    expect(actionsStore.runs.map((r) => r.run_id)).toContain("s1");
  });
});
