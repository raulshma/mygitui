/**
 * Unit tests for the op + auth stores (`$lib/stores/ops.svelte`). The IPC
 * client is fully mocked (`vi.mock`) — no Tauri runtime is touched.
 * `op-progress` / `auth-request` handlers are captured from the mocked
 * subscriptions so tests can push events on demand.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  authRespond,
  checkpointRestore,
  checkpoints,
  onAuthRequest,
  onOpLog,
  onOpProgress,
  opCancel,
  stage,
} from "$lib/ipc/client";
import type { AuthRequest, OpLog, OpProgress } from "$lib/ipc/types";
import { resetRepoEvents } from "$lib/events/repoEvents";
import { dismissToast, getToasts } from "$lib/toast";
import {
  AUTH_PROMPT_TIMEOUT_MS,
  authStore,
  AuthStore,
  invertStageEntry,
  MAX_UNDO_PER_REPO,
  opStore,
  OpStore,
  type AuthAnswerPayload,
  type UiUndoEntry,
} from "$lib/stores/ops.svelte";

vi.mock("$lib/ipc/client", () => ({
  authRespond: vi.fn(),
  opCancel: vi.fn(),
  checkpointRestore: vi.fn(),
  checkpoints: vi.fn(),
  stage: vi.fn(),
  onAuthRequest: vi.fn(),
  onOpProgress: vi.fn(),
  onOpLog: vi.fn(),
  // $lib/events/repoEvents (imported by ops.svelte) pulls the other event
  // seams in transitively; they are never called in this suite.
  onRepoChanged: vi.fn(),
  onCloneProgress: vi.fn(),
  onActionOutput: vi.fn(),
  onPtyOutput: vi.fn(),
  onPtyExit: vi.fn(),
}));

const mockAuthRespond = vi.mocked(authRespond);
const mockOnAuthRequest = vi.mocked(onAuthRequest);
const mockOnOpProgress = vi.mocked(onOpProgress);
const mockOnOpLog = vi.mocked(onOpLog);
const mockOpCancel = vi.mocked(opCancel);
const mockStage = vi.mocked(stage);
const mockCheckpoints = vi.mocked(checkpoints);
const mockCheckpointRestore = vi.mocked(checkpointRestore);

// ---------------------------------------------------------------------------
// Fixtures + subscription harness
// ---------------------------------------------------------------------------

function progress(patch: Partial<OpProgress> & { op_id: string; repo_id: string }): OpProgress {
  return {
    kind: "fetch",
    message: "contacting remote",
    pct: null,
    done: false,
    error: null,
    outcome: null,
    ...patch,
  };
}

function logEntry(
  patch: Partial<OpLog> & { op_id: string; repo_id: string },
): OpLog {
  return {
    kind: "commit",
    line: "hook output",
    ...patch,
  };
}

const AUTH_REQ: AuthRequest = {
  op_id: "op-1",
  repo_id: "repo-1",
  url: "https://host/repo.git",
  kind: "https-user",
  prompt: "Username for https://host/repo.git",
};

const opHandlers: Array<(event: OpProgress) => void> = [];
const logHandlers: Array<(event: OpLog) => void> = [];
const authHandlers: Array<(event: AuthRequest) => void> = [];
const unlistenSpy = vi.fn();

beforeEach(() => {
  opHandlers.length = 0;
  logHandlers.length = 0;
  authHandlers.length = 0;
  unlistenSpy.mockReset();
  mockAuthRespond.mockReset();
  mockOnOpProgress.mockReset();
  mockOnOpLog.mockReset();
  mockOnAuthRequest.mockReset();
  mockOpCancel.mockReset();
  mockAuthRespond.mockResolvedValue(undefined);
  mockOpCancel.mockResolvedValue(true);
  mockOnOpProgress.mockImplementation(
    ((cb: (event: OpProgress) => void) => {
      opHandlers.push(cb);
      return Promise.resolve(unlistenSpy);
    }) as typeof onOpProgress,
  );
  mockOnOpLog.mockImplementation(
    ((cb: (event: OpLog) => void) => {
      logHandlers.push(cb);
      return Promise.resolve(unlistenSpy);
    }) as typeof onOpLog,
  );
  mockOnAuthRequest.mockImplementation(
    ((cb: (event: AuthRequest) => void) => {
      authHandlers.push(cb);
      return Promise.resolve(unlistenSpy);
    }) as typeof onAuthRequest,
  );
});

afterEach(() => {
  opStore.stop();
  authStore.stop();
  resetRepoEvents(); // drops the module-owned backend subscriptions
});

/** Flushes pending microtasks (subscription + respond chains). */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

// ---------------------------------------------------------------------------
// OpStore
// ---------------------------------------------------------------------------

describe("OpStore", () => {
  it("startOpsEvents subscribes once and routes op-progress events", async () => {
    const store = new OpStore();
    store.startOpsEvents();
    store.startOpsEvents(); // idempotent
    await flush();

    expect(mockOnOpProgress).toHaveBeenCalledTimes(1);
    expect(opHandlers).toHaveLength(1);

    opHandlers[0]!(progress({ op_id: "a", repo_id: "repo-1" }));

    expect(store.opsFor("repo-1")).toHaveLength(1);
    expect(store.opsFor("repo-1")[0]).toMatchObject({
      op_id: "a",
      kind: "fetch",
      message: "contacting remote",
    });
    expect(store.busy("repo-1")).toBe(true);
    expect(store.busy("repo-2")).toBe(false);

    store.stop();
  });

  it("upserts progress by op_id (updates, no duplicates)", () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", pct: 10 }));
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", pct: 80, message: "receiving" }));

    const ops = store.opsFor("repo-1");
    expect(ops).toHaveLength(1);
    expect(ops[0]).toMatchObject({ pct: 80, message: "receiving" });

    store.applyProgress(progress({ op_id: "b", repo_id: "repo-1", kind: "push" }));
    expect(store.opsFor("repo-1")).toHaveLength(2);
    expect(store.busy("repo-1", "push")).toBe(true);
    expect(store.busy("repo-1", "commit")).toBe(false);
  });

  it("drops the op on done and clears the repo bucket when empty", () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1" }));
    store.applyProgress(progress({ op_id: "b", repo_id: "repo-1" }));

    store.applyProgress(
      progress({ op_id: "a", repo_id: "repo-1", done: true }),
    );
    expect(store.opsFor("repo-1").map((op) => op.op_id)).toEqual(["b"]);

    store.applyProgress(
      progress({ op_id: "b", repo_id: "repo-1", done: true }),
    );
    expect(store.opsFor("repo-1")).toEqual([]);
    expect(store.busy("repo-1")).toBe(false);
    expect(Object.keys(store.ops)).toEqual([]);
  });

  it("streams op-log lines per op and keeps them after completion", async () => {
    const store = new OpStore();
    store.startOpsEvents();
    await flush();

    expect(mockOnOpLog).toHaveBeenCalledTimes(1);
    expect(logHandlers).toHaveLength(1);

    logHandlers[0]!(logEntry({ op_id: "a", repo_id: "repo-1", line: "hook: lint" }));
    logHandlers[0]!(logEntry({ op_id: "a", repo_id: "repo-1", line: "hook: tests" }));

    expect(store.logsFor("repo-1")).toMatchObject({
      op_id: "a",
      kind: "commit",
      lines: ["hook: lint", "hook: tests"],
      done: false,
      error: null,
    });

    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "commit", done: true }));

    expect(store.logsFor("repo-1")).toMatchObject({ done: true, error: null });
    expect(store.logsFor("repo-2")).toBeNull();
    store.stop();
  });

  it("a new op of the same kind replaces stale output; clear dismisses it", () => {
    const store = new OpStore();
    store.applyLog(logEntry({ op_id: "a", repo_id: "repo-1", line: "old" }));
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "commit", done: true }));

    store.applyProgress(
      progress({ op_id: "b", repo_id: "repo-1", kind: "commit", message: "committing" }),
    );
    expect(store.logsFor("repo-1")).toBeNull();

    store.applyLog(logEntry({ op_id: "b", repo_id: "repo-1", line: "new" }));
    expect(store.logsFor("repo-1")?.lines).toEqual(["new"]);

    store.clearLogs("repo-1");
    expect(store.logsFor("repo-1")).toBeNull();
  });

  it("toasts an error when an op completes with an error", () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "push" }));
    store.applyProgress(
      progress({ op_id: "a", repo_id: "repo-1", kind: "push", done: true, error: "non-fast-forward" }),
    );

    expect(store.opsFor("repo-1")).toEqual([]);
    expect(
      getToasts().some(
        (t) => t.kind === "error" && t.message.includes("Push failed: non-fast-forward"),
      ),
    ).toBe(true);
  });

  it("toasts a cancellation completion as info, not error", () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "fetch" }));
    store.applyProgress(
      progress({ op_id: "a", repo_id: "repo-1", kind: "fetch", done: true, error: "cancelled" }),
    );

    expect(store.opsFor("repo-1")).toEqual([]);
    expect(getToasts().some((t) => t.kind === "info" && t.message === "Cancelled")).toBe(true);
    expect(getToasts().some((t) => t.kind === "error" && t.message.includes("cancelled"))).toBe(
      false,
    );
  });

  it("cancel marks the op cancelling, calls opCancel, and keeps the flag across progress", async () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "push" }));

    const pending = store.cancel("a");
    // Flagged synchronously so the UI spins before the IPC resolves.
    expect(store.opsFor("repo-1")[0]?.cancelling).toBe(true);

    await pending;
    expect(mockOpCancel).toHaveBeenCalledWith("repo-1", "a");

    // A later progress event upserts the op but preserves the flag.
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", pct: 50, message: "sending" }));
    expect(store.opsFor("repo-1")[0]).toMatchObject({ cancelling: true, pct: 50 });

    // The final cancellation error toasts as info.
    store.applyProgress(
      progress({ op_id: "a", repo_id: "repo-1", done: true, error: "cancelled" }),
    );
    expect(getToasts().some((t) => t.kind === "info" && t.message === "Cancelled")).toBe(true);
  });

  it("cancel un-flags the op when the backend reports it already finished", async () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "fetch" }));

    mockOpCancel.mockResolvedValueOnce(false);
    await expect(store.cancel("a")).resolves.toBe(false);
    expect(store.opsFor("repo-1")[0]?.cancelling).toBe(false);
  });

  it("cancel un-flags and toasts when the IPC call rejects", async () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "push" }));

    mockOpCancel.mockRejectedValueOnce(new Error("backend gone") as never);
    await expect(store.cancel("a")).resolves.toBe(false);
    expect(store.opsFor("repo-1")[0]?.cancelling).toBe(false);
    expect(
      getToasts().some((t) => t.kind === "error" && t.message.includes("backend gone")),
    ).toBe(true);
  });

  it("cancel resolves false without IPC for an unknown op", async () => {
    const store = new OpStore();
    await expect(store.cancel("nope")).resolves.toBe(false);
    expect(mockOpCancel).not.toHaveBeenCalled();
  });

  it("stop deregisters the store and clears state; restart re-registers", async () => {
    const store = new OpStore();
    store.startOpsEvents();
    await flush();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1" }));

    store.stop();

    expect(store.busy("repo-1")).toBe(false);

    // The module owns the backend subscription (stop() must not tear it
    // down); the deregistered store simply stops receiving events.
    opHandlers[0]!(progress({ op_id: "b", repo_id: "repo-1" }));
    expect(store.busy("repo-1")).toBe(false);
    expect(unlistenSpy).not.toHaveBeenCalled();

    // The store can be restarted: it re-registers and events flow again.
    store.startOpsEvents();
    await flush();
    opHandlers[0]!(progress({ op_id: "c", repo_id: "repo-1" }));
    expect(store.busy("repo-1")).toBe(true);
    store.stop();
  });

  // -- completed-op history ring -----------------------------------------------

  it("a done op lands in history as succeeded (typed outcome) and leaves the active list", () => {
    const store = new OpStore();
    store.applyProgress(
      progress({ op_id: "a", repo_id: "repo-1", kind: "push", message: "pushing to origin" }),
    );
    // The final event carries the op's last message (backend contract).
    store.applyProgress(
      progress({
        op_id: "a",
        repo_id: "repo-1",
        kind: "push",
        message: "pushing to origin",
        done: true,
        outcome: "completed",
      }),
    );

    expect(store.opsFor("repo-1")).toEqual([]); // still dropped from active
    expect(store.historyFor("repo-1")).toHaveLength(1);
    expect(store.historyFor("repo-1")[0]).toMatchObject({
      op_id: "a",
      kind: "push",
      message: "pushing to origin",
      outcome: "succeeded",
      error: null,
    });
    expect(store.historyFor("repo-2")).toEqual([]); // per-repo
  });

  it("records failed ops via the typed outcome and keeps the error", () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "push" }));
    store.applyProgress(
      progress({
        op_id: "a",
        repo_id: "repo-1",
        kind: "push",
        done: true,
        error: "non-fast-forward",
        outcome: "failed",
      }),
    );

    expect(store.historyFor("repo-1")[0]).toMatchObject({
      outcome: "failed",
      error: "non-fast-forward",
    });
  });

  it("classifies failed and cancelled via legacy error text when outcome is absent", () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "fetch" }));
    store.applyProgress(
      progress({ op_id: "a", repo_id: "repo-1", kind: "fetch", done: true, error: "cancelled" }),
    );
    store.applyProgress(progress({ op_id: "b", repo_id: "repo-1", kind: "pull" }));
    store.applyProgress(
      progress({ op_id: "b", repo_id: "repo-1", kind: "pull", done: true, error: "merge conflict" }),
    );

    const [pull, fetch] = store.historyFor("repo-1"); // newest first
    expect(pull).toMatchObject({ op_id: "b", outcome: "failed", error: "merge conflict" });
    expect(fetch).toMatchObject({ op_id: "a", outcome: "cancelled", error: "cancelled" });
  });

  it("records cancelled via the typed outcome even when the error text lacks 'cancel'", () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "push" }));
    store.applyProgress(
      progress({
        op_id: "a",
        repo_id: "repo-1",
        kind: "push",
        done: true,
        error: "remote hook declined the push",
        outcome: "cancelled",
      }),
    );

    expect(store.historyFor("repo-1")[0]).toMatchObject({
      outcome: "cancelled",
      error: "remote hook declined the push",
    });
  });

  it("started_at approximates the first progress event; done-as-first-event records zero duration", () => {
    const store = new OpStore();
    const before = Date.now();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1" }));
    const afterFirst = Date.now();
    store.applyProgress(
      progress({ op_id: "a", repo_id: "repo-1", done: true, outcome: "completed" }),
    );

    const seen = store.historyFor("repo-1")[0]!;
    expect(seen.started_at).toBeGreaterThanOrEqual(before);
    expect(seen.started_at).toBeLessThanOrEqual(afterFirst);
    expect(seen.finished_at).toBeGreaterThanOrEqual(seen.started_at);

    // An op whose done event is the first one seen (fast op / store restart
    // mid-run) falls back to a zero duration instead of a wrong start.
    store.applyProgress(
      progress({ op_id: "b", repo_id: "repo-1", done: true, outcome: "completed" }),
    );
    const unseen = store.historyFor("repo-1")[0]!;
    expect(unseen.op_id).toBe("b");
    expect(unseen.started_at).toBe(unseen.finished_at);
  });

  it("caps the ring at 25 per repo, newest first, oldest dropped", () => {
    const store = new OpStore();
    for (let i = 0; i < 30; i++) {
      store.applyProgress(progress({ op_id: `op-${i}`, repo_id: "repo-1" }));
      store.applyProgress(
        progress({ op_id: `op-${i}`, repo_id: "repo-1", done: true, outcome: "completed" }),
      );
    }

    const history = store.historyFor("repo-1");
    expect(history).toHaveLength(25);
    expect(history[0]?.op_id).toBe("op-29"); // newest at the front
    expect(history[24]?.op_id).toBe("op-5"); // op-0 … op-4 fell off
    expect(history.some((entry) => entry.op_id === "op-4")).toBe(false);
  });

  it("clearHistory drops only the repo's history; stop clears everything", () => {
    const store = new OpStore();
    for (const repo of ["repo-1", "repo-2"]) {
      store.applyProgress(progress({ op_id: "a", repo_id: repo }));
      store.applyProgress(
        progress({ op_id: "a", repo_id: repo, done: true, outcome: "completed" }),
      );
    }

    store.clearHistory("repo-1");
    expect(store.historyFor("repo-1")).toEqual([]);
    expect(store.historyFor("repo-2")).toHaveLength(1);

    store.stop();
    expect(store.historyFor("repo-2")).toEqual([]);
  });
});

describe("UI undo stack (batch B item 41)", () => {
  const stageEntry = (label = "stage 1 file"): UiUndoEntry => ({
    kind: "stage",
    staged: ["a.txt"],
    unstaged: [],
    label,
  });
  const checkpointEntry = (id = "cp-1"): UiUndoEntry => ({
    kind: "checkpoint",
    checkpointId: id,
    label: `discard — restore checkpoint ${id}`,
  });

  beforeEach(() => {
    mockStage.mockReset();
    mockCheckpoints.mockReset();
    mockCheckpointRestore.mockReset();
    mockStage.mockResolvedValue(undefined);
    mockCheckpointRestore.mockResolvedValue(undefined);
  });

  it("pushes up to MAX_UNDO_PER_REPO entries, dropping the oldest", () => {
    const store = new OpStore();
    for (let i = 0; i < MAX_UNDO_PER_REPO + 2; i++) {
      store.pushUndo("repo-1", stageEntry(`e${i}`));
    }
    expect(store.undoCount("repo-1")).toBe(MAX_UNDO_PER_REPO);
    // Newest kept; the two oldest dropped.
    const stack = store.undoStacks["repo-1"]!;
    expect(stack.at(-1)?.kind === "stage" && stack.at(-1)!.label === `e${MAX_UNDO_PER_REPO + 1}`).toBe(
      true,
    );
    expect(stack[0]?.kind === "stage" && stack[0]!.label === "e2").toBe(true);
    // Stacks are per repo.
    expect(store.undoCount("repo-2")).toBe(0);
  });

  it("inverts stage entries: undo stages what was unstaged and vice versa", () => {
    expect(
      invertStageEntry({ kind: "stage", staged: ["a", "b"], unstaged: ["c"], label: "x" }),
    ).toEqual({ restage: ["c"], unstage: ["a", "b"] });
  });

  it("runUndo pops the newest stage entry and re-runs the inverted stage", async () => {
    const store = new OpStore();
    store.pushUndo("repo-1", {
      kind: "stage",
      staged: ["a.txt"],
      unstaged: ["b.txt"],
      label: "stage/unstage of 2 files",
    });
    mockStage.mockResolvedValue(undefined);

    const entry = await store.runUndo("repo-1");

    expect(entry).not.toBeNull();
    expect(store.undoCount("repo-1")).toBe(0);
    // unstage first (invert of "staged"), then restage.
    expect(mockStage).toHaveBeenNthCalledWith(1, "repo-1", {
      targets: [{ file: "a.txt" }],
      unstage: true,
    });
    expect(mockStage).toHaveBeenNthCalledWith(2, "repo-1", {
      targets: [{ file: "b.txt" }],
      unstage: false,
    });
    // Success toast announces what was undone.
    expect(getToasts().some((t) => t.message.startsWith("Undone:"))).toBe(true);
  });

  it("runUndo restores a discard's checkpoint via checkpoint_restore", async () => {
    const store = new OpStore();
    store.pushUndo("repo-1", checkpointEntry("cp-9"));

    const entry = await store.runUndo("repo-1");

    expect(entry).toEqual({ kind: "checkpoint", checkpointId: "cp-9", label: "discard — restore checkpoint cp-9" });
    expect(mockCheckpointRestore).toHaveBeenCalledWith("repo-1", "cp-9");
    expect(mockStage).not.toHaveBeenCalled();
  });

  it("refuses to undo while an op is in flight (stack untouched)", async () => {
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1" })); // in-flight op
    store.pushUndo("repo-1", stageEntry());

    const entry = await store.runUndo("repo-1");

    expect(entry).toBeNull();
    expect(store.undoCount("repo-1")).toBe(1);
    expect(mockStage).not.toHaveBeenCalled();
    expect(getToasts().some((t) => t.message.includes("Wait for the running op"))).toBe(true);
  });

  it("consumes the entry and toasts on an undo failure", async () => {
    const store = new OpStore();
    store.pushUndo("repo-1", checkpointEntry("cp-gone"));
    mockCheckpointRestore.mockRejectedValue(new Error("checkpoint vanished"));

    const entry = await store.runUndo("repo-1");

    expect(entry).not.toBeNull(); // consumed even though it failed
    expect(store.undoCount("repo-1")).toBe(0);
    expect(getToasts().some((t) => t.message.startsWith("Undo failed:"))).toBe(true);
  });

  it("clearUndo drops the repo's stack without touching others", () => {
    const store = new OpStore();
    store.pushUndo("repo-1", stageEntry());
    store.pushUndo("repo-2", stageEntry());

    store.clearUndo("repo-1");

    expect(store.undoCount("repo-1")).toBe(0);
    expect(store.undoCount("repo-2")).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// AuthStore
// ---------------------------------------------------------------------------

describe("AuthStore", () => {
  it("startAuthEvents subscribes once and surfaces requests as pending", async () => {
    const store = new AuthStore();
    store.startAuthEvents();
    store.startAuthEvents();
    await flush();

    expect(mockOnAuthRequest).toHaveBeenCalledTimes(1);
    expect(store.pending).toBeNull();

    authHandlers[0]!(AUTH_REQ);

    expect(store.pending).toEqual(AUTH_REQ);
    store.stop();
  });

  it("answer sends credentials via authRespond and resolves waiters", async () => {
    const store = new AuthStore();
    store.handleRequest(AUTH_REQ);

    const payload: AuthAnswerPayload = { username: "ada", password: "s3cret", store: true };
    let resolved: AuthAnswerPayload | null | undefined;
    void store.whenAnswered("op-1").then((value) => {
      resolved = value;
    });

    await store.answer(payload);

    expect(mockAuthRespond).toHaveBeenCalledWith("op-1", "ada", "s3cret", true);
    expect(store.pending).toBeNull();
    expect(resolved).toEqual(payload);
  });

  it("dismiss responds with empty credentials (cancel) and resolves null", async () => {
    const store = new AuthStore();
    store.handleRequest(AUTH_REQ);

    let resolved: AuthAnswerPayload | null | undefined;
    void store.whenAnswered("op-1").then((value) => {
      resolved = value;
    });

    await store.dismiss();

    // The store passes positional args; the client wrapper omits empty ones.
    expect(mockAuthRespond).toHaveBeenCalledWith("op-1", undefined, undefined, false);
    expect(resolved).toBeNull();
    expect(store.pending).toBeNull();
  });

  it("queues concurrent requests and shows them one at a time", async () => {
    const store = new AuthStore();
    const second: AuthRequest = { ...AUTH_REQ, op_id: "op-2", kind: "ssh-passphrase" };

    store.handleRequest(AUTH_REQ);
    store.handleRequest(second);

    expect(store.pending?.op_id).toBe("op-1");

    await store.answer({ password: "pw" });

    expect(store.pending?.op_id).toBe("op-2");

    await store.dismiss();

    expect(store.pending).toBeNull();
    expect(mockAuthRespond).toHaveBeenNthCalledWith(1, "op-1", undefined, "pw", false);
    expect(mockAuthRespond).toHaveBeenNthCalledWith(2, "op-2", undefined, undefined, false);
  });

  it("answer failures surface a toast instead of throwing", async () => {
    mockAuthRespond.mockRejectedValueOnce(new Error("op already gone") as never);
    const store = new AuthStore();
    store.handleRequest(AUTH_REQ);

    await expect(store.answer({ password: "pw" })).resolves.toBeUndefined();
    expect(
      getToasts().some(
        (t) => t.kind === "error" && t.message.includes("op already gone"),
      ),
    ).toBe(true);
  });

  it("answer/dismiss are no-ops without a pending request", async () => {
    const store = new AuthStore();
    await store.answer({ password: "pw" });
    await store.dismiss();
    expect(mockAuthRespond).not.toHaveBeenCalled();
  });

  it("stop resolves outstanding waiters with null", async () => {
    const store = new AuthStore();
    store.handleRequest(AUTH_REQ);
    let resolved: AuthAnswerPayload | null | undefined;
    void store.whenAnswered("op-1").then((value) => {
      resolved = value;
    });

    store.stop();
    await flush();

    expect(resolved).toBeNull();
    expect(store.pending).toBeNull();
  });

  // -- state-machine audit regressions ---------------------------------------

  it("the typed outcome classifies the toast, even when error text lacks 'cancel'", () => {
    for (const t of getToasts()) dismissToast(t.id); // toasts accumulate across tests
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "push" }));
    store.applyProgress(
      progress({
        op_id: "a",
        repo_id: "repo-1",
        kind: "push",
        done: true,
        error: "remote hook declined the push",
        outcome: "cancelled",
      }),
    );

    expect(getToasts().some((t) => t.kind === "info" && t.message === "Cancelled")).toBe(true);
  });

  it("outcome 'failed' wins over error text that merely contains 'cancel'", () => {
    for (const t of getToasts()) dismissToast(t.id); // toasts accumulate across tests
    const store = new OpStore();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1", kind: "fetch" }));
    store.applyProgress(
      progress({
        op_id: "a",
        repo_id: "repo-1",
        kind: "fetch",
        done: true,
        error: "failed to cancel the transfer lock",
        outcome: "failed",
      }),
    );

    expect(
      getToasts().some((t) => t.kind === "error" && t.message.includes("failed to cancel")),
    ).toBe(true);
    expect(getToasts().some((t) => t.kind === "info" && t.message === "Cancelled")).toBe(false);
  });

  it("an auth dialog for a finished op is cleared instead of sticking forever", async () => {
    const ops = new OpStore();
    const auth = new AuthStore();
    ops.onOpFinished = (opId) => auth.notifyOpFinished(opId);
    auth.handleRequest(AUTH_REQ);
    expect(auth.pending?.op_id).toBe("op-1");

    // The op ends without an answer (cancelled / failed / queue dropped):
    // the done event must retire the dialog and resolve its waiters.
    let resolved: AuthAnswerPayload | null | undefined;
    void auth.whenAnswered("op-1").then((value) => {
      resolved = value;
    });

    ops.applyProgress(
      progress({
        op_id: "op-1",
        repo_id: "repo-1",
        kind: "push",
        done: true,
        error: "cancelled",
        outcome: "cancelled",
      }),
    );

    expect(auth.pending).toBeNull();
    await flush();
    expect(resolved).toBeNull();
  });

  it("a queued auth request for a finished op is dropped from the queue", () => {
    const auth = new AuthStore();
    auth.handleRequest(AUTH_REQ);
    const second: AuthRequest = { ...AUTH_REQ, op_id: "op-2", kind: "ssh-passphrase" };
    auth.handleRequest(second);
    // op-1 shows, op-2 waits in line — then op-2's op dies.
    auth.notifyOpFinished("op-2");

    expect(auth.pending?.op_id).toBe("op-1");
    // Answering op-1 must not surface the dead request.
    void auth.answer({ password: "pw" });
    expect(auth.pending).toBeNull();
  });

  it("pruneStale retires requests older than the backend's 120s prompt timeout", () => {
    const auth = new AuthStore();
    auth.handleRequest(AUTH_REQ);
    expect(auth.pending).not.toBeNull();

    auth.pruneStale(Date.now() + 119_000);
    expect(auth.pending).not.toBeNull();

    auth.pruneStale(Date.now() + AUTH_PROMPT_TIMEOUT_MS + 1);
    expect(auth.pending).toBeNull();
  });
});
