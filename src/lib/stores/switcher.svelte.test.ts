/**
 * Unit tests for the quick-switcher store ($lib/stores/switcher.svelte):
 * open/close state, item building, the fuzzy scorer and activation. The IPC
 * client is mocked (vi.mock) exactly like tabs.test.ts — no Tauri runtime.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { openRepo, repoStatus } from "$lib/ipc/client";
import { openTab, resetTabStore, tabStore } from "$lib/stores/tabs.svelte";
import {
  activateSwitcherItem,
  buildSwitcherItems,
  closeSwitcher,
  fuzzyMatch,
  isSwitcherOpen,
  normalizeRepoPath,
  openSwitcher,
  rankSwitcherItems,
  repoBasename,
  switcherSnapshot,
  toggleSwitcher,
} from "$lib/stores/switcher.svelte";

vi.mock("$lib/ipc/client", () => ({
  openRepo: vi.fn(),
  closeRepo: vi.fn(),
  repoStatus: vi.fn(),
  onRepoChanged: vi.fn(),
}));

const mockOpenRepo = vi.mocked(openRepo);
const mockRepoStatus = vi.mocked(repoStatus);

function recent(path: string, lastOpened = 1): {
  path: string;
  pinned: boolean;
  lastOpened: number;
} {
  return { path, pinned: false, lastOpened };
}

function tab(id: string, root: string) {
  return {
    id,
    name: repoBasename(root),
    root,
    status: null,
    generation: 0,
  };
}

beforeEach(() => {
  mockOpenRepo.mockReset();
  mockRepoStatus.mockReset();
  mockRepoStatus.mockResolvedValue({
    branch: "main",
    head: "abc",
    detached: false,
    ahead: 0,
    behind: 0,
    merging: false,
    rebasing: false,
    sequencer: false,
    entries: [],
  });
  mockOpenRepo.mockImplementation((path: string) =>
    Promise.resolve({
      repo_id: `id-${path}`,
      root: path,
      name: repoBasename(path),
      bare: false,
      git_dir: `${path}/.git`,
    }),
  );
  closeSwitcher();
  resetTabStore();
});

afterEach(() => {
  closeSwitcher();
  resetTabStore();
});

describe("switcher open/close state", () => {
  it("starts closed and toggles", () => {
    expect(isSwitcherOpen()).toBe(false);
    toggleSwitcher();
    expect(isSwitcherOpen()).toBe(true);
    toggleSwitcher();
    expect(isSwitcherOpen()).toBe(false);
  });

  it("open/close are explicit and the snapshot reflects state", () => {
    openSwitcher();
    expect(isSwitcherOpen()).toBe(true);
    expect(switcherSnapshot().open).toBe(true);
    closeSwitcher();
    expect(switcherSnapshot().open).toBe(false);
  });
});

describe("buildSwitcherItems", () => {
  it("lists open tabs first, then recents", () => {
    const items = buildSwitcherItems(
      [recent("C:/repos/alpha"), recent("C:/repos/beta")],
      [tab("repo-1", "C:/repos/gamma")],
    );
    expect(items.map((i) => i.kind)).toEqual(["tab", "recent", "recent"]);
    expect(items[0]).toMatchObject({
      kind: "tab",
      label: "gamma",
      sub: "C:/repos/gamma",
      target: "repo-1",
    });
    expect(items[1]).toMatchObject({
      kind: "recent",
      label: "alpha",
      sub: "C:/repos/alpha",
      target: "C:/repos/alpha",
    });
  });

  it("drops recents that already have an open tab (normalized compare)", () => {
    const items = buildSwitcherItems(
      [recent("C:\\repos\\gamma\\"), recent("C:/repos/other")],
      [tab("repo-1", "C:/repos/gamma")],
    );
    expect(items.map((i) => i.target)).toEqual(["repo-1", "C:/repos/other"]);
  });
});

describe("normalizeRepoPath / repoBasename", () => {
  it("normalizes separators, case and trailing slashes", () => {
    expect(normalizeRepoPath("C:\\Repos\\Gamma\\")).toBe("c:/repos/gamma");
    expect(normalizeRepoPath("/home/u/repo/")).toBe("/home/u/repo");
  });

  it("takes the final segment as the repo name", () => {
    expect(repoBasename("C:\\repos\\my-repo")).toBe("my-repo");
    expect(repoBasename("/home/u/repo/")).toBe("repo");
    expect(repoBasename("solo")).toBe("solo");
  });
});

describe("fuzzyMatch", () => {
  it("matches case-insensitive subsequences and reports positions", () => {
    const m = fuzzyMatch("MG", "mygitui");
    expect(m).not.toBeNull();
    expect(m!.positions).toEqual([0, 2]); // m(0) … g(2)
  });

  it("returns null when the needle is not a subsequence", () => {
    expect(fuzzyMatch("zzz", "mygitui")).toBeNull();
    expect(fuzzyMatch("gitx", "git")).toBeNull();
  });

  it("empty needle matches everything with score 0", () => {
    expect(fuzzyMatch("", "anything")).toEqual({ score: 0, positions: [] });
  });

  it("consecutive characters outrank scattered ones", () => {
    const tight = fuzzyMatch("ab", "abc")!;
    const scattered = fuzzyMatch("ab", "a-x-b")!;
    expect(tight.score).toBeGreaterThan(scattered.score);
  });

  it("word starts outrank mid-word hits", () => {
    const wordStart = fuzzyMatch("r", "my-repo")!; // r at word start
    const midWord = fuzzyMatch("r", "arrezzo")!; // r inside a word
    expect(wordStart.score).toBeGreaterThan(midWord.score);
  });

  it("exact prefixes score highest (camelCase word start)", () => {
    const prefix = fuzzyMatch("my", "mygitui")!;
    const later = fuzzyMatch("yi", "mygitui")!;
    expect(prefix.score).toBeGreaterThan(later.score);
  });
});

describe("rankSwitcherItems", () => {
  const ITEMS = [
    { id: "tab:1", kind: "tab" as const, label: "mygitui", sub: "C:/code/mygitui", target: "1" },
    { id: "recent:2", kind: "recent" as const, label: "arrepo", sub: "C:/repos/arrepo", target: "C:/repos/arrepo" },
    { id: "recent:3", kind: "recent" as const, label: "library", sub: "C:/repos/library", target: "C:/repos/library" },
  ];

  it("empty query keeps all items in build order with score 0", () => {
    const ranked = rankSwitcherItems(ITEMS, "");
    expect(ranked.map((r) => r.id)).toEqual(["tab:1", "recent:2", "recent:3"]);
    expect(ranked.every((r) => r.score === 0)).toBe(true);
  });

  it("drops items where the query is not a subsequence", () => {
    expect(rankSwitcherItems(ITEMS, "zzz")).toEqual([]);
  });

  it("a prefix match outranks a scattered subsequence", () => {
    // "ar" matches arrepo (word-start prefix) and libraRy (mid-word) —
    // arrepo must rank first.
    const ranked = rankSwitcherItems(ITEMS, "ar");
    expect(ranked.map((r) => r.id)).toEqual(["recent:2", "recent:3"]);
    expect(ranked[0]!.score).toBeGreaterThan(ranked[1]!.score);
  });

  it("matches against the path too (sub line)", () => {
    const ranked = rankSwitcherItems(ITEMS, "code/myg");
    expect(ranked.map((r) => r.id)).toEqual(["tab:1"]);
  });

  it("equal scores keep build order (stable sort)", () => {
    const items = [
      { id: "a", kind: "recent" as const, label: "beta", sub: "/b", target: "/b" },
      { id: "b", kind: "recent" as const, label: "alpha", sub: "/a", target: "/a" },
    ];
    const ranked = rankSwitcherItems(items, "");
    expect(ranked.map((r) => r.label)).toEqual(["beta", "alpha"]);
  });
});

describe("activateSwitcherItem", () => {
  it("closes the switcher and focuses tab items via setActive", async () => {
    // Activation targets the global tab store (same singleton the switcher
    // reads), so drive that here — the IPC client mock backs it.
    await openTab("C:/repos/alpha");
    const item = buildSwitcherItems([], tabStore.tabs)[0]!;
    openSwitcher();

    await activateSwitcherItem(item);

    expect(isSwitcherOpen()).toBe(false);
    expect(tabStore.activeId).toBe(item.target);
  });

  it("opens recent items as repository tabs", async () => {
    const item = {
      id: "recent:C:/repos/beta",
      kind: "recent" as const,
      label: "beta",
      sub: "C:/repos/beta",
      target: "C:/repos/beta",
    };
    openSwitcher();

    await activateSwitcherItem(item);

    expect(isSwitcherOpen()).toBe(false);
    expect(mockOpenRepo).toHaveBeenCalledWith("C:/repos/beta");
  });

  it("activation failures propagate to the caller", async () => {
    mockOpenRepo.mockRejectedValueOnce(new Error("not a repository"));
    const item = {
      id: "recent:C:/bad",
      kind: "recent" as const,
      label: "bad",
      sub: "C:/bad",
      target: "C:/bad",
    };
    await expect(activateSwitcherItem(item)).rejects.toThrow("not a repository");
    // The switcher still closed.
    expect(isSwitcherOpen()).toBe(false);
  });
});
