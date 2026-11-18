/**
 * Unit tests for the terminal session store (`terminalStore.svelte.ts`).
 * The IPC client, the Tauri guard and toasts are fully mocked (`vi.mock`);
 * the terminal UI is a `FakeTerminal` injected through `setFactory` — no
 * xterm.js or Tauri runtime is touched.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  onPtyExit,
  onPtyOutput,
  ptyCreate,
  ptyKill,
  ptyResize,
  ptyWrite,
} from "$lib/ipc/client";
import { isTauri } from "$lib/entry/dragdrop";
import { resetRepoEvents } from "$lib/events/repoEvents";
import { toast } from "$lib/toast";
import {
  MAX_TERMINALS_PER_REPO,
  RESIZE_DEBOUNCE_MS,
  sessionKey,
  splitSessionKey,
  TerminalStore,
  type TerminalFactory,
  type TerminalLike,
} from "./terminalStore.svelte";

vi.mock("$lib/ipc/client", () => ({
  ptyCreate: vi.fn(),
  ptyWrite: vi.fn(),
  ptyResize: vi.fn(),
  ptyKill: vi.fn(),
  onPtyOutput: vi.fn(),
  onPtyExit: vi.fn(),
  // $lib/events/repoEvents (imported by terminalStore) pulls the other
  // event seams in transitively; they are never called in this suite.
  onRepoChanged: vi.fn(),
  onCloneProgress: vi.fn(),
  onOpProgress: vi.fn(),
  onOpLog: vi.fn(),
  onAuthRequest: vi.fn(),
  onActionOutput: vi.fn(),
}));

vi.mock("$lib/entry/dragdrop", () => ({
  isTauri: vi.fn(() => true),
}));

vi.mock("$lib/toast", () => ({
  toast: vi.fn(),
}));

const mockPtyCreate = vi.mocked(ptyCreate);
const mockPtyWrite = vi.mocked(ptyWrite);
const mockPtyResize = vi.mocked(ptyResize);
const mockPtyKill = vi.mocked(ptyKill);
const mockOnPtyOutput = vi.mocked(onPtyOutput);
const mockOnPtyExit = vi.mocked(onPtyExit);
const mockIsTauri = vi.mocked(isTauri);
const mockToast = vi.mocked(toast);

/** Event callbacks captured from the (mocked) tauri subscriptions. */
let emitOutput: ((session_id: string, data: string) => void) | null = null;
let emitExit: ((session_id: string, code: number) => void) | null = null;

/** Minimal TerminalLike double recording what the store does to it. */
class FakeTerminal implements TerminalLike {
  element: HTMLElement | null = null;
  rows = 24;
  cols = 80;
  disposed = false;
  writes: string[] = [];
  dataHandler: ((data: string) => void) | null = null;
  openedInto: HTMLElement[] = [];

  open(host: HTMLElement): void {
    this.openedInto.push(host);
    this.element = host;
  }
  write(data: string): void {
    this.writes.push(data);
  }
  onData(handler: (data: string) => void): { dispose(): void } {
    this.dataHandler = handler;
    return {
      dispose: () => {
        if (this.dataHandler === handler) this.dataHandler = null;
      },
    };
  }
  focus(): void {}
  dispose(): void {
    this.disposed = true;
  }
  /** Test helper: emulate the user typing. */
  type(data: string): void {
    this.dataHandler?.(data);
  }
}

/** Factory producing FakeTerminals (records everything it created). */
function makeFactory(): {
  factory: TerminalFactory;
  created: FakeTerminal[];
} {
  const created: FakeTerminal[] = [];
  const factory: TerminalFactory = () => {
    const term = new FakeTerminal();
    created.push(term);
    return { term, fit: () => ({ cols: term.cols, rows: term.rows }), setTheme: vi.fn() };
  };
  return { factory, created };
}

/** Drains the promise chain so `pty_create` resolutions land. */
async function flush(): Promise<void> {
  for (let i = 0; i < 6; i++) await Promise.resolve();
}

beforeEach(() => {
  vi.clearAllMocks();
  let next = 0;
  mockPtyCreate.mockImplementation(() => Promise.resolve(`pty-${++next}`));
  mockPtyWrite.mockResolvedValue(undefined);
  mockPtyResize.mockResolvedValue(undefined);
  mockPtyKill.mockResolvedValue(undefined);
  mockOnPtyOutput.mockImplementation((cb) => {
    emitOutput = (session_id, data) => cb({ session_id, data });
    return Promise.resolve(() => {});
  });
  mockOnPtyExit.mockImplementation((cb) => {
    emitExit = (session_id, code) => cb({ session_id, code });
    return Promise.resolve(() => {});
  });
  mockIsTauri.mockReturnValue(true);
  resetRepoEvents(); // fresh module-owned pty subscriptions per test
});

describe("TerminalStore.ensure", () => {
  it("creates the session lazily (pty + factory terminal)", async () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);

    const entry = store.ensure("r1");
    expect(entry).not.toBeNull();
    expect(entry!.status).toBe("starting");
    expect(entry!.sessionId).toBeNull();
    expect(mockPtyCreate).toHaveBeenCalledTimes(1);
    expect(mockPtyCreate).toHaveBeenCalledWith("r1", 24, 80);
    expect(created).toHaveLength(1);

    await flush();
    expect(entry!.status).toBe("live");
    expect(entry!.sessionId).toBe("pty-1");
  });

  it("reuses the live session (one per repo terminal, no duplicate ptys)", async () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);

    const first = store.ensure("r1");
    await flush();
    const second = store.ensure("r1");

    expect(second).toBe(first);
    expect(mockPtyCreate).toHaveBeenCalledTimes(1);
    expect(mockPtyCreate).toHaveBeenCalledWith("r1", 24, 80);
    // A different repo gets its own session.
    store.ensure("r2");
    expect(mockPtyCreate).toHaveBeenCalledTimes(2);
  });

  it("reports unavailable and never calls the backend outside Tauri", () => {
    mockIsTauri.mockReturnValue(false);
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);

    const entry = store.ensure("r1");

    expect(entry!.status).toBe("unavailable");
    expect(mockPtyCreate).not.toHaveBeenCalled();
  });

  it("marks the session failed + toasts when pty_create rejects", async () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    mockPtyCreate.mockRejectedValueOnce(new Error("no shell"));

    const entry = store.ensure("r1");
    await flush();

    expect(entry!.status).toBe("failed");
    expect(entry!.error).toBe("no shell");
    expect(entry!.term).toBeNull();
    expect(created[0]!.disposed).toBe(true);
    expect(mockToast).toHaveBeenCalledWith(
      "Terminal failed to start: no shell",
      { kind: "error" },
    );
  });
});

describe("TerminalStore data flow", () => {
  interface Setup {
    store: TerminalStore;
    created: FakeTerminal[];
  }

  async function setup(): Promise<Setup> {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    store.ensure("r1");
    await flush();
    return { store, created };
  }

  it("routes keystrokes to pty_write once the session is live", async () => {
    const { created } = await setup();
    created[0]!.type("git status");
    expect(mockPtyWrite).toHaveBeenCalledTimes(1);
    expect(mockPtyWrite).toHaveBeenCalledWith("pty-1", "git status");
  });

  it("drops keystrokes while the pty is still starting", () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    store.ensure("r1");
    created[0]!.type("early");
    expect(mockPtyWrite).not.toHaveBeenCalled();
  });

  it("routes pty-output events to the owning terminal by session id", async () => {
    const { created } = await setup();
    emitOutput!("pty-1", "hello\r\n");
    expect(created[0]!.writes).toEqual(["hello\r\n"]);
    // Unknown session ids are ignored (never throw).
    expect(() => emitOutput!("bogus", "x")).not.toThrow();
    expect(created[0]!.writes).toEqual(["hello\r\n"]);
  });

  it("exit: toasts, cleans up, does not auto-recreate; next ensure starts fresh", async () => {
    const { store, created } = await setup();
    const entry = store.session("r1")!;

    emitExit!("pty-1", 0);
    await flush();

    expect(mockToast).toHaveBeenCalledWith("Terminal exited (code 0)");
    expect(mockPtyKill).not.toHaveBeenCalled(); // it died on its own
    expect(created[0]!.disposed).toBe(true);
    expect(entry.status).toBe("exited");
    expect(entry.term).toBeNull();

    // No auto-recreate: the store stays exited until the user asks again.
    expect(store.session("r1")!.status).toBe("exited");

    // Reopening creates a NEW session.
    const again = store.ensure("r1");
    await flush();
    expect(again!.sessionId).toBe("pty-2");
    expect(again!.status).toBe("live");
  });

  it("reports the exit code in the toast", async () => {
    await setup();
    emitExit!("pty-1", 0);
    expect(mockToast).toHaveBeenCalledWith("Terminal exited (code 0)");
  });

  it("ignores exit events for sessions that were replaced meanwhile", async () => {
    const { store, created } = await setup();
    store.restart("r1");
    await flush(); // new pty-2 is live now

    emitExit!("pty-1", 137); // stale event for the killed first pty

    expect(store.session("r1")!.status).toBe("live");
    expect(store.session("r1")!.sessionId).toBe("pty-2");
    expect(created[1]!.disposed).toBe(false);
  });
});

describe("TerminalStore kill / restart / dispose", () => {
  it("kill: kills the pty and marks the session exited", async () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    store.ensure("r1");
    await flush();

    store.kill("r1");

    expect(mockPtyKill).toHaveBeenCalledTimes(1);
    expect(mockPtyKill).toHaveBeenCalledWith("pty-1");
    expect(created[0]!.disposed).toBe(true);
    expect(store.session("r1")!.status).toBe("exited");
    // Output for the dead session is dropped.
    emitOutput!("pty-1", "after kill");
    expect(created[0]!.writes).toEqual([]);
  });

  it("kill for an unknown repo is a no-op", () => {
    const store = new TerminalStore();
    expect(() => store.kill("nope")).not.toThrow();
    expect(mockPtyKill).not.toHaveBeenCalled();
  });

  it("restart: kills the old pty and creates a fresh session", async () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    store.ensure("r1");
    await flush();

    const restarted = store.restart("r1");
    await flush();

    expect(mockPtyKill).toHaveBeenCalledTimes(1);
    expect(mockPtyKill).toHaveBeenCalledWith("pty-1");
    expect(created).toHaveLength(2);
    expect(created[0]!.disposed).toBe(true);
    expect(restarted!.sessionId).toBe("pty-2");
    expect(restarted!.status).toBe("live");
    expect(restarted!.term).toBe(created[1]);
  });

  it("dispose (repo tab closed): kills the pty and forgets the repo", async () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);
    store.ensure("r1");
    await flush();

    store.dispose("r1");

    expect(mockPtyKill).toHaveBeenCalledTimes(1);
    expect(mockPtyKill).toHaveBeenCalledWith("pty-1");
    expect(store.session("r1")).toBeNull();
    // A later mount starts from scratch.
    store.ensure("r1");
    expect(mockPtyCreate).toHaveBeenCalledTimes(2);
  });

  it("dispose without a session never touches the backend", () => {
    const store = new TerminalStore();
    store.dispose("ghost");
    expect(mockPtyKill).not.toHaveBeenCalled();
  });

  it("a torn-down session's late pty_create result is killed as an orphan", async () => {
    let release: ((id: string) => void) | null = null;
    mockPtyCreate.mockImplementation(
      () =>
        new Promise<string>((resolve) => {
          release = resolve;
        }),
    );
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);
    store.ensure("r1");
    store.dispose("r1"); // before the create resolves
    release!("pty-late");
    await flush();

    expect(mockPtyKill).toHaveBeenCalledTimes(1);
    expect(mockPtyKill).toHaveBeenCalledWith("pty-late");
    expect(store.session("r1")).toBeNull();
  });
});

describe("TerminalStore multi-session (terminal tabs, item 59)", () => {
  it("sessionKey round-trips through splitSessionKey", () => {
    expect(splitSessionKey(sessionKey("repo-1", "t2"))).toEqual({
      repoId: "repo-1",
      terminalId: "t2",
    });
    expect(splitSessionKey("repo-1::t1")).toEqual({ repoId: "repo-1", terminalId: "t1" });
    // Repo ids may themselves contain "::" — the split takes the LAST one.
    expect(splitSessionKey("a::b::t3")).toEqual({ repoId: "a::b", terminalId: "t3" });
    expect(splitSessionKey("no-separator")).toBeNull();
    expect(splitSessionKey("::t1")).toBeNull();
  });

  it("ensure allocates the first terminal id and tracks it as active", () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);

    store.ensure("r1");

    expect(store.list("r1")).toEqual(["t1"]);
    expect(store.activeId("r1")).toBe("t1");
    expect(store.session("r1")!.terminalId).toBe("t1");
  });

  it("create opens a second tab: own pty, tab order grows, new tab is active", async () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    store.ensure("r1");
    await flush();

    const second = store.create("r1");
    await flush();

    expect(second).not.toBeNull();
    expect(second!.terminalId).toBe("t2");
    expect(store.list("r1")).toEqual(["t1", "t2"]);
    expect(store.activeId("r1")).toBe("t2");
    expect(store.session("r1")).toBe(second);
    expect(mockPtyCreate).toHaveBeenCalledTimes(2);
    expect(created).toHaveLength(2);
    // Session records are keyed by the composite key, not the repo id.
    expect(store.sessionById("r1", "t1")!.sessionId).toBe("pty-1");
    expect(store.sessionById("r1", "t2")!.sessionId).toBe("pty-2");
  });

  it("create returns null (and adds nothing) at the per-repo cap", () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);
    for (let i = 0; i < MAX_TERMINALS_PER_REPO; i++) {
      if (i === 0) store.ensure("r1");
      else store.create("r1");
    }
    expect(store.list("r1")).toHaveLength(MAX_TERMINALS_PER_REPO);
    expect(mockPtyCreate).toHaveBeenCalledTimes(MAX_TERMINALS_PER_REPO);

    expect(store.create("r1")).toBeNull();
    expect(store.list("r1")).toHaveLength(MAX_TERMINALS_PER_REPO);
    expect(mockPtyCreate).toHaveBeenCalledTimes(MAX_TERMINALS_PER_REPO);
  });

  it("setActive switches the focused tab without creating sessions", async () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);
    store.ensure("r1");
    store.create("r1");
    await flush();
    mockPtyCreate.mockClear();

    store.setActive("r1", "t1");

    expect(store.activeId("r1")).toBe("t1");
    expect(store.session("r1")!.terminalId).toBe("t1");
    expect(mockPtyCreate).not.toHaveBeenCalled();
    // Unknown ids are ignored.
    store.setActive("r1", "t9");
    expect(store.activeId("r1")).toBe("t1");
  });

  it("pty-output routes to the owning terminal only", async () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    store.ensure("r1");
    await flush();
    store.create("r1");
    await flush();

    emitOutput!("pty-1", "one\r\n");
    emitOutput!("pty-2", "two\r\n");

    expect(created[0]!.writes).toEqual(["one\r\n"]);
    expect(created[1]!.writes).toEqual(["two\r\n"]);
  });

  it("exit marks only that session exited; the other tabs stay live", async () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);
    store.ensure("r1");
    await flush();
    store.create("r1");
    await flush();

    emitExit!("pty-1", 0);
    await flush();

    expect(store.sessionById("r1", "t1")!.status).toBe("exited");
    expect(store.sessionById("r1", "t2")!.status).toBe("live");
    expect(mockPtyKill).not.toHaveBeenCalled(); // it died on its own
  });

  it("close removes one tab: kills its pty, re-focuses the left neighbor", async () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    store.ensure("r1");
    await flush();
    store.create("r1");
    await flush();
    store.create("r1"); // t3, active
    await flush();

    store.close("r1", "t3");

    expect(mockPtyKill).toHaveBeenCalledWith("pty-3");
    expect(created[2]!.disposed).toBe(true);
    expect(store.list("r1")).toEqual(["t1", "t2"]);
    expect(store.activeId("r1")).toBe("t2");
    expect(store.sessionById("r1", "t3")).toBeNull();
  });

  it("close of a background tab keeps the active tab focused", async () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);
    store.ensure("r1");
    await flush();
    store.create("r1"); // t2 active
    await flush();

    store.close("r1", "t1");

    expect(store.list("r1")).toEqual(["t2"]);
    expect(store.activeId("r1")).toBe("t2");
  });

  it("close of the last terminal clears the repo's tab state", async () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);
    store.ensure("r1");
    await flush();

    store.close("r1", "t1");

    expect(store.list("r1")).toEqual([]);
    expect(store.activeId("r1")).toBeNull();
    expect(store.session("r1")).toBeNull();
    // The next ensure starts a fresh first terminal.
    store.ensure("r1");
    expect(store.list("r1")).toEqual(["t1"]);
  });

  it("close for an unknown terminal id is a no-op", async () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);
    store.ensure("r1");
    await flush();

    expect(() => store.close("r1", "t9")).not.toThrow();
    expect(() => store.close("ghost", "t1")).not.toThrow();
    expect(store.list("r1")).toEqual(["t1"]);
    expect(mockPtyKill).not.toHaveBeenCalled();
  });

  it("dispose kills and forgets ALL of the repo's terminals (repo tab closed)", async () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    store.ensure("r1");
    await flush();
    store.create("r1");
    await flush();
    store.create("r1");
    await flush();

    store.dispose("r1");

    expect(mockPtyKill).toHaveBeenCalledTimes(3);
    expect(mockPtyKill).toHaveBeenCalledWith("pty-1");
    expect(mockPtyKill).toHaveBeenCalledWith("pty-2");
    expect(mockPtyKill).toHaveBeenCalledWith("pty-3");
    for (const term of created) expect(term.disposed).toBe(true);
    expect(store.list("r1")).toEqual([]);
    expect(store.activeId("r1")).toBeNull();
    expect(Object.keys(store.sessions)).toHaveLength(0);
    // A later mount starts from scratch.
    store.ensure("r1");
    expect(store.list("r1")).toEqual(["t1"]);
  });

  it("writeInput writes command + Enter to the ACTIVE session's pty", async () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);
    store.ensure("r1");
    await flush();
    store.create("r1"); // t2 is active now
    await flush();

    expect(store.writeInput("r1", "git status\r")).toBe(true);
    expect(mockPtyWrite).toHaveBeenLastCalledWith("pty-2", "git status\r");
  });

  it("writeInput drops silently (false) without a live pty", async () => {
    const store = new TerminalStore();
    store.setFactory(makeFactory().factory);

    expect(store.writeInput("ghost", "git status\r")).toBe(false);

    store.ensure("r1"); // still starting — no session id yet
    expect(store.writeInput("r1", "git status\r")).toBe(false);
    expect(mockPtyWrite).not.toHaveBeenCalled();
  });

  it("keystrokes land in their own terminal's shell (per-tab onData closure)", async () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    store.ensure("r1");
    await flush();
    store.create("r1");
    await flush();

    created[0]!.type("tab-one");
    created[1]!.type("tab-two");

    expect(mockPtyWrite).toHaveBeenNthCalledWith(1, "pty-1", "tab-one");
    expect(mockPtyWrite).toHaveBeenNthCalledWith(2, "pty-2", "tab-two");
  });

  it("restart replaces only the ACTIVE terminal", async () => {
    const store = new TerminalStore();
    const { factory, created } = makeFactory();
    store.setFactory(factory);
    store.ensure("r1");
    await flush();
    store.create("r1"); // t2 active
    await flush();

    const restarted = store.restart("r1");
    await flush();

    expect(restarted!.terminalId).toBe("t2");
    expect(restarted!.sessionId).toBe("pty-3");
    expect(store.list("r1")).toEqual(["t1", "t2"]);
    // t1's session was untouched.
    expect(store.sessionById("r1", "t1")!.sessionId).toBe("pty-1");
    expect(created[0]!.disposed).toBe(false);
    expect(created[1]!.disposed).toBe(true);
  });
});

describe("TerminalStore.resize", () => {
  it("debounces to a single pty_resize with the latest dims", async () => {
    vi.useFakeTimers();
    try {
      const store = new TerminalStore();
      store.setFactory(makeFactory().factory);
      store.ensure("r1");
      await vi.waitFor(() => {
        expect(store.session("r1")!.status).toBe("live");
      });

      store.resize("r1", 100, 30);
      store.resize("r1", 120, 34);
      expect(mockPtyResize).not.toHaveBeenCalled();

      vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS);
      expect(mockPtyResize).toHaveBeenCalledTimes(1);
    expect(mockPtyResize).toHaveBeenCalledWith("pty-1", 34, 120);
    } finally {
      vi.useRealTimers();
    }
  });

  it("skips repos without a live pty and cancels pending resizes on teardown", async () => {
    vi.useFakeTimers();
    try {
      const store = new TerminalStore();
      store.setFactory(makeFactory().factory);
      store.resize("ghost", 80, 24);
      vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS * 2);
      expect(mockPtyResize).not.toHaveBeenCalled();

      store.ensure("r1");
      await vi.waitFor(() => {
        expect(store.session("r1")!.status).toBe("live");
      });
      store.resize("r1", 80, 24);
      store.dispose("r1");
      vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS * 2);
      expect(mockPtyResize).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});
