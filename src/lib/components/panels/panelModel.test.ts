/**
 * Unit tests for the M3 panel pure logic (`panelModel.ts`): relative age
 * formatting, worktree badge classification, reflog day grouping and the
 * conflict-banner source mapping. No IPC, no DOM.
 */

import { describe, expect, it } from "vitest";
import type { BranchInfo, ReflogEntry, RepoStatus } from "$lib/ipc/types";
import {
  conflictAbortCommand,
  conflictSource,
  conflictSourceLabel,
  groupReflogByDay,
  baseName,
  REFLOG_DISPLAY_CAP,
  reflogDayLabel,
  relativeAge,
  shortSha,
  stashRef,
  worktreeBadges,
  worktreeCandidateBranches,
  worktreeNeedsForce,
  worktreeRefLabel,
} from "./panelModel";

/** Fixed "now": 2026-10-07 12:00:00 local. */
const NOW = new Date(2026, 9, 7, 12, 0, 0).getTime();
const sec = (ms: number): number => Math.floor(ms / 1000);

function sig(time: number) {
  return { name: "Ada", email: "ada@example.com", time, offset_minutes: 0 };
}

function entry(timeSeconds: number, message = "commit: x"): ReflogEntry {
  return {
    old_sha: "1111111111111111111111111111111111111111",
    new_sha: "2222222222222222222222222222222222222222",
    signature: sig(timeSeconds),
    message,
  };
}

function status(patch: Partial<RepoStatus>): RepoStatus {
  return {
    branch: "main",
    head: null,
    detached: false,
    ahead: 0,
    behind: 0,
    merging: false,
    rebasing: false,
    sequencer: false,
    entries: [],
    ...patch,
  };
}

describe("shortSha / baseName / stashRef", () => {
  it("shortens a sha to 7 chars", () => {
    expect(shortSha("2222222222222222222222222222222222222222")).toBe("2222222");
  });

  it("passes through already-short or empty shas", () => {
    expect(shortSha("abc")).toBe("abc");
    expect(shortSha("")).toBe("");
  });

  it("splits names on / and \\", () => {
    expect(baseName("C:/repo/.git/worktrees/one")).toBe("one");
    expect(baseName("C:\\repo\\wt")).toBe("wt");
    expect(baseName("plain")).toBe("plain");
  });

  it("formats the stash ref label", () => {
    expect(stashRef({ index: 3 })).toBe("stash@{3}");
  });
});

describe("relativeAge", () => {
  it("reads fresh timestamps as just now", () => {
    expect(relativeAge(sec(NOW - 10_000), NOW)).toBe("just now");
    expect(relativeAge(sec(NOW + 60_000), NOW)).toBe("just now"); // clock skew
  });

  it("buckets minutes / hours / days", () => {
    expect(relativeAge(sec(NOW - 5 * 60_000), NOW)).toBe("5m ago");
    expect(relativeAge(sec(NOW - 3 * 3_600_000), NOW)).toBe("3h ago");
    expect(relativeAge(sec(NOW - 2 * 86_400_000), NOW)).toBe("2d ago");
  });

  it("buckets weeks / months / years", () => {
    expect(relativeAge(sec(NOW - 14 * 86_400_000), NOW)).toBe("2w ago");
    expect(relativeAge(sec(NOW - 45 * 86_400_000), NOW)).toBe("1mo ago");
    expect(relativeAge(sec(NOW - 400 * 86_400_000), NOW)).toBe("1y ago");
  });

  it("uses weeks below 30 days", () => {
    expect(relativeAge(sec(NOW - 29 * 86_400_000), NOW)).toBe("4w ago");
  });
});

describe("conflictSource", () => {
  it("is null on a clean status", () => {
    expect(conflictSource(status({}))).toBeNull();
  });

  it("maps each in-progress flag", () => {
    expect(conflictSource(status({ merging: true }))).toBe("merge");
    expect(conflictSource(status({ rebasing: true }))).toBe("rebase");
    expect(conflictSource(status({ sequencer: true }))).toBe("sequencer");
  });

  it("rebase wins when rebase and merge flags are both set", () => {
    expect(conflictSource(status({ merging: true, rebasing: true }))).toBe("rebase");
  });

  it("rebase wins over the sequencer flag", () => {
    expect(conflictSource(status({ rebasing: true, sequencer: true }))).toBe("rebase");
  });

  it("labels and abort commands follow the source", () => {
    expect(conflictSourceLabel("merge")).toBe("Merge");
    expect(conflictSourceLabel("rebase")).toBe("Rebase");
    expect(conflictSourceLabel("sequencer")).toBe("Cherry-pick/revert");
    expect(conflictAbortCommand("rebase")).toBe("rebase_abort");
    expect(conflictAbortCommand("merge")).toBe("merge_abort");
    expect(conflictAbortCommand("sequencer")).toBe("merge_abort");
  });
});

describe("worktreeBadges / worktreeNeedsForce / worktreeRefLabel", () => {
  it("clean attached worktree has no badges and needs no force", () => {
    const w = { locked: false, prunable: null, detached: false, branch: "feat", head: null };
    expect(worktreeBadges(w)).toEqual([]);
    expect(worktreeNeedsForce(w)).toBe(false);
    expect(worktreeRefLabel(w)).toBe("feat");
  });

  it("detached head is informational, with a short sha", () => {
    const w = {
      locked: false,
      prunable: null,
      detached: true,
      branch: null,
      head: "2222222222222222222222222222222222222222",
    };
    expect(worktreeBadges(w)).toEqual([
      { label: "detached", danger: false, title: "Detached HEAD" },
    ]);
    expect(worktreeRefLabel(w)).toBe("detached @ 2222222");
  });

  it("locked and prunable are danger badges; prunable carries its reason", () => {
    const w = { locked: true, prunable: "missing branch", detached: false, branch: "b", head: null };
    const badges = worktreeBadges(w);
    expect(badges).toHaveLength(2);
    expect(badges[0]).toMatchObject({ label: "locked", danger: true });
    expect(badges[1]).toMatchObject({
      label: "prunable",
      danger: true,
      title: "Prunable: missing branch",
    });
    expect(worktreeNeedsForce(w)).toBe(true);
  });

  it("empty-string prunable counts as not prunable", () => {
    const w = { locked: false, prunable: "", detached: false, branch: "b", head: null };
    expect(worktreeBadges(w)).toEqual([]);
    expect(worktreeNeedsForce(w)).toBe(false);
  });
});

describe("worktreeCandidateBranches", () => {
  const all: BranchInfo[] = [
    { name: "main", sha: "aaa", upstream: null, ahead: 0, behind: 0, gone: false, is_head: true },
    { name: "feat", sha: "bbb", upstream: null, ahead: 0, behind: 0, gone: false, is_head: false },
  ];

  it("excludes checked-out branches", () => {
    expect(worktreeCandidateBranches(all).map((b) => b.name)).toEqual(["feat"]);
  });
});

describe("reflogDayLabel", () => {
  it("labels today and yesterday", () => {
    expect(reflogDayLabel(sec(new Date(2026, 9, 7, 8, 0).getTime()), NOW)).toBe("Today");
    expect(reflogDayLabel(sec(new Date(2026, 9, 6, 23, 0).getTime()), NOW)).toBe("Yesterday");
  });

  it("formats older days as a locale date", () => {
    const label = reflogDayLabel(sec(new Date(2026, 9, 1, 10, 0).getTime()), NOW);
    expect(label).toMatch(/Oct 1,? 2026|1 Oct 2026|2026 Oct 1/);
  });
});

describe("groupReflogByDay", () => {
  it("returns no groups for an empty reflog", () => {
    expect(groupReflogByDay([], NOW)).toEqual([]);
  });

  it("groups consecutive entries of the same day and labels it", () => {
    const groups = groupReflogByDay(
      [
        entry(sec(new Date(2026, 9, 7, 11, 0).getTime()), "commit: b"),
        entry(sec(new Date(2026, 9, 7, 9, 0).getTime()), "commit: a"),
      ],
      NOW,
    );
    expect(groups).toHaveLength(1);
    expect(groups[0].label).toBe("Today");
    expect(groups[0].entries.map((e) => e.message)).toEqual(["commit: b", "commit: a"]);
  });

  it("starts a new group when the day changes, preserving input order", () => {
    const groups = groupReflogByDay(
      [
        entry(sec(new Date(2026, 9, 7, 11, 0).getTime()), "newest"),
        entry(sec(new Date(2026, 9, 5, 11, 0).getTime()), "older"),
        entry(sec(new Date(2026, 9, 5, 8, 0).getTime()), "oldest"),
      ],
      NOW,
    );
    expect(groups.map((g) => g.label)).toEqual(["Today", "Oct 5, 2026"]);
    expect(groups[1].entries.map((e) => e.message)).toEqual(["older", "oldest"]);
  });
});

describe("REFLOG_DISPLAY_CAP", () => {
  it("is 500 (rendering cap, not a data cap)", () => {
    expect(REFLOG_DISPLAY_CAP).toBe(500);
  });
});
