/**
 * Unit tests for the terminal snippet store (`snippets.svelte.ts`): the
 * built-in list, per-root user slots and the versioned-envelope persistence.
 * Storage is an in-memory double — no localStorage is touched.
 */

import { beforeEach, describe, expect, it } from "vitest";
import {
  BUILTIN_SNIPPETS,
  emptySnippet,
  isFilled,
  parseSnippets,
  SnippetStore,
  terminalSnippets as terminalSingleton,
  TERMINAL_SNIPPETS_STORAGE_KEY,
  USER_SNIPPET_SLOTS,
} from "./snippets.svelte";
import { readEnvelope } from "$lib/state/persist";

/** In-memory storage double recording raw writes for envelope assertions. */
function memStorage(): {
  getItem: (key: string) => string | null;
  setItem: (key: string, value: string) => void;
  data: Map<string, string>;
} {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => void data.set(key, value),
  };
}

beforeEach(() => {
  // Isolate the module singleton per test (storage is injectable there).
  terminalSingleton.reset();
});

describe("snippets catalog", () => {
  it("ships the three built-in quick commands", () => {
    expect(BUILTIN_SNIPPETS.map((snippet) => snippet.command)).toEqual([
      "git status",
      "git log --oneline -10",
      "git fetch --all --prune",
    ]);
  });

  it("exposes exactly two user slots", () => {
    expect(USER_SNIPPET_SLOTS).toBe(2);
  });

  it("isFilled distinguishes a set slot from an empty one", () => {
    expect(isFilled(emptySnippet())).toBe(false);
    expect(isFilled({ label: "", command: "  " })).toBe(false);
    expect(isFilled({ label: "build", command: "npm run build" })).toBe(true);
  });
});

describe("SnippetStore.userFor", () => {
  it("pads unknown roots to the slot count with empty slots", () => {
    const store = new SnippetStore(null);
    expect(store.userFor("/repo/a")).toHaveLength(USER_SNIPPET_SLOTS);
    expect(store.userFor("/repo/a")).toEqual([emptySnippet(), emptySnippet()]);
  });

  it("setUser writes one slot per root and pads the rest", () => {
    const store = new SnippetStore(null);
    store.setUser("/repo/a", 0, { label: "build", command: "npm run build" });

    expect(store.userFor("/repo/a")).toEqual([
      { label: "build", command: "npm run build" },
      emptySnippet(),
    ]);
    // Other roots are untouched.
    expect(store.userFor("/repo/b")).toEqual([emptySnippet(), emptySnippet()]);
  });

  it("ignores out-of-range slot indexes", () => {
    const store = new SnippetStore(null);
    store.setUser("/repo/a", 5, { label: "x", command: "y" });
    expect(store.userFor("/repo/a")).toEqual([emptySnippet(), emptySnippet()]);
  });

  it("re-reads persisted slots after forget (memory-only eviction)", () => {
    const storage = memStorage();
    const store = new SnippetStore(storage);
    store.setUser("/repo/a", 1, { label: "lint", command: "npm run lint" });
    expect(storage.data.has(TERMINAL_SNIPPETS_STORAGE_KEY)).toBe(true);

    store.forget("/repo/a");
    // Memory dropped…
    expect(store.user["/repo/a"]).toBeUndefined();
    // …but the persisted preference survives a re-open (re-hydration).
    expect(store.userFor("/repo/a")).toEqual([
      emptySnippet(),
      { label: "lint", command: "npm run lint" },
    ]);
  });
});

describe("snippet persistence (versioned envelope)", () => {
  it("writes the { v, data } envelope keyed per root", () => {
    const storage = memStorage();
    const store = new SnippetStore(storage);
    store.setUser("/repo/a", 0, { label: "build", command: "npm run build" });

    const entry = readEnvelope(storage, TERMINAL_SNIPPETS_STORAGE_KEY);
    expect(entry).not.toBeNull();
    expect(entry!.version).toBe(1);
    // Slots persist fixed-width (empty slots included) so slot indexes are
    // stable across re-hydration — slot 1 stays slot 1.
    expect(entry!.data).toEqual({
      "/repo/a": [
        { label: "build", command: "npm run build" },
        { label: "", command: "" },
      ],
    });
  });

  it("hydrates persisted slots on first access per root", () => {
    const storage = memStorage();
    const writer = new SnippetStore(storage);
    writer.setUser("/repo/a", 0, { label: "build", command: "npm run build" });
    writer.setUser("/repo/a", 1, { label: "lint", command: "npm run lint" });

    const reader = new SnippetStore(storage);
    expect(reader.userFor("/repo/a")).toEqual([
      { label: "build", command: "npm run build" },
      { label: "lint", command: "npm run lint" },
    ]);
  });

  it("a future/unknown envelope version resolves to the defaults", () => {
    const storage = memStorage();
    storage.data.set(
      TERMINAL_SNIPPETS_STORAGE_KEY,
      JSON.stringify({ v: 99, data: { "/repo/a": [{ label: "x", command: "y" }] } }),
    );
    const store = new SnippetStore(storage);
    expect(store.userFor("/repo/a")).toEqual([emptySnippet(), emptySnippet()]);
  });
});

describe("parseSnippets", () => {
  it("drops malformed payloads and entries, caps slots", () => {
    expect(parseSnippets("nope")).toEqual({});
    expect(parseSnippets(null)).toEqual({});
    expect(parseSnippets({ "": [{ label: "x", command: "y" }] })).toEqual({});
    expect(
      parseSnippets({
        "/repo/a": [{ label: "ok", command: "ok" }, "junk", 42, { label: 1, command: "y" }],
      }),
    ).toEqual({ "/repo/a": [{ label: "ok", command: "ok" }] });
    // More slots than the cap: extras are dropped.
    expect(
      parseSnippets({
        "/repo/a": [
          { label: "1", command: "1" },
          { label: "2", command: "2" },
          { label: "3", command: "3" },
        ],
      }),
    ).toEqual({
      "/repo/a": [
        { label: "1", command: "1" },
        { label: "2", command: "2" },
      ],
    });
  });
});
