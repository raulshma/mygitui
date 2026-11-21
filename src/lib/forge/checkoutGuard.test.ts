/**
 * Unit tests for the checkout-safety decision (forge/checkoutGuard.ts,
 * E2 §9.4): the popover checkout hard-refuses on a dirty worktree and the
 * panel checkout offers a stash — both read this one decision.
 */

import { describe, expect, it } from "vitest";
import type { RepoStatus, StatusEntry } from "$lib/ipc/types";
import { checkoutRefusal, dirtyChangeCount } from "./checkoutGuard";

function entry(path: string): StatusEntry {
  return {
    path,
    old_path: null,
    index: "modified",
    worktree: "unmodified",
  };
}

function status(patch: Partial<RepoStatus> = {}): RepoStatus {
  return {
    branch: "feat",
    head: "abc123",
    detached: false,
    ahead: 0,
    behind: 0,
    merging: false,
    rebasing: false,
    sequencer: false,
    bisecting: false,
    entries: [],
    ...patch,
  };
}

describe("checkoutRefusal", () => {
  it("allows a clean worktree and an unknown status", () => {
    expect(checkoutRefusal(status())).toBeNull();
    expect(checkoutRefusal(null)).toBeNull();
  });

  it("refuses on any change entries, with a count", () => {
    const dirty = status({
      entries: [entry("a.txt"), entry("b.txt"), entry("c.txt")],
    });
    const refusal = checkoutRefusal(dirty);
    expect(refusal).toContain("3 uncommitted changes");
    expect(checkoutRefusal(status({ entries: [entry("a.txt")] }))).toContain(
      "1 uncommitted change",
    );
  });

  it("refuses on in-progress operations", () => {
    expect(checkoutRefusal(status({ merging: true }))).toContain("merge");
    expect(checkoutRefusal(status({ rebasing: true }))).toContain("rebase");
    expect(checkoutRefusal(status({ sequencer: true }))).toContain(
      "cherry-pick",
    );
    expect(checkoutRefusal(status({ bisecting: true }))).toContain("bisect");
  });
});

describe("dirtyChangeCount", () => {
  it("counts distinct paths (a staged and modified file counts once)", () => {
    expect(dirtyChangeCount(null)).toBe(0);
    expect(dirtyChangeCount(status())).toBe(0);
    expect(
      dirtyChangeCount(
        status({ entries: [entry("a.txt"), entry("a.txt"), entry("b.txt")] }),
      ),
    ).toBe(2);
  });
});
