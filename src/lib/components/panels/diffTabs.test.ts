/**
 * Unit tests for the center editor's multi-diff tab algebra (diffTabs.ts):
 * upsert (open = append or refresh-in-place), neighbor activation on
 * close, active-tab resolution, the multi-path stream page filter, and
 * the per-tab file filter.
 */

import { describe, expect, it } from "vitest";
import type { ChangeKind, FileDiff, StatusEntry } from "$lib/ipc/types";
import {
  activeDiffTab,
  diffPageForOpenPaths,
  fileForDiffTab,
  neighborDiffPath,
  upsertDiffTab,
  withoutDiffTab,
  type DiffTab,
} from "./diffTabs";

function entry(
  path: string,
  index: ChangeKind = "modified",
  worktree: ChangeKind = "modified",
  old_path: string | null = null,
): StatusEntry {
  return { path, old_path, index, worktree };
}

function tab(path: string, scope: DiffTab["scope"] = "unstaged"): DiffTab {
  return { entry: entry(path), scope };
}

function diff(path: string, old_path: string | null = null): FileDiff {
  return {
    path,
    old_path,
    binary: false,
    is_image: false,
    additions: 1,
    deletions: 0,
    hunks: [],
  };
}

describe("upsertDiffTab", () => {
  it("appends a new tab for an unopened path", () => {
    const tabs = upsertDiffTab([tab("a.ts")], entry("b.ts"), "staged");
    expect(tabs.map((t) => t.entry.path)).toEqual(["a.ts", "b.ts"]);
    expect(tabs[1]!.scope).toBe("staged");
  });

  it("refreshes an open tab in place (position kept, scope updated)", () => {
    const tabs = upsertDiffTab(
      [tab("a.ts"), tab("b.ts", "unstaged")],
      entry("b.ts", "added"),
      "staged",
    );
    expect(tabs.map((t) => t.entry.path)).toEqual(["a.ts", "b.ts"]);
    expect(tabs[1]!.scope).toBe("staged");
    expect(tabs[1]!.entry.index).toBe("added");
    expect(tabs[1]!.entry.worktree).toBe("modified");
  });

  it("appends to an empty list", () => {
    expect(upsertDiffTab([], entry("a.ts"), "unstaged")).toHaveLength(1);
  });

  it("does not mutate the input list", () => {
    const original = [tab("a.ts")];
    upsertDiffTab(original, entry("b.ts"), "unstaged");
    expect(original).toHaveLength(1);
  });
});

describe("neighborDiffPath", () => {
  const three = [tab("a.ts"), tab("b.ts"), tab("c.ts")];

  it("activates the right neighbor when the active tab closes", () => {
    expect(neighborDiffPath(three, "b.ts", "b.ts")).toBe("c.ts");
  });

  it("clamps to the new last tab when the active was the last", () => {
    expect(neighborDiffPath(three, "c.ts", "c.ts")).toBe("b.ts");
  });

  it("keeps the active tab when another tab closed", () => {
    expect(neighborDiffPath(three, "a.ts", "c.ts")).toBe("a.ts");
  });

  it("returns null when the last tab closed", () => {
    expect(neighborDiffPath([tab("a.ts")], "a.ts", "a.ts")).toBeNull();
  });
});

describe("withoutDiffTab", () => {
  it("removes only the target path", () => {
    const rest = withoutDiffTab([tab("a.ts"), tab("b.ts")], "a.ts");
    expect(rest.map((t) => t.entry.path)).toEqual(["b.ts"]);
  });

  it("is a no-op for an absent path", () => {
    expect(withoutDiffTab([tab("a.ts")], "x.ts")).toHaveLength(1);
  });
});

describe("activeDiffTab", () => {
  it("finds the active path", () => {
    expect(activeDiffTab([tab("a.ts"), tab("b.ts")], "b.ts")?.entry.path).toBe("b.ts");
  });

  it("falls back to the first tab (activePath drifted stale)", () => {
    expect(activeDiffTab([tab("a.ts"), tab("b.ts")], "gone.ts")?.entry.path).toBe("a.ts");
  });

  it("returns null for an empty list", () => {
    expect(activeDiffTab([], "a.ts")).toBeNull();
  });
});

describe("diffPageForOpenPaths", () => {
  it("keeps page files matching any open path", () => {
    const page = [diff("a.ts"), diff("other.ts"), diff("c.ts")];
    const kept = diffPageForOpenPaths(page, ["a.ts", "c.ts"]);
    expect(kept.map((f) => f.path)).toEqual(["a.ts", "c.ts"]);
  });

  it("matches renames by old path", () => {
    const page = [diff("new.ts", "old.ts")];
    expect(diffPageForOpenPaths(page, ["old.ts"])).toHaveLength(1);
    expect(diffPageForOpenPaths(page, ["new.ts"])).toHaveLength(1);
    expect(diffPageForOpenPaths(page, ["neither.ts"])).toHaveLength(0);
  });
});

describe("fileForDiffTab", () => {
  it("filters the streamed files down to one tab's own diff", () => {
    const files = [diff("a.ts"), diff("b.ts"), diff("renamed.ts", "a.ts")];
    expect(fileForDiffTab(files, "a.ts").map((f) => f.path)).toEqual([
      "a.ts",
      "renamed.ts",
    ]);
    expect(fileForDiffTab(files, "b.ts").map((f) => f.path)).toEqual(["b.ts"]);
  });
});
