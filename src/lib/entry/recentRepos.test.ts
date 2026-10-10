import { describe, expect, it } from "vitest";
import {
  RecentRepoStore,
  type StorageLike,
} from "$lib/entry/recentRepos";

const STORAGE_KEY = "mygitui.recent-repos";

/** In-memory Storage stub shared across store instances to simulate reloads. */
function memoryStorage(initial: Record<string, string> = {}): StorageLike {
  const data = new Map<string, string>(Object.entries(initial));
  return {
    getItem: (key) => (data.has(key) ? data.get(key)! : null),
    setItem: (key, value) => {
      data.set(key, value);
    },
  };
}

/** Deterministic clock: `now()` reads the current time, `tick()` advances it. */
function fakeClock(start = 1_000): {
  now: () => number;
  tick: (ms?: number) => void;
} {
  let t = start;
  return {
    now: () => t,
    tick: (ms = 100) => {
      t += ms;
    },
  };
}

function paths(entries: { path: string }[]): string[] {
  return entries.map((e) => e.path);
}

describe("RecentRepoStore", () => {
  it("add() stores a new entry unpinned with a lastOpened timestamp", () => {
    const clock = fakeClock();
    const store = new RecentRepoStore(memoryStorage(), clock.now);

    const openedAt = clock.now();
    store.add("C:/repos/alpha");

    expect(store.list()).toEqual([
      { path: "C:/repos/alpha", pinned: false, lastOpened: openedAt },
    ]);
  });

  it("add() ignores empty paths", () => {
    const store = new RecentRepoStore(memoryStorage());
    store.add("");
    expect(store.list()).toEqual([]);
  });

  it("add() dedupes by path instead of creating duplicates", () => {
    const clock = fakeClock();
    const store = new RecentRepoStore(memoryStorage(), clock.now);

    store.add("C:/repos/alpha");
    const firstOpened = store.list()[0].lastOpened;
    clock.tick();
    store.add("C:/repos/beta");
    clock.tick();
    store.add("C:/repos/alpha"); // re-open: refresh, not duplicate

    const list = store.list();
    expect(paths(list)).toEqual(["C:/repos/alpha", "C:/repos/beta"]);
    expect(list[0].lastOpened).toBeGreaterThan(firstOpened);
    expect(list[0].lastOpened).toBe(clock.now());
  });

  it("add() dedupes separator and case spellings of one Windows folder", () => {
    const clock = fakeClock();
    const store = new RecentRepoStore(memoryStorage(), clock.now);

    store.add("C:/repos/Alpha");
    const firstOpened = store.list()[0].lastOpened;
    clock.tick();
    store.add("C:\\repos\\alpha\\"); // same folder, dialog spelling

    const list = store.list();
    expect(list).toHaveLength(1);
    expect(list[0].path).toBe("C:/repos/Alpha"); // first spelling is kept
    expect(list[0].lastOpened).toBe(clock.now());
    expect(list[0].lastOpened).toBeGreaterThan(firstOpened);
  });

  it("add() keeps POSIX paths case-sensitive", () => {
    const clock = fakeClock();
    const store = new RecentRepoStore(memoryStorage(), clock.now);

    store.add("/repos/Alpha");
    clock.tick();
    store.add("/repos/alpha");

    expect(paths(store.list())).toEqual(["/repos/alpha", "/repos/Alpha"]);
  });

  it("add() collapses \\\\?\\ canonical spellings", () => {
    const store = new RecentRepoStore(memoryStorage(), fakeClock().now);

    store.add("C:/repos/alpha");
    store.add("\\\\?\\C:\\repos\\alpha");

    expect(store.list()).toHaveLength(1);
  });

  it("remove() and togglePin() match canonical spellings", () => {
    const store = new RecentRepoStore(memoryStorage(), fakeClock().now);
    store.add("C:/repos/alpha");

    store.togglePin("C:\\repos\\alpha");
    expect(store.list()[0].pinned).toBe(true);

    store.remove("C:\\repos\\alpha\\");
    expect(store.list()).toEqual([]);
  });

  it("parse merges persisted separator duplicates, keeping newest spelling and pins", () => {
    const polluted = memoryStorage({
      [STORAGE_KEY]: JSON.stringify([
        { path: "C:/repos/alpha", pinned: true, lastOpened: 100 },
        { path: "C:\\repos\\alpha", pinned: false, lastOpened: 200 },
      ]),
    });
    const store = new RecentRepoStore(polluted, fakeClock().now);

    expect(store.list()).toEqual([
      { path: "C:\\repos\\alpha", pinned: true, lastOpened: 200 },
    ]);
  });

  it("list() orders pinned entries first, then most recently opened", () => {
    const clock = fakeClock();
    const store = new RecentRepoStore(memoryStorage(), clock.now);

    store.add("C:/repos/a");
    clock.tick();
    store.add("C:/repos/b");
    clock.tick();
    store.add("C:/repos/c"); // recency order: c, b, a
    store.togglePin("C:/repos/a"); // pinned a jumps to front

    expect(paths(store.list())).toEqual([
      "C:/repos/a",
      "C:/repos/c",
      "C:/repos/b",
    ]);
  });

  it("list() returns a detached copy", () => {
    const store = new RecentRepoStore(memoryStorage());
    store.add("C:/repos/a");

    const snapshot = store.list();
    snapshot.pop();

    expect(store.list()).toHaveLength(1);
  });

  it("remove() drops the entry", () => {
    const store = new RecentRepoStore(memoryStorage());
    store.add("C:/repos/a");
    store.add("C:/repos/b");

    store.remove("C:/repos/a");

    expect(paths(store.list())).toEqual(["C:/repos/b"]);
    store.remove("C:/repos/missing"); // no-op, no throw
    expect(paths(store.list())).toEqual(["C:/repos/b"]);
  });

  it("togglePin() flips pin state both ways", () => {
    const store = new RecentRepoStore(memoryStorage());
    store.add("C:/repos/a");

    store.togglePin("C:/repos/a");
    expect(store.list()[0].pinned).toBe(true);
    store.togglePin("C:/repos/a");
    expect(store.list()[0].pinned).toBe(false);
    store.togglePin("C:/repos/missing"); // no-op, no throw
    expect(store.list()).toHaveLength(1);
  });

  it("persists to the mygitui.recent-repos key and survives reload", () => {
    const storage = memoryStorage();
    const clock = fakeClock();

    const writer = new RecentRepoStore(storage, clock.now);
    writer.add("C:/repos/alpha");
    clock.tick();
    writer.add("C:/repos/beta");
    writer.togglePin("C:/repos/beta");

    expect(storage.getItem(STORAGE_KEY)).toBe(
      JSON.stringify([
        { path: "C:/repos/alpha", pinned: false, lastOpened: 1_000 },
        { path: "C:/repos/beta", pinned: true, lastOpened: 1_100 },
      ]),
    );

    const reader = new RecentRepoStore(storage, clock.now);
    expect(paths(reader.list())).toEqual(["C:/repos/beta", "C:/repos/alpha"]);
    expect(reader.list()[0].pinned).toBe(true);
  });

  it("discards malformed persisted data instead of throwing", () => {
    const corrupt = memoryStorage({ [STORAGE_KEY]: '{"not":"an array"}' });
    expect(new RecentRepoStore(corrupt).list()).toEqual([]);

    const partial = memoryStorage({
      [STORAGE_KEY]: JSON.stringify([
        { path: "C:/repos/ok", pinned: false, lastOpened: 42 },
        { path: 7, pinned: "yes", lastOpened: "x" }, // invalid shape
        null,
        "junk",
        { path: "C:/repos/dupe", pinned: false, lastOpened: 1 },
        { path: "C:/repos/dupe", pinned: false, lastOpened: 2 }, // duplicate
      ]),
    });
    const store = new RecentRepoStore(partial);
    expect(paths(store.list())).toEqual(["C:/repos/ok", "C:/repos/dupe"]);
    expect(store.list()[1].lastOpened).toBe(2); // merged duplicate keeps newest
  });

  it("keeps working when no storage is available", () => {
    const store = new RecentRepoStore(null, fakeClock().now);
    store.add("C:/repos/a");
    store.togglePin("C:/repos/a");

    expect(paths(store.list())).toEqual(["C:/repos/a"]);
    expect(store.list()[0].pinned).toBe(true);
  });

  it("keeps working when storage setItem throws", () => {
    const failing: StorageLike = {
      getItem: () => null,
      setItem: () => {
        throw new Error("quota exceeded");
      },
    };
    const store = new RecentRepoStore(failing, fakeClock().now);
    expect(() => store.add("C:/repos/a")).not.toThrow();
    expect(paths(store.list())).toEqual(["C:/repos/a"]);
  });
});
