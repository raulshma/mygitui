/**
 * Unit tests for the commit bookmark store. Storage is a map-backed fake —
 * localStorage is never touched.
 */

import { describe, expect, it } from "vitest";
import {
  BookmarkStore,
  bookmarksKey,
  type StorageLike,
} from "./bookmarks.svelte";

/** Map-backed storage with a dump helper (mirrors the actions store tests). */
function fakeStorage(): StorageLike & { dump: (key: string) => string } {
  const map = new Map<string, string>();
  return {
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => void map.set(key, value),
    dump: (key) => map.get(key) ?? "",
  };
}

describe("BookmarkStore", () => {
  it("adds, lists and defaults labels to the short sha", () => {
    const store = new BookmarkStore(fakeStorage());
    const b = store.add("root-a", "a".repeat(40));
    expect(b).toEqual({ sha: "a".repeat(40), label: "aaaaaaa" });
    expect(store.list("root-a")).toHaveLength(1);
    expect(store.has("root-a", "a".repeat(40))).toBe(true);
    expect(store.shas("root-a")).toEqual(new Set(["a".repeat(40)]));
    expect(store.list("root-b")).toEqual([]);
  });

  it("rejects blank shas and duplicates", () => {
    const store = new BookmarkStore(fakeStorage());
    expect(store.add("r", "  ")).toBeNull();
    store.add("r", "sha-1");
    expect(store.add("r", "sha-1")).toBeNull();
    expect(store.list("r")).toHaveLength(1);
  });

  it("removes and toggles", () => {
    const store = new BookmarkStore(fakeStorage());
    store.add("r", "sha-1", "the base");
    store.add("r", "sha-2");

    expect(store.remove("r", "missing")).toBe(false);
    expect(store.remove("r", "sha-1")).toBe(true);
    expect(store.list("r").map((b) => b.sha)).toEqual(["sha-2"]);

    // toggle adds when absent …
    expect(store.toggle("r", "sha-3", "wip")).toBe(true);
    expect(store.has("r", "sha-3")).toBe(true);
    // … and removes when present.
    expect(store.toggle("r", "sha-3")).toBe(false);
    expect(store.has("r", "sha-3")).toBe(false);
  });

  it("persists per root under mygitui.bookmarks.<root>", () => {
    const storage = fakeStorage();
    const store = new BookmarkStore(storage);
    store.add("root-a", "sha-1", "label one");
    store.add("root-b", "sha-2");
    expect(storage.dump(bookmarksKey("root-a"))).toBe(
      JSON.stringify([{ sha: "sha-1", label: "label one" }]),
    );
    expect(storage.dump(bookmarksKey("root-b"))).toContain("sha-2");
  });

  it("hydrates lazily per root from storage (ensure)", () => {
    const storage = fakeStorage();
    storage.setItem(
      bookmarksKey("root-a"),
      JSON.stringify([{ sha: "sha-1", label: "" }, { sha: "sha-1" }, "junk"]),
    );
    const store = new BookmarkStore(storage);
    expect(store.list("root-a")).toEqual([]); // not yet hydrated

    store.ensure("root-a");
    // Blank labels fall back to the short sha; malformed entries dropped;
    // duplicates removed.
    expect(store.list("root-a")).toEqual([{ sha: "sha-1", label: "sha-1" }]);
    // ensure is idempotent (does not clobber in-memory edits).
    store.add("root-a", "sha-9");
    store.ensure("root-a");
    expect(store.list("root-a").map((b) => b.sha)).toEqual(["sha-1", "sha-9"]);
  });

  it("tolerates corrupt or non-array payloads", () => {
    const storage = fakeStorage();
    storage.setItem(bookmarksKey("r"), "{not json");
    storage.setItem(bookmarksKey("q"), '{"sha": "nope"}');
    const store = new BookmarkStore(storage);
    store.ensure("r");
    store.ensure("q");
    expect(store.list("r")).toEqual([]);
    expect(store.list("q")).toEqual([]);
  });

  it("clear drops state for one root", () => {
    const store = new BookmarkStore(fakeStorage());
    store.add("r", "sha-1");
    store.clear("r");
    expect(store.list("r")).toEqual([]);
    // Re-hydration is allowed after clear (an empty re-read).
    expect(store.has("r", "sha-1")).toBe(false);
  });

  it("works without storage (in-memory only)", () => {
    const store = new BookmarkStore(null);
    store.add("r", "sha-1");
    expect(store.list("r")).toEqual([{ sha: "sha-1", label: "sha-1" }]);
  });
});
