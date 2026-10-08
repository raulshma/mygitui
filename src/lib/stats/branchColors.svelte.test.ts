/**
 * Unit tests for branch color rules: the pure pattern matcher + ref-color
 * resolution, and the per-repo rule store with an injected map-backed
 * storage (localStorage never touched).
 */

import { describe, expect, it } from "vitest";
import {
  BranchColorStore,
  branchColorForRefs,
  branchColorsKey,
  matchesBranchPattern,
  type StorageLike,
} from "./branchColors.svelte";

function fakeStorage(): StorageLike & { dump: (key: string) => string } {
  const map = new Map<string, string>();
  return {
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => void map.set(key, value),
    dump: (key) => map.get(key) ?? "",
  };
}

// ---------------------------------------------------------------------------
// matchesBranchPattern
// ---------------------------------------------------------------------------

describe("matchesBranchPattern", () => {
  it("supports exact, prefix, suffix and catch-all patterns", () => {
    // Exact.
    expect(matchesBranchPattern("main", "main")).toBe(true);
    expect(matchesBranchPattern("maint", "main")).toBe(false);
    // Prefix (trailing *).
    expect(matchesBranchPattern("feature/one", "feature/*")).toBe(true);
    expect(matchesBranchPattern("features/one", "feature/*")).toBe(false);
    // Suffix (leading *).
    expect(matchesBranchPattern("release-2-hotfix", "*-hotfix")).toBe(true);
    expect(matchesBranchPattern("hotfix-2", "*-hotfix")).toBe(false);
    // Catch-all.
    expect(matchesBranchPattern("anything", "*")).toBe(true);
  });

  it("is case-sensitive and rejects blank inputs", () => {
    expect(matchesBranchPattern("Main", "main")).toBe(false);
    expect(matchesBranchPattern("main", "  ")).toBe(false);
    expect(matchesBranchPattern("  ", "main")).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// branchColorForRefs
// ---------------------------------------------------------------------------

describe("branchColorForRefs", () => {
  const RULES = [
    { pattern: "feature/*", color: "#ff0000" },
    { pattern: "origin/release*", color: "#00ff00" },
  ];

  it("returns the first matching rule's color", () => {
    expect(branchColorForRefs(["feature/one"], RULES)).toBe("#ff0000");
    expect(
      branchColorForRefs(["origin/release-3", "v1.2.0"], RULES),
    ).toBe("#00ff00");
  });

  it("returns null when nothing matches", () => {
    expect(branchColorForRefs(["main", "v1.2.0"], RULES)).toBeNull();
    expect(branchColorForRefs([], RULES)).toBeNull();
    expect(branchColorForRefs(["main"], [])).toBeNull();
  });

  it("ignores HEAD decorations (they are pointers, not branches)", () => {
    // The backend decorates HEAD as "HEAD -> main"; a "main" rule must not
    // recolor a row whose only decoration is the HEAD pointer.
    expect(
      branchColorForRefs(["HEAD -> main"], [{ pattern: "main", color: "#f00" }]),
    ).toBeNull();
    // …but the branch decoration alongside it does match.
    expect(
      branchColorForRefs(["HEAD -> main", "main"], [
        { pattern: "main", color: "#f00" },
      ]),
    ).toBe("#f00");
  });

  it("skips rules with blank colors and honors rule order", () => {
    const rules = [
      { pattern: "main", color: "   " }, // skipped
      { pattern: "main", color: "#123456" },
    ];
    expect(branchColorForRefs(["main"], rules)).toBe("#123456");
  });
});

// ---------------------------------------------------------------------------
// BranchColorStore
// ---------------------------------------------------------------------------

describe("BranchColorStore", () => {
  it("adds, updates and removes rules with validation", () => {
    const store = new BranchColorStore(fakeStorage());
    expect(store.addRule("r", "  ", "#f00")).toBeNull(); // blank pattern
    expect(store.addRule("r", "main", " ")).toBeNull(); // blank color

    expect(store.addRule("r", "main", "#ff0000")).toEqual({
      pattern: "main",
      color: "#ff0000",
    });
    expect(store.addRule("r", "main", "#00ff00")).toBeNull(); // duplicate pattern

    store.addRule("r", "feature/*", "#00ff00");
    store.updateRule("r", 1, { color: " #0000ff " });
    expect(store.rules("r")[1]).toEqual({ pattern: "feature/*", color: "#0000ff" });
    // Blank patches keep the old value.
    store.updateRule("r", 1, { pattern: "  ", color: "" });
    expect(store.rules("r")[1]?.pattern).toBe("feature/*");

    expect(store.removeRule("r", 5)).toBe(false);
    expect(store.removeRule("r", 1)).toBe(true);
    expect(store.rules("r").map((r) => r.pattern)).toEqual(["main"]);
  });

  it("persists per root under mygitui.branchcolors.<root> and hydrates", () => {
    const storage = fakeStorage();
    const store = new BranchColorStore(storage);
    store.addRule("root-a", "feature/*", "#ff0000");
    expect(storage.dump(branchColorsKey("root-a"))).toBe(
      JSON.stringify([{ pattern: "feature/*", color: "#ff0000" }]),
    );

    const revived = new BranchColorStore(storage);
    expect(revived.rules("root-a")).toEqual([]); // lazy until ensure
    revived.ensure("root-a");
    expect(revived.rules("root-a")).toEqual([
      { pattern: "feature/*", color: "#ff0000" },
    ]);
  });

  it("tolerates corrupt payloads and works without storage", () => {
    const storage = fakeStorage();
    storage.setItem(branchColorsKey("r"), "not json");
    const store = new BranchColorStore(storage);
    store.ensure("r");
    expect(store.rules("r")).toEqual([]);

    const volatile = new BranchColorStore(null);
    expect(volatile.addRule("r", "main", "#fff")).not.toBeNull();
    expect(volatile.rules("r")).toHaveLength(1);
  });
});
