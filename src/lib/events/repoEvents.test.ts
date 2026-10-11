/**
 * Unit tests for the backend-event fan-out module (`$lib/events/repoEvents`).
 *
 * Push events reach the frontend through Tauri's `listen` (not `invoke`), so
 * the seam mocked here is `@tauri-apps/api/event#listen` plus the
 * `isTauri` guard — the same "no Tauri runtime in jsdom" setup
 * client.test.ts achieves with `setTransport` for commands. The module and
 * the client stay real: tests drive register → subscribe → deliver →
 * filter → forward end to end, and `resetRepoEvents` gives every test a
 * fresh module.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import {
  isSignificant,
  onOpProgress,
  onPtyOutput,
  onRepoChange,
  repoChangeKind,
  resetRepoEvents,
  type RepoChange,
} from "$lib/events/repoEvents";
import type {
  OpProgress,
  PtyOutputEvent,
  RepoChangedEvent,
} from "$lib/ipc/types";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

vi.mock("$lib/entry/dragdrop", () => ({
  isTauri: vi.fn(() => true),
}));

const mockListen = vi.mocked(listen);

/** Subscription records captured from the mocked tauri `listen`. */
interface ListenCall {
  name: string;
  handler: (event: { payload: unknown }) => void;
}

const listenCalls: ListenCall[] = [];
const unlistenSpy = vi.fn();

/** Delivers one backend event payload through the (mocked) tauri bus. */
function emit(name: string, payload: unknown): void {
  const call = listenCalls.find((entry) => entry.name === name);
  if (!call) throw new Error(`no subscription for "${name}"`);
  call.handler({ payload });
}

/** Drains the subscription promise chain so `listen` calls land. */
async function flush(): Promise<void> {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
}

function changeEvent(patch: Partial<RepoChangedEvent> = {}): RepoChangedEvent {
  return {
    repo_id: "repo-1",
    paths: [],
    head_moved: false,
    full: false,
    ...patch,
  };
}

/** A fully-typed RepoChange: the wire payload plus its derived kind. */
function asChange(patch: Partial<RepoChangedEvent> = {}): RepoChange {
  const event = changeEvent(patch);
  return { ...event, kind: repoChangeKind(event) };
}

beforeEach(() => {
  listenCalls.length = 0;
  unlistenSpy.mockReset();
  mockListen.mockReset();
  resetRepoEvents();
  mockListen.mockImplementation(
    ((name: string, handler: (event: { payload: unknown }) => void) => {
      listenCalls.push({ name, handler });
      return Promise.resolve(unlistenSpy);
    }) as unknown as typeof listen,
  );
});

afterEach(() => {
  resetRepoEvents();
});

// ---------------------------------------------------------------------------
// repo-changed forwarding + RepoChange kinds
// ---------------------------------------------------------------------------

describe("onRepoChange", () => {
  it("subscribes once per event name and forwards events with a derived kind", async () => {
    const received: RepoChange[] = [];
    onRepoChange({ handle: (change) => received.push(change) });
    await flush();

    expect(listenCalls.map((call) => call.name)).toEqual(["repo-changed"]);
    emit("repo-changed", changeEvent({ head_moved: true }));

    expect(received).toHaveLength(1);
    expect(received[0]).toMatchObject({
      repo_id: "repo-1",
      head_moved: true,
      full: false,
      kind: "head_moved",
    });
  });

  it("does not deliver filtered-out events; passing events reach the handler", async () => {
    const significant: RepoChange[] = [];
    const everything: RepoChange[] = [];
    onRepoChange({
      filter: isSignificant,
      handle: (change) => significant.push(change),
    });
    onRepoChange({ handle: (change) => everything.push(change) });
    await flush();

    emit("repo-changed", changeEvent({ paths: ["src/a.ts"] })); // index churn
    emit("repo-changed", changeEvent({ head_moved: true }));

    expect(significant.map((change) => change.kind)).toEqual(["head_moved"]);
    expect(everything.map((change) => change.kind)).toEqual(["paths", "head_moved"]);
  });

  it("unsubscribe stops delivery; re-registration reuses the module's subscription", async () => {
    const first: RepoChange[] = [];
    const second: RepoChange[] = [];
    const unsubscribe = onRepoChange({ handle: (change) => first.push(change) });
    await flush();
    expect(unlistenSpy).not.toHaveBeenCalled();

    emit("repo-changed", changeEvent({ full: true }));
    expect(first).toHaveLength(1);

    unsubscribe();
    unsubscribe(); // double-unsubscribe is safe
    emit("repo-changed", changeEvent({ full: true }));
    expect(first).toHaveLength(1); // no forwarding after unsubscribe

    onRepoChange({ handle: (change) => second.push(change) });
    await flush();
    // Module-owned lifecycle: still exactly one backend subscription.
    expect(listenCalls).toHaveLength(1);
    emit("repo-changed", changeEvent({ head_moved: true }));
    expect(second).toHaveLength(1);
  });

  it("isolates a throwing listener from the other consumers", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const received: RepoChange[] = [];
    onRepoChange({
      handle: () => {
        throw new Error("boom");
      },
    });
    onRepoChange({ handle: (change) => received.push(change) });
    await flush();

    expect(() => emit("repo-changed", changeEvent({ full: true }))).not.toThrow();
    expect(received).toHaveLength(1);
    expect(errorSpy).toHaveBeenCalled();
    errorSpy.mockRestore();
  });
});

// ---------------------------------------------------------------------------
// Channel separation + lazy when-first-needed subscriptions
// ---------------------------------------------------------------------------

describe("per-event channels", () => {
  it("keeps event names separate (repo-changed never reaches an op listener)", async () => {
    const ops: OpProgress[] = [];
    const changes: RepoChange[] = [];
    onOpProgress((event) => ops.push(event));
    onRepoChange({ handle: (change) => changes.push(change) });
    await flush();

    expect(listenCalls.map((call) => call.name).sort()).toEqual([
      "op-progress",
      "repo-changed",
    ]);

    emit("op-progress", { op_id: "a", repo_id: "repo-1" });
    emit("repo-changed", changeEvent({}));

    expect(ops).toHaveLength(1);
    expect(changes).toHaveLength(1);
  });

  it("subscribes lazily: only the event names actually registered for", async () => {
    const output: PtyOutputEvent[] = [];
    onPtyOutput((event) => output.push(event));
    await flush();

    expect(listenCalls.map((call) => call.name)).toEqual(["pty-output"]);
    expect(listenCalls.some((call) => call.name === "repo-changed")).toBe(false);

    emit("pty-output", { session_id: "s1", data: "hi" });
    expect(output).toEqual([{ session_id: "s1", data: "hi" }]);
  });
});

// ---------------------------------------------------------------------------
// resetRepoEvents (test teardown)
// ---------------------------------------------------------------------------

describe("resetRepoEvents", () => {
  it("unsubscribes the backend, drops listeners, and re-subscribes on demand", async () => {
    const received: RepoChange[] = [];
    onRepoChange({ handle: (change) => received.push(change) });
    await flush();

    resetRepoEvents();
    expect(unlistenSpy).toHaveBeenCalledTimes(1);

    emit("repo-changed", changeEvent({ full: true }));
    expect(received).toHaveLength(0); // listeners were dropped too

    onRepoChange({ handle: (change) => received.push(change) });
    await flush();
    expect(listenCalls).toHaveLength(2); // fresh backend subscription
    emit("repo-changed", changeEvent({ head_moved: true }));
    expect(received).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// RepoChange kind derivation + isSignificant (each kind)
// ---------------------------------------------------------------------------

describe("repoChangeKind", () => {
  it("derives the named kind from the watcher payload (full wins)", () => {
    expect(repoChangeKind(changeEvent({ paths: ["src/a.ts"] }))).toBe("paths");
    expect(repoChangeKind(changeEvent({ head_moved: true }))).toBe("head_moved");
    expect(repoChangeKind(changeEvent({ full: true }))).toBe("full");
    expect(
      repoChangeKind(changeEvent({ head_moved: true, full: true })),
    ).toBe("full");
  });
});

describe("isSignificant (the head_moved || full policy from 21571ee)", () => {
  it("accepts head_moved and full, rejects index-only churn", () => {
    expect(isSignificant(asChange({ paths: [".git/index"] }))).toBe(false);
    expect(isSignificant(asChange({ paths: ["src/a.ts"] }))).toBe(false);
    expect(isSignificant(asChange({ head_moved: true }))).toBe(true);
    expect(isSignificant(asChange({ full: true }))).toBe(true);
    expect(isSignificant(asChange({ head_moved: true, full: true }))).toBe(true);
  });
});
