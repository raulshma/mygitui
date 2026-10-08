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
import { toast } from "$lib/toast";
import {
  RESIZE_DEBOUNCE_MS,
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
let emitExit: ((session_id: string, exit_code: number | null) => void) | null = null;

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
    emitExit = (session_id, exit_code) => cb({ session_id, exit_code });
    return Promise.resolve(() => {});
  });
  mockIsTauri.mockReturnValue(true);
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

  it("reuses the live session (one per repo, no duplicate ptys)", async () => {
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

  it("reports null exit codes in the toast", async () => {
    await setup();
    emitExit!("pty-1", null);
    expect(mockToast).toHaveBeenCalledWith("Terminal exited");
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
