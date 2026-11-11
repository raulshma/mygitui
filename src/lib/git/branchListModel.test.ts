/**
 * Unit tests for the branches presentation model (`$lib/git/branchListModel`):
 * filter semantics across local and remote rows, head-first ordering,
 * remote grouping (input order preserved), show-more paging, and the
 * tracking-local lookup with its stale-data fallbacks.
 */

import { describe, expect, it } from "vitest";
import type { BranchInfo, RemoteBranchInfo } from "$lib/ipc/types";
import {
  BRANCH_PAGE_SIZE,
  branchMatchesFilter,
  groupRemoteBranches,
  localSectionRows,
  pageRows,
  remoteSectionRows,
  trackingLocalBranch,
} from "./branchListModel";

function branch(patch: Partial<BranchInfo> & { name: string }): BranchInfo {
  return {
    sha: "aaaa1112223334445",
    upstream: null,
    ahead: 0,
    behind: 0,
    gone: false,
    is_head: false,
    ...patch,
  };
}

function remoteBranch(
  patch: Partial<RemoteBranchInfo> & { name: string },
): RemoteBranchInfo {
  return {
    remote: "origin",
    sha: "bbbb2223334445566",
    tracked_by: null,
    ...patch,
  };
}

describe("branchMatchesFilter", () => {
  it("matches substrings case-insensitively and trims the query", () => {
    expect(branchMatchesFilter("feature/auth", " FEAT ")).toBe(true);
    expect(branchMatchesFilter("main", "feat")).toBe(false);
  });

  it("matches everything for a blank filter", () => {
    expect(branchMatchesFilter("main", "")).toBe(true);
    expect(branchMatchesFilter("main", "   ")).toBe(true);
  });
});

describe("localSectionRows", () => {
  it("pins the HEAD branch on top of the backend's alphabetical order", () => {
    const rows = localSectionRows(
      [
        branch({ name: "dev" }),
        branch({ name: "main", is_head: true }),
        branch({ name: "topic" }),
      ],
      "",
    );
    expect(rows.map((r) => r.name)).toEqual(["main", "dev", "topic"]);
  });

  it("keeps an already-first HEAD row in place", () => {
    const rows = localSectionRows(
      [branch({ name: "main", is_head: true }), branch({ name: "dev" })],
      "",
    );
    expect(rows.map((r) => r.name)).toEqual(["main", "dev"]);
  });

  it("filters by name and pins the current branch when it matches", () => {
    const rows = localSectionRows(
      [
        branch({ name: "dev" }),
        branch({ name: "maint", is_head: true }),
        branch({ name: "topic" }),
      ],
      "mai",
    );
    expect(rows.map((r) => r.name)).toEqual(["maint"]);
  });

  it("is stable when no branch is head (detached HEAD)", () => {
    const rows = localSectionRows(
      [branch({ name: "b" }), branch({ name: "a" })],
      "",
    );
    expect(rows.map((r) => r.name)).toEqual(["b", "a"]);
  });
});

describe("remoteSectionRows", () => {
  it("matches the full remote/name, so a remote name filters its group", () => {
    const rows = remoteSectionRows(
      [
        remoteBranch({ name: "main" }),
        remoteBranch({ name: "next" }),
        remoteBranch({ remote: "fork", name: "main" }),
      ],
      "fork",
    );
    expect(rows.map((r) => `${r.remote}/${r.name}`)).toEqual(["fork/main"]);
  });

  it("keeps remote branches a local branch tracks (shown with an affordance)", () => {
    const rows = remoteSectionRows(
      [
        remoteBranch({ name: "main", tracked_by: "main" }),
        remoteBranch({ name: "next" }),
      ],
      "",
    );
    expect(rows).toHaveLength(2);
  });
});

describe("groupRemoteBranches", () => {
  it("one group per remote, first-seen order, branches kept in input order", () => {
    const groups = groupRemoteBranches([
      remoteBranch({ remote: "fork", name: "z" }),
      remoteBranch({ remote: "origin", name: "main" }),
      remoteBranch({ remote: "origin", name: "next" }),
      remoteBranch({ remote: "fork", name: "a" }),
    ]);
    expect(groups.map((g) => g.remote)).toEqual(["fork", "origin"]);
    expect(groups[0].branches.map((b) => b.name)).toEqual(["z", "a"]);
    expect(groups[1].branches.map((b) => b.name)).toEqual(["main", "next"]);
  });

  it("returns no groups for an empty list", () => {
    expect(groupRemoteBranches([])).toEqual([]);
  });
});

describe("pageRows", () => {
  const big = Array.from({ length: 250 }, (_, i) => branch({ name: `b${i}` }));

  it("shows the first page and reports the hidden remainder", () => {
    const page = pageRows(big, false);
    expect(page.visible).toHaveLength(BRANCH_PAGE_SIZE);
    expect(page.hidden).toBe(50);
    expect(page.total).toBe(250);
  });

  it("shows everything once expanded", () => {
    const page = pageRows(big, true);
    expect(page.visible).toHaveLength(250);
    expect(page.hidden).toBe(0);
  });

  it("short-circuits short lists", () => {
    const page = pageRows([branch({ name: "main" })], false);
    expect(page.visible).toHaveLength(1);
    expect(page.hidden).toBe(0);
    expect(page.total).toBe(1);
  });

  it("honors a custom limit", () => {
    expect(pageRows(big, false, 10).visible).toHaveLength(10);
  });
});

describe("trackingLocalBranch", () => {
  it("finds the local branch via the backend's tracked_by pointer", () => {
    const local = branch({ name: "main", upstream: "origin/main" });
    const found = trackingLocalBranch(
      [local],
      remoteBranch({ name: "main", tracked_by: "main" }),
    );
    expect(found).toBe(local);
  });

  it("falls back to the upstream match when tracked_by is stale", () => {
    const local = branch({ name: "main", upstream: "origin/main" });
    expect(
      trackingLocalBranch([local], remoteBranch({ name: "main" })),
    ).toBe(local);
  });

  it("returns null when nothing tracks the remote branch", () => {
    expect(
      trackingLocalBranch([branch({ name: "dev" })], remoteBranch({ name: "main" })),
    ).toBeNull();
    expect(trackingLocalBranch([], remoteBranch({ name: "main" }))).toBeNull();
  });
});
