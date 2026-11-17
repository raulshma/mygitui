import { describe, expect, it } from "vitest";
import type { StatusEntry } from "$lib/ipc/types";
import {
  groupStatusEntries,
  isDiscardable,
  isStageable,
  kindLetter,
  openSide,
  splitPath,
} from "./changesModel";

function entry(overrides: Partial<StatusEntry> & { path: string }): StatusEntry {
  return {
    old_path: null,
    index: "unmodified",
    worktree: "unmodified",
    ...overrides,
  };
}

describe("groupStatusEntries", () => {
  it("returns empty groups for empty input", () => {
    expect(groupStatusEntries([])).toEqual({
      staged: [],
      changed: [],
      untracked: [],
    });
  });

  it("groups staged-only entries (index changed, worktree clean)", () => {
    const groups = groupStatusEntries([
      entry({ path: "src/a.ts", index: "modified" }),
      entry({ path: "new.ts", index: "added" }),
    ]);
    expect(groups.staged.map((e) => e.path)).toEqual(["src/a.ts", "new.ts"]);
    expect(groups.changed).toEqual([]);
    expect(groups.untracked).toEqual([]);
  });

  it("groups worktree-only entries (index clean, worktree changed)", () => {
    const groups = groupStatusEntries([
      entry({ path: "src/b.ts", worktree: "modified" }),
      entry({ path: "gone.ts", worktree: "deleted" }),
    ]);
    expect(groups.staged).toEqual([]);
    expect(groups.changed.map((e) => e.path)).toEqual(["src/b.ts", "gone.ts"]);
    expect(groups.untracked).toEqual([]);
  });

  it("groups an entry dirty on both sides under staged only", () => {
    const groups = groupStatusEntries([
      entry({ path: "both.ts", index: "modified", worktree: "modified" }),
    ]);
    expect(groups.staged.map((e) => e.path)).toEqual(["both.ts"]);
    expect(groups.changed).toEqual([]);
  });

  it("groups untracked entries regardless of the index side", () => {
    const groups = groupStatusEntries([
      entry({ path: "tmp.log", worktree: "untracked" }),
      entry({ path: "weird.txt", index: "modified", worktree: "untracked" }),
    ]);
    expect(groups.untracked.map((e) => e.path)).toEqual(["tmp.log", "weird.txt"]);
    expect(groups.staged).toEqual([]);
    expect(groups.changed).toEqual([]);
  });

  it("groups conflicted worktree entries under changed", () => {
    const groups = groupStatusEntries([
      entry({ path: "conflict.txt", worktree: "conflicted" }),
      entry({ path: "staged-conflict.txt", index: "modified", worktree: "conflicted" }),
    ]);
    expect(groups.changed.map((e) => e.path)).toEqual(["conflict.txt"]);
    expect(groups.staged.map((e) => e.path)).toEqual(["staged-conflict.txt"]);
    expect(groups.untracked).toEqual([]);
  });

  it("keeps renames in their group under entry.path (old_path intact)", () => {
    const groups = groupStatusEntries([
      entry({ path: "renamed.ts", old_path: "old.ts", index: "renamed" }),
      entry({ path: "wt-renamed.ts", old_path: "wt-old.ts", worktree: "renamed" }),
    ]);
    expect(groups.staged.map((e) => e.path)).toEqual(["renamed.ts"]);
    expect(groups.staged[0]?.old_path).toBe("old.ts");
    expect(groups.changed.map((e) => e.path)).toEqual(["wt-renamed.ts"]);
    expect(groups.changed[0]?.old_path).toBe("wt-old.ts");
  });

  it("preserves backend order within each group", () => {
    const groups = groupStatusEntries([
      entry({ path: "z-late.ts", worktree: "modified" }),
      entry({ path: "a-early.ts", index: "modified" }),
      entry({ path: "m-mid.ts", worktree: "modified" }),
    ]);
    expect(groups.changed.map((e) => e.path)).toEqual(["z-late.ts", "m-mid.ts"]);
    expect(groups.staged.map((e) => e.path)).toEqual(["a-early.ts"]);
  });
});

describe("kindLetter", () => {
  it("maps each change kind to its status letter", () => {
    expect(kindLetter("added")).toBe("A");
    expect(kindLetter("deleted")).toBe("D");
    expect(kindLetter("modified")).toBe("M");
    expect(kindLetter("renamed")).toBe("R");
    expect(kindLetter("copied")).toBe("C");
    expect(kindLetter("untracked")).toBe("U");
    expect(kindLetter("conflicted")).toBe("!");
    expect(kindLetter("unmodified")).toBe("·");
    expect(kindLetter("ignored")).toBe("·");
  });
});

describe("splitPath", () => {
  it("splits nested paths", () => {
    expect(splitPath("src/lib/agent/file.ts")).toEqual({
      base: "file.ts",
      dir: "src/lib/agent",
    });
  });

  it("returns an empty dir for root-level paths", () => {
    expect(splitPath("README.md")).toEqual({ base: "README.md", dir: "" });
  });

  it("handles windows separators", () => {
    expect(splitPath("src\\lib\\file.ts")).toEqual({
      base: "file.ts",
      dir: "src\\lib",
    });
  });
});

describe("openSide", () => {
  it("diffs purely staged entries HEAD→index", () => {
    expect(openSide(entry({ path: "a.ts", index: "modified" }))).toBe("index");
  });

  it("diffs everything else index→worktree", () => {
    expect(openSide(entry({ path: "a.ts", worktree: "modified" }))).toBe("worktree");
    expect(
      openSide(entry({ path: "a.ts", index: "modified", worktree: "modified" })),
    ).toBe("worktree");
    expect(openSide(entry({ path: "a.ts", worktree: "untracked" }))).toBe("worktree");
  });
});

describe("isStageable / isDiscardable", () => {
  it("stages dirty non-conflicted worktree rows, including untracked", () => {
    expect(isStageable(entry({ path: "a.ts", worktree: "modified" }))).toBe(true);
    expect(isStageable(entry({ path: "a.ts", worktree: "untracked" }))).toBe(true);
    expect(isStageable(entry({ path: "a.ts", worktree: "conflicted" }))).toBe(false);
    expect(isStageable(entry({ path: "a.ts", index: "modified" }))).toBe(false);
  });

  it("discards worktree-only rows only", () => {
    expect(isDiscardable(entry({ path: "a.ts", worktree: "modified" }))).toBe(true);
    expect(isDiscardable(entry({ path: "a.ts", worktree: "untracked" }))).toBe(true);
    expect(isDiscardable(entry({ path: "a.ts", worktree: "conflicted" }))).toBe(false);
    expect(isDiscardable(entry({ path: "a.ts", index: "modified" }))).toBe(false);
    expect(
      isDiscardable(entry({ path: "a.ts", index: "modified", worktree: "modified" })),
    ).toBe(false);
  });
});
