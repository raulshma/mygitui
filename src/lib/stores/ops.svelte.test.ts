/**
 * Unit tests for the op + auth stores (`$lib/stores/ops.svelte`). The IPC
 * client is fully mocked (`vi.mock`) — no Tauri runtime is touched.
 * `op-progress` / `auth-request` handlers are captured from the mocked
 * subscriptions so tests can push events on demand.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { authRespond, onAuthRequest, onOpProgress } from "$lib/ipc/client";
import type { AuthRequest, OpProgress } from "$lib/ipc/types";
import { getToasts } from "$lib/toast";
import {
  authStore,
  AuthStore,
  opStore,
  OpStore,
  type AuthAnswerPayload,
} from "$lib/stores/ops.svelte";

vi.mock("$lib/ipc/client", () => ({
  authRespond: vi.fn(),
  onAuthRequest: vi.fn(),
  onOpProgress: vi.fn(),
}));

const mockAuthRespond = vi.mocked(authRespond);
const mockOnAuthRequest = vi.mocked(onAuthRequest);
const mockOnOpProgress = vi.mocked(onOpProgress);

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
const authHandlers: Array<(event: AuthRequest) => void> = [];
const unlistenSpy = vi.fn();

beforeEach(() => {
  opHandlers.length = 0;
  authHandlers.length = 0;
  unlistenSpy.mockReset();
  mockAuthRespond.mockReset();
  mockOnOpProgress.mockReset();
  mockOnAuthRequest.mockReset();
  mockAuthRespond.mockResolvedValue(undefined);
  mockOnOpProgress.mockImplementation(
    ((cb: (event: OpProgress) => void) => {
      opHandlers.push(cb);
      return Promise.resolve(unlistenSpy);
    }) as typeof onOpProgress,
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

  it("stop unsubscribes and clears state", async () => {
    const store = new OpStore();
    store.startOpsEvents();
    await flush();
    store.applyProgress(progress({ op_id: "a", repo_id: "repo-1" }));

    store.stop();

    expect(unlistenSpy).toHaveBeenCalledTimes(1);
    expect(store.busy("repo-1")).toBe(false);

    // Events after stop are not delivered through the subscription anymore
    // (handler captured above would still work if invoked, but the real
    // unlisten is gone); the store can be restarted.
    store.startOpsEvents();
    await flush();
    expect(mockOnOpProgress).toHaveBeenCalledTimes(2);
    store.stop();
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
});
