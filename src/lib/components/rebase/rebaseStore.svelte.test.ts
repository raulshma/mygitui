/**
 * Unit tests for the rebase monitor (`./rebaseStore.svelte`). The IPC client
 * is fully mocked (`vi.mock`) and polling uses fake timers — no Tauri
 * runtime, no real 500ms waits. Covers: polling lifecycle (immediate poll,
 * 500ms cadence, idempotent start, repo switch, stop cleanup), the
 * poll-until-done transition, pause + conflict detection (toast-once),
 * continue/abort delegation and error surfacing.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { conflicts, rebaseAbort, rebaseContinue, rebaseState } from "$lib/ipc/client";
import type { ConflictFile, RebaseState, RebaseStep } from "$lib/ipc/types";
import { dismissToast, getToasts } from "$lib/toast";
import { REBASE_POLL_MS, RebaseMonitor } from "./rebaseStore.svelte";

vi.mock("$lib/ipc/client", () => ({
  rebaseState: vi.fn(),
  rebaseContinue: vi.fn(),
  rebaseAbort: vi.fn(),
  conflicts: vi.fn(),
}));

const mockRebaseState = vi.mocked(rebaseState);
const mockRebaseContinue = vi.mocked(rebaseContinue);
const mockRebaseAbort = vi.mocked(rebaseAbort);
const mockConflicts = vi.mocked(conflicts);

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const SHA_A = "a".repeat(40);
const SHA_B = "b".repeat(40);

function step(sha: string): RebaseStep {
  return { sha, action: "pick", new_message: null };
}

function state(patch: Partial<RebaseState> = {}): RebaseState {
  return {
    active: false,
    plan: [],
    current: 0,
    paused_for_edit: false,
    paused_for_exec: false,
    exec_error: null,
    rewritten: [],
    ...patch,
  };
}

const RUNNING = state({
  active: true,
  plan: [step(SHA_A), step(SHA_B)],
  current: 0,
  rewritten: [],
});

const CONFLICT_FILES: ConflictFile[] = [
  { path: "src/a.ts", has_base: true, has_ours: true, has_theirs: true, source: "rebase" },
  { path: "src/b.ts", has_base: true, has_ours: true, has_theirs: false, source: "rebase" },
];

async function flush(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

beforeEach(() => {
  vi.useFakeTimers();
  // Fake timers never fire toast auto-dismiss — drain the shared toast store.
  for (const item of getToasts()) dismissToast(item.id);
  mockRebaseState.mockReset().mockResolvedValue(state());
  mockRebaseContinue.mockReset().mockResolvedValue(state());
  mockRebaseAbort.mockReset().mockResolvedValue(undefined);
  mockConflicts.mockReset().mockResolvedValue([]);
});

afterEach(() => {
  vi.useRealTimers();
});

// ---------------------------------------------------------------------------
// Polling lifecycle
// ---------------------------------------------------------------------------

describe("RebaseMonitor polling lifecycle", () => {
  it("polls immediately on start and then every 500ms", async () => {
    mockRebaseState.mockResolvedValue(RUNNING);
    const monitor = new RebaseMonitor();

    monitor.start("repo-1");
    await flush();

    expect(mockRebaseState).toHaveBeenCalledTimes(1);
    expect(mockRebaseState).toHaveBeenCalledWith("repo-1");
    expect(monitor.polling).toBe(true);

    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS);
    expect(mockRebaseState).toHaveBeenCalledTimes(2);

    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS * 2);
    expect(mockRebaseState).toHaveBeenCalledTimes(4);

    monitor.stop();
  });

  it("applies the polled state (active, plan, current, rewritten)", async () => {
    mockRebaseState.mockResolvedValue(
      state({ active: true, plan: [step(SHA_A)], current: 0, rewritten: [[SHA_A, SHA_A]] }),
    );
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();

    expect(monitor.active).toBe(true);
    expect(monitor.state?.plan).toHaveLength(1);
    expect(monitor.stepCount).toBe(1);
    expect(monitor.rewrittenCount).toBe(1);
    expect(monitor.currentStep?.sha).toBe(SHA_A);

    monitor.stop();
  });

  it("start is idempotent for the same repo (timer not duplicated)", async () => {
    mockRebaseState.mockResolvedValue(RUNNING);
    const monitor = new RebaseMonitor();

    monitor.start("repo-1");
    await flush();
    monitor.start("repo-1");
    monitor.start("repo-1");
    await flush();

    expect(mockRebaseState).toHaveBeenCalledTimes(1); // no extra immediate poll

    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS);
    expect(mockRebaseState).toHaveBeenCalledTimes(2);

    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS * 2);
    expect(mockRebaseState).toHaveBeenCalledTimes(4); // exactly one timer

    monitor.stop();
  });

  it("stops polling on its own once the rebase is no longer active (poll-until-done)", async () => {
    mockRebaseState.mockResolvedValue(RUNNING);
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();
    expect(monitor.polling).toBe(true);

    // The rebase finishes underneath us.
    mockRebaseState.mockResolvedValue(state({ active: false, rewritten: [[SHA_A, SHA_A]] }));
    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS);

    expect(mockRebaseState).toHaveBeenCalledTimes(2);
    expect(monitor.polling).toBe(false);
    expect(monitor.active).toBe(false);
    // Final state stays visible for the completion summary.
    expect(monitor.state?.active).toBe(false);
    expect(monitor.rewrittenCount).toBe(1);

    // No further polls after the natural stop.
    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS * 3);
    expect(mockRebaseState).toHaveBeenCalledTimes(2);
  });

  it("toasts success on the natural finish, but not when nothing was running", async () => {
    const monitor = new RebaseMonitor();
    monitor.start("repo-1"); // backend idle → first poll disengages quietly
    await flush();
    expect(monitor.polling).toBe(false);
    expect(getToasts().some((t) => t.message.includes("Rebase complete"))).toBe(false);

    mockRebaseState.mockResolvedValue(RUNNING);
    monitor.start("repo-1");
    await flush();
    mockRebaseState.mockResolvedValue(state());
    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS);

    expect(getToasts().some((t) => t.kind === "success" && t.message.includes("Rebase complete"))).toBe(true);
  });

  it("stop() clears the timer and all state", async () => {
    mockRebaseState.mockResolvedValue(RUNNING);
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();
    expect(monitor.active).toBe(true);

    monitor.stop();

    expect(monitor.polling).toBe(false);
    expect(monitor.active).toBe(false);
    expect(monitor.state).toBeNull();
    expect(monitor.conflictCount).toBe(0);
    expect(monitor.repoId).toBeNull();

    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS * 2);
    expect(mockRebaseState).toHaveBeenCalledTimes(1); // timer is gone

    // Restartable after stop.
    monitor.start("repo-1");
    await flush();
    expect(mockRebaseState).toHaveBeenCalledTimes(2);
    monitor.stop();
  });

  it("switching repos resets state and retargets the polls", async () => {
    mockRebaseState.mockResolvedValue(RUNNING);
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();
    expect(monitor.active).toBe(true);

    monitor.start("repo-2");
    await flush();

    expect(mockRebaseState).toHaveBeenLastCalledWith("repo-2");
    expect(monitor.repoId).toBe("repo-2");
    // The old repo's active flag did not leak into the new run.
    expect(monitor.active).toBe(true); // RUNNING mock applies to the new repo too

    monitor.stop();
    expect(monitor.repoId).toBeNull();
  });

  it("keeps polling through transient poll errors and records them", async () => {
    mockRebaseState.mockRejectedValueOnce(new Error("backend hiccup"));
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();

    expect(monitor.error).toBe("backend hiccup");
    expect(monitor.polling).toBe(true);

    mockRebaseState.mockResolvedValue(RUNNING);
    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS);

    expect(monitor.error).toBeNull();
    expect(monitor.active).toBe(true);
    monitor.stop();
  });

  it("skips overlapping ticks while a poll is in flight", async () => {
    let release: (value: RebaseState) => void = () => {};
    mockRebaseState.mockReturnValue(
      new Promise<RebaseState>((resolve) => {
        release = resolve;
      }),
    );
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS * 3); // three ticks would fire

    expect(mockRebaseState).toHaveBeenCalledTimes(1); // overlap-guarded

    release(RUNNING);
    await flush();
    monitor.stop();
  });
});

// ---------------------------------------------------------------------------
// Pause + conflict detection
// ---------------------------------------------------------------------------

describe("RebaseMonitor pause detection", () => {
  it("polls conflicts while paused_for_edit and exposes the count", async () => {
    mockRebaseState.mockResolvedValue(
      state({ active: true, paused_for_edit: true, plan: [step(SHA_A)], current: 0 }),
    );
    mockConflicts.mockResolvedValue(CONFLICT_FILES);
    const monitor = new RebaseMonitor();

    monitor.start("repo-1");
    await flush();

    expect(mockConflicts).toHaveBeenCalledWith("repo-1");
    expect(monitor.paused).toBe(true);
    expect(monitor.conflictCount).toBe(2);
    expect(monitor.inConflict).toBe(true);

    monitor.stop();
  });

  it("toasts once when conflicts appear, not on every poll", async () => {
    mockRebaseState.mockResolvedValue(state({ active: true, paused_for_edit: true }));
    mockConflicts.mockResolvedValue(CONFLICT_FILES);
    const monitor = new RebaseMonitor();

    monitor.start("repo-1");
    await flush();
    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS * 2);
    await flush();

    const conflictToasts = getToasts().filter(
      (t) => t.kind === "error" && t.message.includes("Rebase conflict"),
    );
    expect(conflictToasts).toHaveLength(1);
    expect(conflictToasts[0]?.message).toContain("2 files");

    monitor.stop();
  });

  it("clears conflicts (and re-arms the toast) when the rebase unpauses", async () => {
    mockRebaseState.mockResolvedValue(state({ active: true, paused_for_edit: true }));
    mockConflicts.mockResolvedValue(CONFLICT_FILES);
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();
    expect(monitor.inConflict).toBe(true);

    mockRebaseState.mockResolvedValue(RUNNING); // active, not paused
    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS);

    expect(monitor.conflictCount).toBe(0);
    expect(monitor.inConflict).toBe(false);
    expect(monitor.paused).toBe(false);

    monitor.stop();
  });

  it("does not poll conflicts while the rebase runs unpaused", async () => {
    mockRebaseState.mockResolvedValue(RUNNING);
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();
    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS);

    expect(mockConflicts).not.toHaveBeenCalled();
    monitor.stop();
  });
});

// ---------------------------------------------------------------------------
// Continue / abort delegation
// ---------------------------------------------------------------------------

describe("RebaseMonitor continue/abort", () => {
  it("continueRebase delegates to rebase_continue and applies the result", async () => {
    mockRebaseState.mockResolvedValue(state({ active: true, paused_for_edit: true }));
    mockConflicts.mockResolvedValue(CONFLICT_FILES);
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();
    expect(monitor.inConflict).toBe(true);

    const next = state({ active: true, plan: [step(SHA_A)], current: 1, rewritten: [[SHA_A, SHA_A]] });
    mockRebaseContinue.mockResolvedValue(next);
    mockConflicts.mockResolvedValue([]);

    const result = await monitor.continueRebase();

    expect(mockRebaseContinue).toHaveBeenCalledWith("repo-1");
    expect(result).toEqual(next);
    expect(monitor.state).toEqual(next);
    expect(monitor.rewrittenCount).toBe(1);
    monitor.stop();
  });

  it("continueRebase refreshes conflicts immediately when still paused", async () => {
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();

    const stillPaused = state({ active: true, paused_for_edit: true });
    mockRebaseContinue.mockResolvedValue(stillPaused);
    mockConflicts.mockResolvedValue([CONFLICT_FILES[0] as ConflictFile]);

    await monitor.continueRebase();

    expect(monitor.paused).toBe(true);
    expect(monitor.conflictCount).toBe(1);
    monitor.stop();
  });

  it("continueRebase toasts and returns null on failure (never throws)", async () => {
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();
    mockRebaseContinue.mockRejectedValue(new Error("unresolved conflicts"));

    await expect(monitor.continueRebase()).resolves.toBeNull();
    expect(monitor.error).toBe("unresolved conflicts");
    expect(getToasts().some((t) => t.kind === "error" && t.message.includes("unresolved conflicts"))).toBe(true);
    monitor.stop();
  });

  it("continueRebase is a no-op without a bound repo", async () => {
    const monitor = new RebaseMonitor();
    await expect(monitor.continueRebase()).resolves.toBeNull();
    expect(mockRebaseContinue).not.toHaveBeenCalled();
  });

  it("abort delegates to rebase_abort, toasts and fully resets", async () => {
    mockRebaseState.mockResolvedValue(RUNNING);
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();

    const aborted = await monitor.abort();

    expect(aborted).toBe(true);
    expect(mockRebaseAbort).toHaveBeenCalledWith("repo-1");
    expect(getToasts().some((t) => t.message.includes("Rebase aborted"))).toBe(true);
    expect(monitor.state).toBeNull();
    expect(monitor.active).toBe(false);
    expect(monitor.polling).toBe(false);

    await vi.advanceTimersByTimeAsync(REBASE_POLL_MS * 2);
    expect(mockRebaseState).toHaveBeenCalledTimes(1); // polling is gone
  });

  it("abort toasts and returns false on failure (monitor keeps running)", async () => {
    mockRebaseState.mockResolvedValue(RUNNING);
    const monitor = new RebaseMonitor();
    monitor.start("repo-1");
    await flush();
    mockRebaseAbort.mockRejectedValue(new Error("cannot abort"));

    await expect(monitor.abort()).resolves.toBe(false);
    expect(monitor.error).toBe("cannot abort");
    expect(monitor.polling).toBe(true); // still watching the live rebase
    monitor.stop();
  });
});
