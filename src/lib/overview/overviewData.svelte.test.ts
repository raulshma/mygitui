import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { RepoId, RepoStatus, RepoSummary } from "$lib/ipc/types";
import { favoriteRepos } from "$lib/stores/favoriteRepos.svelte";
import { RecentRepoStore, STORAGE_KEY as RECENTS_KEY } from "$lib/entry/recentRepos";
import { recentRepos, resetTabStore, tabStore } from "$lib/stores/tabs.svelte";
import {
  OverviewData,
  OverviewWarmer,
  prsForRepo,
  SCAN_CONCURRENCY,
  scanRepoSummaries,
  type OverviewDataOptions,
  type WarmerIpc,
} from "./overviewData.svelte";
import { forgeStore } from "$lib/stores/forge.svelte";

function status(patch: Partial<RepoStatus> = {}): RepoStatus {
  return {
    branch: "main",
    head: null,
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

function summary(root: string, patch: Partial<RepoSummary> = {}): RepoSummary {
  return {
    root,
    name: root.split("/").pop() ?? root,
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

beforeEach(() => {
  resetTabStore();
  favoriteRepos.clear();
  for (const entry of recentRepos.list()) recentRepos.remove(entry.path);
});

afterEach(() => {
  resetTabStore();
  favoriteRepos.clear();
  for (const entry of recentRepos.list()) recentRepos.remove(entry.path);
});

describe("scanRepoSummaries", () => {
  it("scans all roots with at most SCAN_CONCURRENCY in flight", async () => {
    let inFlight = 0;
    let peak = 0;
    const roots = Array.from({ length: 12 }, (_, i) => `/repos/r${i}`);
    const results: string[] = [];
    await scanRepoSummaries(roots, {
      fetch: async (root) => {
        inFlight += 1;
        peak = Math.max(peak, inFlight);
        await new Promise((resolve) => setTimeout(resolve, 1));
        inFlight -= 1;
        return summary(root);
      },
      onResult: (root) => results.push(root),
      onError: () => {},
    });
    expect(peak).toBe(SCAN_CONCURRENCY);
    expect(results.sort()).toEqual([...roots].sort());
  });

  it("reports failures per root and still scans the rest", async () => {
    const errors: string[] = [];
    const done: string[] = [];
    await scanRepoSummaries(["/ok", "/bad", "/ok2"], {
      fetch: async (root) => {
        if (root === "/bad") throw new Error("not a repo");
        return summary(root);
      },
      onResult: (root) => done.push(root),
      onError: (root) => errors.push(root),
    });
    expect(done.sort()).toEqual(["/ok", "/ok2"]);
    expect(errors).toEqual(["/bad"]);
  });

  it("handles an empty root list", async () => {
    await scanRepoSummaries([], {
      fetch: async () => summary("/x"),
      onResult: () => {},
      onError: () => {},
    });
  });
});

describe("OverviewWarmer", () => {
  function fakeIpc(): WarmerIpc & { opened: string[]; closed: string[] } {
    const opened: string[] = [];
    const closed: string[] = [];
    return {
      opened,
      closed,
      open: async (path: string) => {
        opened.push(path);
        return { repo_id: `id-${path}`, root: path };
      },
      close: async (repoId: RepoId) => {
        closed.push(repoId);
      },
    };
  }

  it("warms once per root; concurrent warms coalesce into one open", async () => {
    const ipc = fakeIpc();
    const warmer = new OverviewWarmer(ipc);
    const [a, b] = await Promise.all([
      warmer.warm("/repos/alpha"),
      warmer.warm("/repos/alpha"),
    ]);
    expect(a).toBe("id-/repos/alpha");
    expect(b).toBe("id-/repos/alpha");
    expect(ipc.opened).toEqual(["/repos/alpha"]);
    expect(warmer.isWarmed("/repos/alpha")).toBe(true);
    expect(warmer.idFor("/repos/alpha")).toBe("id-/repos/alpha");
    expect(warmer.idFor("/repos/other")).toBeNull();
  });

  it("a failed open resolves null (row stays summary-tier)", async () => {
    const warmer = new OverviewWarmer({
      open: async () => {
        throw new Error("nope");
      },
      close: async () => {},
    });
    await expect(warmer.warm("/repos/gone")).resolves.toBeNull();
    expect(warmer.isWarmed("/repos/gone")).toBe(false);
  });

  it("closeExcept keeps genuine tabs (path-variant tolerant) and closes the rest", async () => {
    const ipc = fakeIpc();
    const warmer = new OverviewWarmer(ipc);
    await warmer.warm("/repos/alpha");
    await warmer.warm("/repos/beta");
    await warmer.closeExcept(["/repos/alpha"]);
    expect(ipc.closed).toEqual(["id-/repos/beta"]);
    expect(warmer.isWarmed("/repos/alpha")).toBe(true);
    expect(warmer.isWarmed("/repos/beta")).toBe(false);

    await warmer.warm("/repos/Gamma");
    await warmer.closeExcept(["/repos/gamma"]);
    expect(ipc.closed).toEqual(["id-/repos/beta", "id-/repos/alpha"]);
    expect(warmer.isWarmed("/repos/Gamma")).toBe(true);
    expect(warmer.isWarmed("/repos/alpha")).toBe(false);
  });

  it("an open landing after closeExcept is closed, not leaked", async () => {
    const ipc = fakeIpc();
    let release!: (info: { repo_id: RepoId; root: string }) => void;
    const gated = new Promise<{ repo_id: RepoId; root: string }>((resolve) => {
      release = resolve;
    });
    const warmer = new OverviewWarmer({ ...ipc, open: () => gated });
    const warm = warmer.warm("/repos/late");

    // The Overview exits while the open is still in flight.
    await warmer.closeExcept([]);
    release({ repo_id: "late-id", root: "/repos/late" });
    await expect(warm).resolves.toBeNull();
    expect(warmer.isWarmed("/repos/late")).toBe(false);
    expect(ipc.closed).toEqual(["late-id"]);
  });
});

describe("OverviewData", () => {
  /** Builds an isolated OverviewData over in-memory seams. */
  function makeData(overrides: {
    summaries?: Array<[string, RepoSummary | Error]>;
    statuses?: Map<string, RepoStatus>;
    warmer?: OverviewWarmer;
    now?: () => number;
  } = {}): OverviewData {
    const summaries = new Map(overrides.summaries ?? []);
    const statuses = overrides.statuses ?? new Map();
    const options: OverviewDataOptions = {
      summarize: async (root) => {
        const hit = summaries.get(root);
        if (hit instanceof Error) throw hit;
        return hit ?? summary(root);
      },
      status: async (repoId) => statuses.get(repoId) ?? status(),
      // No-op subscription: warmed-row events are driven directly in tests.
      onChange: async () => () => {},
      warmer:
        overrides.warmer ??
        new OverviewWarmer({
          open: async (path) => ({ repo_id: `warm-${path}`, root: path }),
          close: async () => {},
        }),
      now: overrides.now ?? (() => 1_000),
    };
    return new OverviewData(options);
  }

  /** Pushes a fake open tab into the shared tab store (no IPC). */
  function pushTab(id: string, root: string): void {
    tabStore.tabs.push({
      id,
      name: root.split("/").pop() ?? root,
      root,
      status: null,
      statusError: null,
      generation: 0,
      deviceId: null,
      openError: null,
    });
  }

  it("activate seeds rows from the library and scans non-open repos", async () => {
    const data = makeData({
      summaries: [
        [
          "/repos/alpha",
          summary("/repos/alpha", {
            ahead: 2,
            last_commit: { sha: "a".repeat(40), summary: "init", timestamp: 5 },
          }),
        ],
        ["/repos/gone", new Error("not a repo")],
      ],
    });
    recentRepos.add("/repos/alpha");
    recentRepos.add("/repos/gone");

    await data.activate();
    expect(data.rows.map((row) => row.name).sort()).toEqual(["alpha", "gone"]);
    const alpha = data.rowFor("/repos/alpha")!;
    expect(alpha.loaded).toBe(true);
    expect(alpha.missing).toBe(false);
    expect(alpha.ahead).toBe(2);
    expect(alpha.lastCommit?.summary).toBe("init");
    expect(alpha.refreshedAt).toBe(1_000);
    expect(data.rowFor("/repos/gone")!.missing).toBe(true);
    expect(data.scanning).toBe(false);
    expect(data.lastRefreshAt).toBe(1_000);
  });

  it("open-tab rows go live from tab.status; no summary fetch is made", async () => {
    const fetches: string[] = [];
    const data = new OverviewData({
      summarize: async (root) => {
        fetches.push(root);
        return summary(root);
      },
      status: async () => status(),
      onChange: async () => () => {},
      warmer: new OverviewWarmer({
        open: async (path) => ({ repo_id: `warm-${path}`, root: path }),
        close: async () => {},
      }),
      now: () => 2_000,
    });
    pushTab("t1", "/repos/live");
    tabStore.tabs[0]!.status = status({ ahead: 3 });
    await data.activate();

    const liveRow = data.rowFor("/repos/live")!;
    expect(liveRow.open).toBe(true);
    expect(liveRow.repoId).toBe("t1");
    expect(liveRow.ahead).toBe(3);
    expect(liveRow.loaded).toBe(true);
    expect(fetches).toEqual([]);
    expect(liveRow.refreshedAt).toBe(2_000);
  });

  it("refreshWarmedRow applies warmed status through repo_status", async () => {
    const statuses = new Map<string, RepoStatus>([
      ["warm-/repos/cold", status({ ahead: 5, branch: "warmed" })],
    ]);
    const data = makeData({ statuses });
    recentRepos.add("/repos/cold");
    await data.activate();

    const row = data.rowFor("/repos/cold")!;
    row.repoId = "warm-/repos/cold";
    await data.refreshWarmedRow("/repos/cold");
    expect(row.branch).toBe("warmed");
    expect(row.ahead).toBe(5);
  });

  it("warmRow opens a session and applies live status; deactivate closes it", async () => {
    const closed: string[] = [];
    const statuses = new Map<string, RepoStatus>([
      ["warm-/repos/cold", status({ ahead: 5, branch: "warmed" })],
    ]);
    const data = makeData({
      statuses,
      warmer: new OverviewWarmer({
        open: async (path) => ({ repo_id: `warm-${path}`, root: path }),
        close: async (repoId) => {
          closed.push(repoId);
        },
      }),
    });
    recentRepos.add("/repos/cold");
    await data.activate();

    const repoId = await data.warmRow("/repos/cold");
    expect(repoId).toBe("warm-/repos/cold");
    const row = data.rowFor("/repos/cold")!;
    expect(row.repoId).toBe("warm-/repos/cold");
    expect(row.branch).toBe("warmed");
    expect(row.ahead).toBe(5);

    // A genuine tab's warmed session survives the exit (nothing warmed here
    // is a tab, so it closes).
    await data.deactivate();
    expect(closed).toEqual(["warm-/repos/cold"]);
  });

  it("deactivate keeps sessions whose root became a real tab", async () => {
    const closed: string[] = [];
    const data = makeData({
      warmer: new OverviewWarmer({
        open: async (path) => ({ repo_id: `warm-${path}`, root: path }),
        close: async (repoId) => {
          closed.push(repoId);
        },
      }),
    });
    recentRepos.add("/repos/promoted");
    await data.activate();
    await data.warmRow("/repos/promoted");

    // The repo becomes a genuine tab before the Overview closes.
    pushTab("t2", "/repos/promoted");
    await data.deactivate();
    expect(closed).toEqual([]);
  });

  it("addFolder validates via the scan tier; failures are not added", async () => {
    const data = makeData({ summaries: [["/repos/valid", summary("/repos/valid")]] });
    await expect(data.addFolder("/repos/valid")).resolves.toBe(true);
    expect(recentRepos.list().map((entry) => entry.path)).toContain("/repos/valid");

    const failing = makeData({
      summaries: [["/repos/bogus", new Error("not a repo")]],
    });
    await expect(failing.addFolder("/repos/bogus")).resolves.toBe(false);
    expect(recentRepos.list().map((entry) => entry.path)).not.toContain("/repos/bogus");
  });

  it("removeFromLibrary drops recents + favorites and the row (tabs stay)", async () => {
    favoriteRepos.add("/repos/fav");
    const data = makeData();
    recentRepos.add("/repos/fav");
    pushTab("t3", "/repos/fav");
    await data.activate();
    expect(data.rowFor("/repos/fav")).not.toBeNull();

    data.removeFromLibrary("/repos/fav");
    expect(data.rowFor("/repos/fav")).toBeNull();
    expect(favoriteRepos.has("/repos/fav")).toBe(false);
    expect(recentRepos.list()).toHaveLength(0);
    // The tab stays until closed.
    expect(tabStore.tabs.map((tab) => tab.root)).toEqual(["/repos/fav"]);
  });

  it("visibleRows applies filter + sort; totals cover all rows", async () => {
    const data = makeData({
      summaries: [
        ["/repos/quiet", summary("/repos/quiet")],
        ["/repos/loud", summary("/repos/loud", { ahead: 1 })],
      ],
    });
    recentRepos.add("/repos/quiet");
    recentRepos.add("/repos/loud");
    await data.activate();

    expect(data.visibleRows(null, "name", "asc").map((r) => r.name)).toEqual([
      "loud",
      "quiet",
    ]);
    expect(data.visibleRows("needs-push", "name", "asc").map((r) => r.name)).toEqual([
      "loud",
    ]);
    expect(data.totals()).toMatchObject({ repos: 2, ahead: 1, dirty: 0 });
  });

  it("sync refreshes provenance flags (pin/favorite/open) in place", async () => {
    const data = makeData();
    recentRepos.add("/repos/x");
    await data.activate();

    recentRepos.togglePin("/repos/x");
    favoriteRepos.add("/repos/x");
    pushTab("t4", "/repos/x");
    data.sync();

    const row = data.rowFor("/repos/x")!;
    expect(row.pinned).toBe(true);
    expect(row.favorite).toBe(true);
    expect(row.open).toBe(true);
    expect(row.repoId).toBe("t4");
  });

  it("sync clears a closed tab's repoId (no dead ids on summary-tier rows)", async () => {
    const data = makeData();
    recentRepos.add("/repos/x");
    pushTab("t5", "/repos/x");
    await data.activate();
    expect(data.rowFor("/repos/x")!.repoId).toBe("t5");

    tabStore.tabs.splice(0, tabStore.tabs.length);
    data.sync();
    expect(data.rowFor("/repos/x")!.repoId).toBeNull();
    expect(data.rowFor("/repos/x")!.open).toBe(false);
  });

  it("a refresh during an in-flight scan coalesces (no duplicate fetches)", async () => {
    let resolveScan!: () => void;
    const gate = new Promise<void>((resolve) => (resolveScan = resolve));
    let fetches = 0;
    const data = new OverviewData({
      summarize: async (root) => {
        fetches += 1;
        await gate;
        return summary(root);
      },
      status: async () => status(),
      onChange: async () => () => {},
      now: () => 1_000,
    });
    recentRepos.add("/repos/x");

    const pass = data.refresh();
    // focus + visibilitychange both fire while the scan runs: the second
    // call must return immediately instead of scanning again.
    await data.refresh();
    expect(fetches).toBe(1);

    resolveScan();
    await pass;
    expect(fetches).toBe(1);
    expect(data.scanning).toBe(false);
  });

  it("a scan superseded by deactivate leaves rows untouched", async () => {
    let resolveScan!: () => void;
    const gate = new Promise<void>((resolve) => (resolveScan = resolve));
    const data = new OverviewData({
      summarize: async (root) => {
        await gate;
        return summary(root, { ahead: 9 });
      },
      status: async () => status(),
      onChange: async () => () => {},
      now: () => 1_000,
    });
    recentRepos.add("/repos/x");

    const pass = data.refresh();
    await expect.poll(() => data.scanning).toBe(true);

    await data.deactivate();
    resolveScan();
    await pass;

    // The stale result is dropped: the row keeps its skeleton state and no
    // refresh stamp is written while hidden.
    const row = data.rowFor("/repos/x")!;
    expect(row.ahead).toBe(0);
    expect(row.loaded).toBe(false);
    expect(row.missing).toBe(false);
    expect(data.scanning).toBe(false);
    expect(data.lastRefreshAt).toBeNull();
  });

  it("sync populates lastOpened, isRemoteDevice, and carries over remote info", async () => {
    const data = makeData();
    recentRepos.add("/repos/local");
    const recents = recentRepos.list();
    const entry = recents.find((r) => r.path === "/repos/local");
    expect(entry).toBeDefined();

    // Add ssh remote tab
    pushTab("ssh-1", "ssh://remote-device/srv/git/project");
    tabStore.tabs[0]!.deviceId = "remote-device";
    (tabStore.tabs[0]! as any).hasRemote = true;
    (tabStore.tabs[0]! as any).remoteUrl = "https://github.com/org/repo.git";

    data.sync();

    const localRow = data.rowFor("/repos/local")!;
    expect(localRow.lastOpened).toBe(entry!.lastOpened);
    expect(localRow.isRemoteDevice).toBe(false);
    expect(localRow.hasRemote).toBe(false);

    const sshRow = data.rowFor("ssh://remote-device/srv/git/project")!;
    expect(sshRow.isRemoteDevice).toBe(true);
    expect(sshRow.hasRemote).toBe(true);
    expect(sshRow.remoteUrl).toBe("https://github.com/org/repo.git");
  });

  it("applyTabData carries over remote info from tab into the row", async () => {
    const data = makeData();
    pushTab("tab-remote", "/repos/open-repo");
    (tabStore.tabs[0]! as any).hasRemote = true;
    (tabStore.tabs[0]! as any).remoteUrl = "https://gitlab.com/team/repo.git";
    tabStore.tabs[0]!.status = status({ ahead: 1, behind: 2 });

    await data.activate();

    const row = data.rowFor("/repos/open-repo")!;
    expect(row.open).toBe(true);
    expect(row.hasRemote).toBe(true);
    expect(row.remoteUrl).toBe("https://gitlab.com/team/repo.git");
    expect(row.ahead).toBe(1);
    expect(row.behind).toBe(2);
  });

  it("prsForRepo resolves open PRs and falls back to snapshot", () => {
    expect(prsForRepo(null)).toBeNull();

    // Mocking forgeStore methods
    const origPrsStale = forgeStore.prsStaleFor;
    const origSnapshotStale = forgeStore.snapshotStaleFor;

    try {
      (forgeStore as any).prsStaleFor = () => null;
      (forgeStore as any).snapshotStaleFor = () => null;
      expect(prsForRepo("r1")).toBeNull();

      // Snapshot with open PR fallback
      (forgeStore as any).snapshotStaleFor = () => ({
        branch: "feature",
        remote: "origin",
        pr: { number: 42, title: "Test PR", state: "OPEN", head_ref_name: "feature", head_ref_oid: "abc", is_draft: false, url: "https://github.com/org/repo/pull/42", review_decision: null, author: "dev" },
        pr_checks: [],
        head_checks: [],
      });
      expect(prsForRepo("r1")).toEqual([
        expect.objectContaining({ number: 42, state: "OPEN" }),
      ]);

      // PRs list filters only OPEN
      (forgeStore as any).prsStaleFor = () => [
        { number: 1, title: "Open PR", state: "OPEN", head_ref_name: "feat1", head_ref_oid: "1", is_draft: false, url: "", review_decision: null, author: "dev" },
        { number: 2, title: "Closed PR", state: "CLOSED", head_ref_name: "feat2", head_ref_oid: "2", is_draft: false, url: "", review_decision: null, author: "dev" },
        { number: 3, title: "Merged PR", state: "MERGED", head_ref_name: "feat3", head_ref_oid: "3", is_draft: false, url: "", review_decision: null, author: "dev" },
      ];
      expect(prsForRepo("r1")).toEqual([
        expect.objectContaining({ number: 1, state: "OPEN" }),
      ]);
    } finally {
      (forgeStore as any).prsStaleFor = origPrsStale;
      (forgeStore as any).snapshotStaleFor = origSnapshotStale;
    }
  });

  it("warms remote device repositories directly without calling repoSummary", async () => {
    const fetched: string[] = [];
    const warmed: string[] = [];
    const data = new OverviewData({
      summarize: async (root) => {
        fetched.push(root);
        return summary(root);
      },
      status: async () => status(),
      onChange: async () => () => {},
      warmer: new OverviewWarmer({
        open: async (path) => {
          warmed.push(path);
          return { repo_id: `warm-${path}`, root: path };
        },
        close: async () => {},
      }),
      now: () => 1_000,
    });

    recentRepos.add("ssh://vps-server/srv/repo");
    await data.activate();

    // Should NOT call repoSummary on ssh:// roots (which would fail in git2 discover)
    expect(fetched).not.toContain("ssh://vps-server/srv/repo");
    // Should warm it directly
    expect(warmed).toContain("ssh://vps-server/srv/repo");
    const row = data.rowFor("ssh://vps-server/srv/repo")!;
    expect(row.missing).toBe(false);
  });
});
