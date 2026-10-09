/**
 * Unit tests for the merge dirty-guard (merge/mergeModel.ts).
 */

import { describe, expect, it } from "vitest";
import type { StatusEntry } from "$lib/ipc/types";
import { workingCopyDirty } from "$lib/components/merge/mergeModel";

function entry(index: StatusEntry["index"], worktree: StatusEntry["index"] = "unmodified"): StatusEntry {
  return { path: "a.txt", old_path: null, index, worktree };
}

describe("workingCopyDirty", () => {
  it("tolerates unmodified / ignored / untracked entries on both sides", () => {
    expect(workingCopyDirty([])).toBe(false);
    expect(workingCopyDirty([entry("unmodified")])).toBe(false);
    expect(workingCopyDirty([entry("untracked", "untracked")])).toBe(false);
    expect(workingCopyDirty([entry("ignored", "ignored")])).toBe(false);
  });

  it("is dirty when the index side carries a change", () => {
    expect(workingCopyDirty([entry("modified")])).toBe(true);
    expect(workingCopyDirty([entry("added")])).toBe(true);
    expect(workingCopyDirty([entry("deleted")])).toBe(true);
    expect(workingCopyDirty([entry("renamed")])).toBe(true);
    expect(workingCopyDirty([entry("copied")])).toBe(true);
  });

  it("is dirty when the worktree side carries a change", () => {
    expect(workingCopyDirty([entry("unmodified", "modified")])).toBe(true);
    expect(workingCopyDirty([entry("unmodified", "deleted")])).toBe(true);
  });

  it("treats conflicts as dirty", () => {
    expect(workingCopyDirty([entry("conflicted", "conflicted")])).toBe(true);
  });

  it("scans every entry", () => {
    expect(
      workingCopyDirty([entry("unmodified"), entry("unmodified", "modified")]),
    ).toBe(true);
    expect(
      workingCopyDirty([entry("unmodified"), entry("unmodified")]),
    ).toBe(false);
  });
});
