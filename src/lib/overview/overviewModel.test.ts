import { describe, expect, it } from "vitest";
import type { PrInfo, RepoSummary, StatusEntry } from "$lib/ipc/types";
import {
  applyLiveStatus,
  applySummary,
  attentionRank,
  buildLibrary,
  countsFromEntries,
  defaultExpanded,
  emptyRow,
  groupStatusEntries,
  kindLetter,
  lastOpenedAgo,
  markMissing,
  matchesFilter,
  OVERVIEW_FILTERS,
  OVERVIEW_SORTS,
  refreshedAgo,
  remoteSyncStatus,
  repoProvider,
  rowDirty,
  rowKind,
  shortSha,
  sortRows,
  splitPath,
  openSide,
  totalsOf,
  type OverviewRowData,
} from "./overviewModel";

function entry(
  path: string,
  index: StatusEntry["index"],
  worktree: StatusEntry["worktree"],
  old_path: string | null = null,
): StatusEntry {
  return { path, old_path, index, worktree };
}

function row(patch: Partial<OverviewRowData> = {}): OverviewRowData {
  return {
    ...emptyRow({
      root: "/repos/alpha",
      name: "alpha",
      pinned: false,
      favorite: false,
      lastOpened: 0,
      open: false,
    }),
    loaded: true,
    hasRemote: true,
    remoteUrl: "https://github.com/example/alpha",
    ...patch,
  };
}

function summary(patch: Partial<RepoSummary> = {}): RepoSummary {
  return {
    root: "/repos/alpha",
    name: "alpha",
    branch: "main",
    detached: false,
    ahead: 0,
    behind: 0,
    conflicts: 0,
    staged: 0,
    unstaged: 0,
    untracked: 0,
    entries: [],
    last_commit: null,
    remote_url: null,
    has_remote: false,
    ...patch,
  };
}

function pr(number: number, patch: Partial<PrInfo> = {}): PrInfo {
  return {
    number,
    title: `PR ${number}`,
    head_ref_name: "feature",
    head_ref_oid: "",
    base_ref_name: "main",
    state: "OPEN",
    is_draft: false,
    url: `https://example.test/${number}`,
    created_at: null,
    updated_at: null,
    merged_at: null,
    closed_at: null,
    author: "someone",
    body: "",
    additions: 1,
    deletions: 0,
    changed_files: 1,
    review_decision: "",
    mergeable: "",
    labels: [],
    ...patch,
  };
}

describe("buildLibrary", () => {
  it("unions recents, open tabs and favorites, deduped by canonical root", () => {
    const library = buildLibrary(
      [
        { path: "/repos/alpha", pinned: false, lastOpened: 10 },
        { path: "/repos/beta", pinned: true, lastOpened: 5 },
      ],
      ["/repos/alpha", "/repos/gamma"],
      ["/repos/delta"],
    );
    const roots = library.map((entry) => entry.root).sort();
    expect(roots).toEqual(["/repos/alpha", "/repos/beta", "/repos/delta", "/repos/gamma"]);
    const alpha = library.find((entry) => entry.name === "alpha")!;
    expect(alpha.open).toBe(true);
    expect(alpha.lastOpened).toBe(10);
    const beta = library.find((entry) => entry.name === "beta")!;
    expect(beta.pinned).toBe(true);
    expect(library.find((entry) => entry.name === "delta")!.favorite).toBe(true);
  });

  it("folds separator/case variants of one root into a single entry", () => {
    const library = buildLibrary(
      [{ path: "C:\\repos\\Alpha", pinned: false, lastOpened: 1 }],
      ["c:/repos/alpha/"],
      [],
    );
    expect(library).toHaveLength(1);
    expect(library[0]!.open).toBe(true);
    expect(library[0]!.lastOpened).toBe(1);
  });

  it("sorts pinned first, then by recent activity", () => {
    const library = buildLibrary(
      [
        { path: "/repos/a", pinned: false, lastOpened: 3 },
        { path: "/repos/p", pinned: true, lastOpened: 1 },
        { path: "/repos/b", pinned: false, lastOpened: 9 },
      ],
      [],
      [],
    );
    expect(library.map((entry) => entry.name)).toEqual(["p", "b", "a"]);
  });
});

describe("countsFromEntries / groupStatusEntries", () => {
  it("counts staged, unstaged, untracked and conflicts per kind", () => {
    const entries = [
      entry("staged-only.ts", "modified", "unmodified"),
      entry("both.ts", "modified", "modified"),
      entry("unstaged.ts", "unmodified", "modified"),
      entry("new.ts", "unmodified", "untracked"),
      entry("conflict.ts", "conflicted", "modified"),
    ];
    expect(countsFromEntries(entries)).toEqual({
      staged: 2,
      unstaged: 2,
      untracked: 1,
      conflicts: 1,
    });
  });

  it("counts an index+worktree change on both sides (like the backend)", () => {
    expect(
      countsFromEntries([entry("both.ts", "modified", "modified")]),
    ).toEqual({ staged: 1, unstaged: 1, untracked: 0, conflicts: 0 });
  });

  it("groups each entry into exactly one section", () => {
    const entries = [
      entry("staged-only.ts", "modified", "unmodified"),
      entry("both.ts", "modified", "modified"),
      entry("unstaged.ts", "unmodified", "modified"),
      entry("new.ts", "unmodified", "untracked"),
    ];
    const groups = groupStatusEntries(entries);
    expect(groups.staged.map((e) => e.path)).toEqual(["staged-only.ts", "both.ts"]);
    expect(groups.changed.map((e) => e.path)).toEqual(["unstaged.ts"]);
    expect(groups.untracked.map((e) => e.path)).toEqual(["new.ts"]);
  });

  it("maps change kinds to letters and picks the conflict kind first", () => {
    expect(kindLetter("modified")).toBe("M");
    expect(kindLetter("added")).toBe("A");
    expect(kindLetter("deleted")).toBe("D");
    expect(kindLetter("renamed")).toBe("R");
    expect(kindLetter("untracked")).toBe("U");
    expect(kindLetter("conflicted")).toBe("!");
    expect(kindLetter("unmodified")).toBe("·");
    expect(rowKind(entry("c", "conflicted", "modified"))).toBe("conflicted");
    expect(rowKind(entry("c", "unmodified", "modified"))).toBe("modified");
  });

  it("splits paths and picks the diff side", () => {
    expect(splitPath("a/b/c.txt")).toEqual({ base: "c.txt", dir: "a/b" });
    expect(splitPath("c.txt")).toEqual({ base: "c.txt", dir: "" });
    expect(openSide(entry("s", "modified", "unmodified"))).toBe("index");
    expect(openSide(entry("s", "modified", "modified"))).toBe("worktree");
    expect(openSide(entry("s", "unmodified", "modified"))).toBe("worktree");
  });
});

describe("row updates", () => {
  it("emptyRow initializes new remote and device fields", () => {
    const local = emptyRow({
      root: "/repos/alpha",
      name: "alpha",
      pinned: false,
      favorite: false,
      lastOpened: 12345,
      open: false,
    });
    expect(local.lastOpened).toBe(12345);
    expect(local.hasRemote).toBe(false);
    expect(local.remoteUrl).toBeNull();
    expect(local.isRemoteDevice).toBe(false);

    const ssh = emptyRow({
      root: "ssh://dev-box/repos/beta",
      name: "beta",
      pinned: false,
      favorite: false,
      lastOpened: 0,
      open: true,
    });
    expect(ssh.isRemoteDevice).toBe(true);
  });

  it("applySummary folds a repo_summary into the row and carries over remote/device info", () => {
    const base = {
      ...emptyRow({
        root: "ssh://box/repos/alpha",
        name: "alpha",
        pinned: false,
        favorite: false,
        lastOpened: 42,
        open: false,
      }),
      isRemoteDevice: true,
    };
    const next = applySummary(
      base,
      summary({
        entries: [entry("a.ts", "unmodified", "modified")],
        ahead: 2,
        last_commit: { sha: "a".repeat(40), summary: "init", timestamp: 100 },
        has_remote: true,
        remote_url: "https://github.com/org/alpha.git",
      } as any),
      5000,
    );
    expect(next.loaded).toBe(true);
    expect(next.ahead).toBe(2);
    expect(next.counts.unstaged).toBe(1);
    expect(next.lastCommit?.summary).toBe("init");
    expect(next.refreshedAt).toBe(5000);
    expect(next.lastOpened).toBe(42);
    expect(next.isRemoteDevice).toBe(true);
    expect(next.hasRemote).toBe(true);
    expect(next.remoteUrl).toBe("https://github.com/org/alpha.git");
  });

  it("applyLiveStatus keeps prior gh columns, carries over fields, and stamps freshness", () => {
    const base = {
      ...row({
        prs: [pr(1)],
        failingChecks: 0,
        lastCommit: null,
        refreshedAt: 1,
        lastOpened: 999,
        isRemoteDevice: true,
        hasRemote: true,
        remoteUrl: "https://gitlab.com/group/proj",
      }),
    };
    const next = applyLiveStatus(
      base,
      {
        branch: "main",
        head: null,
        detached: false,
        ahead: 1,
        behind: 0,
        merging: false,
        rebasing: false,
        sequencer: false,
        bisecting: false,
        entries: [entry("a.ts", "modified", "unmodified")],
      },
      9000,
    );
    expect(next.prs).toEqual([pr(1)]);
    expect(next.failingChecks).toBe(0);
    expect(next.counts.staged).toBe(1);
    expect(next.refreshedAt).toBe(9000);
    expect(next.lastOpened).toBe(999);
    expect(next.isRemoteDevice).toBe(true);
    expect(next.hasRemote).toBe(true);
    expect(next.remoteUrl).toBe("https://gitlab.com/group/proj");
  });

  it("markMissing flags the row without wiping provenance", () => {
    const next = markMissing(row({ open: true }), 42);
    expect(next.missing).toBe(true);
    expect(next.loaded).toBe(true);
    expect(next.open).toBe(true);
    expect(next.refreshedAt).toBe(42);
  });
});

describe("remoteSyncStatus", () => {
  it("reports local-only when hasRemote is false", () => {
    const res = remoteSyncStatus(row({ hasRemote: false, ahead: 1, behind: 2 }));
    expect(res).toEqual({
      status: "local-only",
      label: "Local only",
      outgoing: 1,
      incoming: 2,
    });
  });

  it("reports diverged when ahead > 0 and behind > 0", () => {
    const res = remoteSyncStatus(row({ hasRemote: true, ahead: 3, behind: 2 }));
    expect(res).toEqual({
      status: "diverged",
      label: "↑3 ↓2",
      outgoing: 3,
      incoming: 2,
    });
  });

  it("reports needs-push when ahead > 0", () => {
    const res = remoteSyncStatus(row({ hasRemote: true, ahead: 4, behind: 0 }));
    expect(res).toEqual({
      status: "needs-push",
      label: "↑4 to push",
      outgoing: 4,
      incoming: 0,
    });
  });

  it("reports needs-pull when behind > 0", () => {
    const res = remoteSyncStatus(row({ hasRemote: true, ahead: 0, behind: 5 }));
    expect(res).toEqual({
      status: "needs-pull",
      label: "↓5 to pull",
      outgoing: 0,
      incoming: 5,
    });
  });

  it("reports in-sync when ahead === 0 and behind === 0", () => {
    const res = remoteSyncStatus(row({ hasRemote: true, ahead: 0, behind: 0 }));
    expect(res).toEqual({
      status: "in-sync",
      label: "In sync",
      outgoing: 0,
      incoming: 0,
    });
  });
});

describe("repoProvider", () => {
  it("identifies remote-device first", () => {
    expect(repoProvider(row({ isRemoteDevice: true, remoteUrl: "https://github.com/foo/bar" }))).toBe("remote-device");
  });

  it("identifies local when not hasRemote", () => {
    expect(repoProvider(row({ isRemoteDevice: false, hasRemote: false, remoteUrl: "https://github.com/foo/bar" }))).toBe("local");
  });

  it("identifies github from url", () => {
    expect(repoProvider(row({ isRemoteDevice: false, hasRemote: true, remoteUrl: "https://github.com/foo/bar.git" }))).toBe("github");
    expect(repoProvider(row({ isRemoteDevice: false, hasRemote: true, remoteUrl: "git@github.com:foo/bar.git" }))).toBe("github");
  });

  it("identifies gitlab from url", () => {
    expect(repoProvider(row({ isRemoteDevice: false, hasRemote: true, remoteUrl: "https://gitlab.com/foo/bar.git" }))).toBe("gitlab");
    expect(repoProvider(row({ isRemoteDevice: false, hasRemote: true, remoteUrl: "git@gitlab.corp.local:foo/bar.git" }))).toBe("gitlab");
  });

  it("identifies generic remote otherwise", () => {
    expect(repoProvider(row({ isRemoteDevice: false, hasRemote: true, remoteUrl: "https://bitbucket.org/foo/bar.git" }))).toBe("remote");
    expect(repoProvider(row({ isRemoteDevice: false, hasRemote: true, remoteUrl: null }))).toBe("remote");
  });
});

describe("lastOpenedAgo", () => {
  const now = 1_000_000_000_000;

  it("returns null for unknown / non-positive timestamps", () => {
    expect(lastOpenedAgo(0, now)).toBeNull();
    expect(lastOpenedAgo(-1, now)).toBeNull();
    expect(lastOpenedAgo(NaN, now)).toBeNull();
  });

  it("formats seconds, minutes, hours, days ago", () => {
    expect(lastOpenedAgo(now - 10_000, now)).toBe("Accessed 10s ago");
    expect(lastOpenedAgo(now - 15 * 60_000, now)).toBe("Accessed 15m ago");
    expect(lastOpenedAgo(now - 2 * 3_600_000, now)).toBe("Accessed 2h ago");
    expect(lastOpenedAgo(now - 3 * 86_400_000, now)).toBe("Accessed 3d ago");
  });
});

describe("filters", () => {
  const rows: OverviewRowData[] = [
    row({ root: "/a", name: "a", ahead: 2, hasRemote: true }),
    row({ root: "/b", name: "b", behind: 3, hasRemote: true }),
    row({ root: "/c", name: "c", counts: { staged: 1, unstaged: 0, untracked: 0, conflicts: 0 }, hasRemote: true }),
    row({ root: "/d", name: "d", prs: [pr(7)], hasRemote: true }),
    row({ root: "/e", name: "e", failingChecks: 2, hasRemote: true }),
    row({ root: "/f", name: "f", missing: true, hasRemote: true }),
    row({ root: "/g", name: "g", hasRemote: false }),
  ];

  it("every preset selects exactly its rows", () => {
    expect(rows.filter((r) => matchesFilter(r, "needs-push")).map((r) => r.name)).toEqual(["a"]);
    expect(rows.filter((r) => matchesFilter(r, "needs-pull")).map((r) => r.name)).toEqual(["b"]);
    expect(rows.filter((r) => matchesFilter(r, "dirty")).map((r) => r.name)).toEqual(["c"]);
    expect(rows.filter((r) => matchesFilter(r, "has-prs")).map((r) => r.name)).toEqual(["d"]);
    expect(rows.filter((r) => matchesFilter(r, "failing-checks")).map((r) => r.name)).toEqual(["e"]);
    expect(rows.filter((r) => matchesFilter(r, "missing")).map((r) => r.name)).toEqual(["f"]);
    expect(rows.filter((r) => matchesFilter(r, "local-only")).map((r) => r.name)).toEqual(["g"]);
    expect(rows.filter((r) => matchesFilter(r, null))).toHaveLength(7);
  });

  it("exposes the presets in display order", () => {
    expect(OVERVIEW_FILTERS.map((preset) => preset.id)).toEqual([
      "needs-push",
      "needs-pull",
      "dirty",
      "has-prs",
      "failing-checks",
      "missing",
      "local-only",
    ]);
  });
});

describe("sortRows", () => {
  const base = {
    loaded: true,
    lastCommit: { sha: "a".repeat(40), summary: "s", timestamp: 100 },
  };
  const rows: OverviewRowData[] = [
    row({ ...base, root: "/clean", name: "clean" }),
    row({
      ...base,
      root: "/dirty",
      name: "dirty",
      lastCommit: { sha: "b".repeat(40), summary: "s", timestamp: 300 },
      counts: { staged: 0, unstaged: 1, untracked: 0, conflicts: 0 },
    }),
    row({
      ...base,
      root: "/conflict",
      name: "conflict",
      lastCommit: { sha: "c".repeat(40), summary: "s", timestamp: 200 },
      counts: { staged: 0, unstaged: 0, untracked: 0, conflicts: 1 },
    }),
    row({
      ...base,
      root: "/sync",
      name: "sync",
      ahead: 1,
      lastCommit: { sha: "d".repeat(40), summary: "s", timestamp: 400 },
    }),
  ];

  it("attention order: conflicts, sync debt, dirty, clean; ties by newest commit", () => {
    const tied = [
      row({ ...base, root: "/dirty2", name: "dirty2", counts: { staged: 0, unstaged: 2, untracked: 0, conflicts: 0 }, lastCommit: { sha: "e".repeat(40), summary: "s", timestamp: 50 } }),
      row({ ...base, root: "/dirty1", name: "dirty1", counts: { staged: 0, unstaged: 1, untracked: 0, conflicts: 0 }, lastCommit: { sha: "f".repeat(40), summary: "s", timestamp: 500 } }),
    ];
    expect(sortRows(tied, "attention", "asc").map((r) => r.name)).toEqual([
      "dirty1",
      "dirty2",
    ]);
    expect(sortRows(rows, "attention", "asc").map((r) => r.name)).toEqual([
      "conflict",
      "sync",
      "dirty",
      "clean",
    ]);
  });

  it("named columns honor the direction and flip on re-sort", () => {
    expect(sortRows(rows, "name", "asc").map((r) => r.name)).toEqual([
      "clean",
      "conflict",
      "dirty",
      "sync",
    ]);
    expect(sortRows(rows, "name", "desc").map((r) => r.name)).toEqual([
      "sync",
      "dirty",
      "conflict",
      "clean",
    ]);
    expect(sortRows(rows, "last-commit", "desc").map((r) => r.name)[0]).toBe("sync");
  });

  it("ranks by attention helper", () => {
    expect(attentionRank(rows[0]!)).toBe(3);
    expect(attentionRank(rows[1]!)).toBe(2);
    expect(attentionRank(rows[2]!)).toBe(0);
    expect(attentionRank(rows[3]!)).toBe(1);
  });
});

describe("totals + stamps", () => {
  it("totals count repos, dirty rows, ahead/behind sums and PRs", () => {
    const rows = [
      row({ ahead: 2, behind: 1, counts: { staged: 1, unstaged: 0, untracked: 0, conflicts: 0 } }),
      row({ prs: [pr(1), pr(2)] }),
    ];
    expect(totalsOf(rows)).toEqual({
      repos: 2,
      dirty: 1,
      ahead: 2,
      behind: 1,
      prs: 2,
      attentionNeeded: 1,
      outOfSync: 1,
      inSync: 1,
    });
  });

  it("refreshedAgo buckets seconds/minutes/hours/days", () => {
    const now = 1_000_000_000_000;
    expect(refreshedAgo(null, now)).toBeNull();
    expect(refreshedAgo(now - 5_000, now)).toBe("updated 5s ago");
    expect(refreshedAgo(now - 120_000, now)).toBe("updated 2m ago");
    expect(refreshedAgo(now - 7_200_000, now)).toBe("updated 2h ago");
    expect(refreshedAgo(now - 48 * 3_600_000, now)).toBe("updated 2d ago");
  });

  it("row helpers: dirty, default expansion, short sha", () => {
    expect(rowDirty(row())).toBe(false);
    expect(rowDirty(row({ counts: { staged: 0, unstaged: 0, untracked: 1, conflicts: 0 } }))).toBe(true);
    expect(defaultExpanded(row({ ahead: 1 }))).toBe(true);
    expect(defaultExpanded(row({ counts: { staged: 0, unstaged: 0, untracked: 0, conflicts: 1 } }))).toBe(true);
    expect(defaultExpanded(row())).toBe(false);
    expect(shortSha("abcdefghij")).toBe("abcdefg");
  });
});

describe("sort ids", () => {
  it("exposes the sortable columns", () => {
    expect(OVERVIEW_SORTS.map((column) => column.id)).toEqual([
      "name",
      "last-commit",
      "ahead",
      "behind",
      "changes",
      "prs",
    ]);
  });
});
