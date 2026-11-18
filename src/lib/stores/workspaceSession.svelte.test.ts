/**
 * Tests for the per-repo workspace session store (`workspaceSession`):
 * parse validation, load/save round-trips through the versioned
 * localStorage envelope, per-root independence, the debounced write,
 * flush/forget eviction, and the memory-only side (commit object, dirty
 * buffers) that must never reach storage.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CommitInfo, GitSignature } from "$lib/ipc/types";
import {
  WORKSPACE_SESSION_DEBOUNCE_MS,
  WORKSPACE_SESSION_KEY_PREFIX,
  diffRowKey,
  diffScrollKey,
  emptyStoredSession,
  fileScrollKey,
  flushWorkspaceSessions,
  parseStoredWorkspaceSession,
  resetWorkspaceSessions,
  workspaceSession,
  WorkspaceSessionStore,
  type StoredWorkspaceSession,
} from "./workspaceSession.svelte";

const ROOT = "/repo";
const KEY = WORKSPACE_SESSION_KEY_PREFIX + ROOT;

const sig: GitSignature = {
  name: "a",
  email: "a@example.com",
  time: 1,
  offset_minutes: 0,
};

/** A valid stored payload (the shape RepoView's syncSession writes). */
function validSession(): StoredWorkspaceSession {
  return {
    openFiles: ["src/a.ts", "src/b.ts"],
    activeFile: "src/a.ts",
    centerKind: "diff",
    diffs: [{ path: "src/a.ts", scope: "unstaged" }],
    activeDiffPath: "src/a.ts",
    commitSha: null,
    scrolls: { "file:src/a.ts": 120, "diff:src/a.ts": 640 },
  };
}

describe("parseStoredWorkspaceSession", () => {
  it("drops non-object payloads", () => {
    expect(parseStoredWorkspaceSession(null)).toBeNull();
    expect(parseStoredWorkspaceSession(42)).toBeNull();
    expect(parseStoredWorkspaceSession("nope")).toBeNull();
    expect(parseStoredWorkspaceSession([])).toBeNull();
  });

  it("drops sessions with a malformed openFiles list", () => {
    const session = validSession();
    expect(parseStoredWorkspaceSession({ ...session, openFiles: "a.ts" })).toBeNull();
    expect(parseStoredWorkspaceSession({ ...session, openFiles: [1, 2] })).toBeNull();
    expect(parseStoredWorkspaceSession({ ...session, openFiles: undefined })).toBeNull();
  });

  it("drops sessions with an unknown centerKind", () => {
    const session = validSession();
    expect(
      parseStoredWorkspaceSession({ ...session, centerKind: "compare" }),
    ).toBeNull();
    expect(
      parseStoredWorkspaceSession({ ...session, centerKind: 7 }),
    ).toBeNull();
  });

  it("drops sessions with malformed diff refs", () => {
    const session = validSession();
    expect(parseStoredWorkspaceSession({ ...session, diffs: "diff" })).toBeNull();
    expect(
      parseStoredWorkspaceSession({ ...session, diffs: [{ path: 1, scope: "unstaged" }] }),
    ).toBeNull();
    expect(
      parseStoredWorkspaceSession({ ...session, diffs: [{ path: "a", scope: "staged" }] }),
    ).not.toBeNull();
    expect(
      parseStoredWorkspaceSession({ ...session, diffs: [null] }),
    ).toBeNull();
  });

  it("drops sessions with a non-record scrolls field", () => {
    const session = validSession();
    expect(parseStoredWorkspaceSession({ ...session, scrolls: [] })).toBeNull();
    expect(parseStoredWorkspaceSession({ ...session, scrolls: "0" })).toBeNull();
  });

  it("keeps valid sessions and coerces nullable strings", () => {
    expect(parseStoredWorkspaceSession(validSession())).toEqual(validSession());
    const sparse = parseStoredWorkspaceSession({
      openFiles: [],
      centerKind: "empty",
      diffs: [],
      activeFile: 42,
      activeDiffPath: undefined,
      commitSha: "abc123",
      scrolls: { "file:ok": 3, "file:bad": -1, "file:nan": "x" },
    });
    expect(sparse).toEqual({
      openFiles: [],
      activeFile: null,
      centerKind: "empty",
      diffs: [],
      activeDiffPath: null,
      commitSha: "abc123",
      // Invalid scroll entries drop individually, not the session.
      scrolls: { "file:ok": 3 },
    });
  });
});

describe("WorkspaceSessionStore", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    resetWorkspaceSessions();
    vi.useRealTimers();
  });

  it("get returns null before anything was stored, ensure defaults to empty", () => {
    const store = new WorkspaceSessionStore();
    expect(store.get(ROOT)).toBeNull();
    expect(store.ensure(ROOT)).toEqual(emptyStoredSession());
  });

  it("round-trips a session through the versioned envelope", () => {
    const store = new WorkspaceSessionStore();
    store.patch(ROOT, validSession());
    vi.advanceTimersByTime(WORKSPACE_SESSION_DEBOUNCE_MS);

    const envelope = JSON.parse(localStorage.getItem(KEY)!);
    expect(envelope.v).toBe(1);
    expect(envelope.data).toEqual(validSession());

    // A fresh instance (app relaunch) reads the persisted state back.
    expect(new WorkspaceSessionStore().get(ROOT)).toEqual(validSession());
  });

  it("keys storage by root so repos stay independent", () => {
    const store = new WorkspaceSessionStore();
    store.patch("/a", { openFiles: ["1"], centerKind: "files" });
    store.patch("/b", { openFiles: ["2"], centerKind: "commit", commitSha: "abc111" });
    vi.advanceTimersByTime(WORKSPACE_SESSION_DEBOUNCE_MS);

    expect(new WorkspaceSessionStore().get("/a")?.openFiles).toEqual(["1"]);
    expect(new WorkspaceSessionStore().get("/b")?.commitSha).toBe("abc111");
    expect(new WorkspaceSessionStore().get("/c")).toBeNull();
  });

  it("debounces writes per root: only the last state lands", () => {
    const store = new WorkspaceSessionStore();
    store.patch(ROOT, { openFiles: ["first"], centerKind: "files" });
    store.patch(ROOT, { openFiles: ["first", "second"], centerKind: "files" });
    vi.advanceTimersByTime(WORKSPACE_SESSION_DEBOUNCE_MS - 1);
    expect(localStorage.getItem(KEY)).toBeNull();
    vi.advanceTimersByTime(1);
    const envelope = JSON.parse(localStorage.getItem(KEY)!);
    expect(envelope.data.openFiles).toEqual(["first", "second"]);
  });

  it("merges patches without clobbering untouched keys", () => {
    const store = new WorkspaceSessionStore();
    store.patch(ROOT, validSession());
    store.patch(ROOT, { centerKind: "empty", diffs: [], activeDiffPath: null });
    expect(store.get(ROOT)).toEqual({
      ...validSession(),
      centerKind: "empty",
      diffs: [],
      activeDiffPath: null,
    });
  });

  it("merges scroll entries per key", () => {
    const store = new WorkspaceSessionStore();
    store.setScroll(ROOT, fileScrollKey("src/a.ts"), 100);
    store.setScroll(ROOT, diffScrollKey("src/a.ts"), 999);
    store.setScroll(ROOT, fileScrollKey("src/a.ts"), 250);
    expect(store.get(ROOT)!.scrolls).toEqual({
      "file:src/a.ts": 250,
      "diff:src/a.ts": 999,
    });
  });

  it("keeps the diff first-visible-row alongside the px scroll", () => {
    // The row seed rides the same scrolls map under `diffrow:<path>` — no
    // schema change, and the file: filter in RepoView's restore skips it.
    const store = new WorkspaceSessionStore();
    store.setScroll(ROOT, diffScrollKey("src/a.ts"), 400);
    store.setScroll(ROOT, diffRowKey("src/a.ts"), 12);
    expect(store.get(ROOT)!.scrolls).toEqual({
      "diff:src/a.ts": 400,
      "diffrow:src/a.ts": 12,
    });
    // Round-trips through the envelope (after the debounced write flush).
    store.flush();
    expect(new WorkspaceSessionStore().get(ROOT)!.scrolls).toEqual({
      "diff:src/a.ts": 400,
      "diffrow:src/a.ts": 12,
    });
  });

  it("flush writes pending sessions immediately", () => {
    const store = new WorkspaceSessionStore();
    store.patch(ROOT, { openFiles: ["x"], centerKind: "files" });
    expect(localStorage.getItem(KEY)).toBeNull();
    store.flush();
    expect(localStorage.getItem(KEY)).not.toBeNull();
    expect(new WorkspaceSessionStore().get(ROOT)?.openFiles).toEqual(["x"]);
  });

  it("forgetFile drops the file's scroll entry and schedules a write", () => {
    const store = new WorkspaceSessionStore();
    store.patch(ROOT, validSession());
    store.flush();
    store.forgetFile(ROOT, "src/a.ts");
    store.flush();
    const session = store.get(ROOT)!;
    expect(session.scrolls["file:src/a.ts"]).toBeUndefined();
    expect(session.openFiles).toEqual(validSession().openFiles);
  });

  it("forget evicts memory, pending timers and storage", () => {
    const store = new WorkspaceSessionStore();
    store.patch(ROOT, validSession());
    store.setBuffer(ROOT, "src/a.ts", "half-typed");
    store.forget(ROOT);
    expect(store.get(ROOT)).toBeNull();
    expect(store.takeBuffers(ROOT)).toEqual({});
    // The debounced write must not resurrect the evicted session.
    vi.advanceTimersByTime(WORKSPACE_SESSION_DEBOUNCE_MS);
    expect(new WorkspaceSessionStore().get(ROOT)).toBeNull();
  });

  it("forgets via an empty-envelope write when storage has no removeItem", () => {
    const store = new WorkspaceSessionStore({
      storage: {
        getItem: (k) => localStorage.getItem(k),
        setItem: (k, v) => localStorage.setItem(k, v),
      },
    });
    store.patch(ROOT, validSession());
    store.flush();
    store.forget(ROOT);
    const envelope = JSON.parse(localStorage.getItem(KEY)!);
    expect(envelope.data).toEqual(emptyStoredSession());
  });

  it("keeps the commit object and dirty buffers in memory only", () => {
    const store = new WorkspaceSessionStore();
    const commit: CommitInfo = {
      sha: "abc123def4567890",
      parents: [],
      author: sig,
      committer: sig,
      summary: "feat: x",
      refs: [],
    };
    store.setCommit(ROOT, commit);
    store.setBuffer(ROOT, "src/a.ts", "half-typed");
    store.patch(ROOT, { centerKind: "commit", commitSha: commit.sha });
    store.flush();

    // Nothing buffer/commit-shaped reached storage.
    const envelope = JSON.parse(localStorage.getItem(KEY)!);
    expect(envelope.data).toEqual({
      ...emptyStoredSession(),
      centerKind: "commit",
      commitSha: "abc123def4567890",
    });

    expect(store.commitFor(ROOT)).toBe(commit);
    expect(store.takeBuffers(ROOT)).toEqual({ "src/a.ts": "half-typed" });
    // takeBuffers drains: the restore consumes them exactly once.
    expect(store.takeBuffers(ROOT)).toEqual({});
    expect(new WorkspaceSessionStore().commitFor(ROOT)).toBeNull();
  });

  it("works with storage unavailable (session stays in memory)", () => {
    const store = new WorkspaceSessionStore({ storage: null });
    store.patch(ROOT, validSession());
    vi.advanceTimersByTime(WORKSPACE_SESSION_DEBOUNCE_MS);
    expect(store.get(ROOT)).toEqual(validSession());
    expect(store.flush()).toBeUndefined();
    expect(new WorkspaceSessionStore({ storage: null }).get(ROOT)).toBeNull();
  });

  it("drops a malformed stored payload instead of throwing", () => {
    localStorage.setItem(KEY, "{not json");
    const store = new WorkspaceSessionStore();
    expect(store.get(ROOT)).toBeNull();
    expect(store.ensure(ROOT)).toEqual(emptyStoredSession());
  });
});

describe("module singleton delegates", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    resetWorkspaceSessions();
    vi.useRealTimers();
  });

  it("flushWorkspaceSessions flushes the shared singleton", () => {
    workspaceSession.patch(ROOT, { openFiles: ["y"], centerKind: "files" });
    flushWorkspaceSessions();
    expect(new WorkspaceSessionStore().get(ROOT)?.openFiles).toEqual(["y"]);
  });
});
