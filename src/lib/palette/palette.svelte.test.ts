/**
 * Command-palette store + registry tests ($lib/palette/palette.svelte):
 * the fuzzy scorer, ranking, section building (with the Recent block),
 * `when` filtering against the active-tab context, recents persistence and
 * command execution (close → record → run → toast failures). The IPC client
 * is mocked (vi.mock) — the tab store is real, seeded with a fake tab.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  checkpoints,
  fetchRepo,
  guardCheckpoint,
  openRepo,
  pickFolder,
  pullRepo,
  pushRepo,
  remotes,
  repoStatus,
  stashApply,
  stashList,
  stashPush,
  stageAll,
} from "$lib/ipc/client";
import { DEFAULT_BINDINGS } from "$lib/palette/keybinds";
import {
  THEME_STORAGE_KEY,
  getThemePreference,
  initTheme,
  setThemePreference,
} from "$lib/theme";
import { getToasts } from "$lib/toast";
import { resetTabStore, tabStore } from "$lib/stores/tabs.svelte";
import {
  COMMANDS,
  activeCtx,
  commandById,
  visibleCommands,
  type Command,
  type CommandCtx,
} from "$lib/palette/commands";
import {
  PALETTE_RECENTS_KEY,
  RECENTS_LIMIT,
  closePalette,
  executeCommandById,
  flattenSections,
  fuzzyScore,
  getRecents,
  isPaletteOpen,
  openPalette,
  paletteSections,
  paletteSnapshot,
  rankCommands,
  recordRecent,
  resetRecents,
  runCommand,
  togglePalette,
  type PaletteSection,
  type StorageLike,
} from "$lib/palette/palette.svelte";

vi.mock("$lib/ipc/client", () => ({
  // tabs.svelte
  openRepo: vi.fn(),
  closeRepo: vi.fn(),
  repoStatus: vi.fn(),
  onRepoChanged: vi.fn(),
  // safety.svelte
  checkpoints: vi.fn(),
  guardCheckpoint: vi.fn(),
  // commands.ts
  checkpointGc: vi.fn(),
  fetchRepo: vi.fn(),
  pickFolder: vi.fn(),
  pullRepo: vi.fn(),
  pushRepo: vi.fn(),
  remotes: vi.fn(),
  stashApply: vi.fn(),
  stashList: vi.fn(),
  stashPush: vi.fn(),
  stageAll: vi.fn(),
}));

const HOME: CommandCtx = { repoId: null, root: null };
const REPO: CommandCtx = { repoId: "r1", root: "/repos/r1" };

/** In-memory Storage fake for recents tests. */
function fakeStorage(): StorageLike & { store: Map<string, string> } {
  const store = new Map<string, string>();
  return {
    store,
    getItem: (key) => store.get(key) ?? null,
    setItem: (key, value) => void store.set(key, value),
  };
}

/** Minimal command factory for scorer/ranking tests. */
function cmd(
  id: string,
  title: string,
  section: string,
  extra: Partial<Command> = {},
): Command {
  return { id, title, section, run: () => {}, ...extra };
}

function seedTab(id = "r1"): void {
  tabStore.tabs.push({
    id,
    name: "r1",
    root: "/repos/r1",
    status: { branch: "main" } as never,
    generation: 0,
  });
  tabStore.activeId = id;
}

beforeEach(() => {
  resetRecents(null); // in-memory recents, no storage side effects
  closePalette();
});

afterEach(() => {
  resetTabStore();
});

describe("registry shape", () => {
  it("has unique, non-empty ids/titles/sections and runnable commands", () => {
    const ids = new Set<string>();
    for (const command of COMMANDS) {
      expect(command.id).toMatch(/^[a-z][a-z0-9.-]*$/);
      expect(ids.has(command.id)).toBe(false);
      ids.add(command.id);
      expect(command.title.length).toBeGreaterThan(0);
      expect(command.section.length).toBeGreaterThan(0);
      expect(typeof command.run).toBe("function");
    }
    expect(ids.size).toBe(COMMANDS.length);
    expect(COMMANDS.length).toBeGreaterThanOrEqual(38);
  });

  it("shortcuts stay consistent with the keybind defaults", () => {
    for (const command of COMMANDS) {
      if (command.shortcut !== undefined) {
        expect(DEFAULT_BINDINGS[command.id]).toBe(command.shortcut);
      }
    }
    for (const [id, combo] of Object.entries(DEFAULT_BINDINGS)) {
      expect(commandById(COMMANDS, id)?.shortcut).toBe(combo);
    }
  });

  it("curates every documented section", () => {
    const sections = new Set(COMMANDS.map((command) => command.section));
    for (const section of [
      "Repo",
      "Status",
      "Branches",
      "History",
      "Diff",
      "Conflicts",
      "Safety",
      "Panels",
      "App",
    ]) {
      expect(sections.has(section)).toBe(true);
    }
  });
});

describe("when() filtering", () => {
  it("hides repo-scoped commands on the home view", () => {
    const visible = visibleCommands(COMMANDS, HOME);
    for (const command of visible) {
      expect(command.when?.(HOME) ?? true).toBe(true);
    }
    expect(commandById(visible, "status.stage-all")).toBeUndefined();
    expect(commandById(visible, "repo.open")).toBeDefined();
    expect(commandById(visible, "app.theme-toggle")).toBeDefined();
  });

  it("shows repo-scoped commands with an active tab", () => {
    const visible = visibleCommands(COMMANDS, REPO);
    expect(commandById(visible, "status.stage-all")).toBeDefined();
    expect(commandById(visible, "panel.focus-undo")).toBeDefined();
    expect(commandById(visible, "branches.pull")).toBeDefined();
  });
});

describe("fuzzy scorer", () => {
  it("matches subsequences with positions and bonuses", () => {
    const hit = fuzzyScore("sa", "Stage all changes");
    expect(hit).not.toBeNull();
    expect(hit!.positions).toEqual([0, 2]); // greedy earliest match
    expect(hit!.score).toBeGreaterThan(2); // word-start bonus on "Stage"
  });

  it("returns an empty match for an empty query", () => {
    expect(fuzzyScore("", "anything")).toEqual({ score: 0, positions: [] });
  });

  it("returns null when the needle is not a subsequence", () => {
    expect(fuzzyScore("xz", "stage all")).toBeNull();
  });

  it("rewards consecutive runs over scattered matches", () => {
    const tight = fuzzyScore("st", "stage")!;
    const loose = fuzzyScore("st", "set out")!;
    expect(tight.score).toBeGreaterThan(loose.score);
  });
});

describe("ranking + sections", () => {
  it("ranks by score and drops non-matches", () => {
    const commands = [
      cmd("a", "Stage all changes", "Status"),
      cmd("b", "Stash changes", "Status"),
      cmd("c", "Toggle theme", "App"),
    ];
    // Both "sta…" titles match; ties keep registry order.
    const ranked = rankCommands(commands, "stage");
    expect(ranked.map((entry) => entry.command.id)).toEqual(["a", "b"]);
    expect(rankCommands(commands, "sta").map((entry) => entry.command.id)).toEqual([
      "a",
      "b",
    ]);
    expect(rankCommands(commands, "zzz")).toEqual([]);
  });

  it("matches keywords too", () => {
    const commands = [cmd("a", "Title", "Section", { keywords: ["foobar"] })];
    expect(rankCommands(commands, "foobar").length).toBe(1);
    expect(rankCommands(commands, "fub").length).toBe(0);
  });

  it("keeps everything (stable order) on an empty query", () => {
    const commands = [cmd("a", "Aaa", "S1"), cmd("b", "Bbb", "S1")];
    const ranked = rankCommands(commands, "");
    expect(ranked.map((entry) => entry.command.id)).toEqual(["a", "b"]);
    expect(ranked.every((entry) => entry.score === 0)).toBe(true);
  });

  it("groups by section preserving first-seen order", () => {
    const commands = [
      cmd("a", "Aaa", "Repo"),
      cmd("b", "Bbb", "App"),
      cmd("c", "Ccc", "Repo"),
    ];
    const sections = paletteSections(commands, HOME, "");
    expect(sections.map((section) => section.name)).toEqual(["Repo", "App"]);
    expect(sections[0]!.commands.map((entry) => entry.command.id)).toEqual(["a", "c"]);
  });

  it("puts matching recents first on an empty query, deduped", () => {
    const commands = [cmd("a", "Aaa", "Repo"), cmd("b", "Bbb", "App")];
    const sections = paletteSections(commands, HOME, "", ["b", "zz", "a"]);
    expect(sections[0]!.name).toBe("Recent");
    expect(sections[0]!.commands.map((entry) => entry.command.id)).toEqual(["b", "a"]);
    // Deduped: "b" appears once overall.
    const flat = flattenSections(sections);
    expect(flat.filter((entry) => entry.command.id === "b").length).toBe(1);
  });

  it("applies when() inside paletteSections", () => {
    const commands = [
      cmd("a", "Aaa", "Repo"),
      cmd("b", "Bbb", "Status", { when: (ctx) => ctx.repoId !== null }),
    ];
    expect(paletteSections(commands, HOME, "").length).toBe(1);
    const flat = flattenSections(paletteSections(commands, REPO, ""));
    expect(flat.map((entry) => entry.command.id)).toEqual(["a", "b"]);
  });

  it("ranks into sections under a query", () => {
    const commands = [
      cmd("a", "Stage all changes", "Status"),
      cmd("b", "Stash changes", "Status"),
    ];
    const sections: PaletteSection[] = paletteSections(commands, REPO, "stash");
    expect(sections.length).toBe(1);
    expect(sections[0]!.commands[0]!.command.id).toBe("b");
  });
});

describe("recents", () => {
  it("records newest-first, dedupes and caps at the limit", () => {
    const storage = fakeStorage();
    for (const id of ["a", "b", "c", "d", "e", "f", "b"]) recordRecent(id, storage);
    const recents = getRecents(storage);
    expect(recents.length).toBe(RECENTS_LIMIT);
    expect(recents).toEqual(["b", "f", "e", "d", "c"]); // newest first, capped
    expect(JSON.parse(storage.store.get(PALETTE_RECENTS_KEY)!).length).toBe(5);
  });

  it("drops malformed persisted data", () => {
    const storage = fakeStorage();
    storage.store.set(PALETTE_RECENTS_KEY, "{nope");
    expect(getRecents(storage)).toEqual([]);
    storage.store.set(PALETTE_RECENTS_KEY, JSON.stringify(["ok", 42, "", null]));
    expect(getRecents(storage)).toEqual(["ok"]);
  });

  it("works without storage", () => {
    recordRecent("a", null);
    expect(paletteSnapshot().recents).toEqual(["a"]);
  });
});

describe("open/close + execution", () => {
  it("toggles the overlay flag", () => {
    expect(isPaletteOpen()).toBe(false);
    openPalette();
    expect(isPaletteOpen()).toBe(true);
    togglePalette();
    expect(isPaletteOpen()).toBe(false);
  });

  it("runCommand closes, records the recent and runs", async () => {
    seedTab();
    vi.mocked(stageAll).mockResolvedValueOnce(undefined);
    const run = vi.fn();
    await runCommand(cmd("test.a", "A", "App", { run }), REPO);
    expect(isPaletteOpen()).toBe(false);
    expect(paletteSnapshot().recents).toContain("test.a");
    expect(run).toHaveBeenCalledWith(REPO);
  });

  it("runCommand toasts failures and still closes", async () => {
    const toastsBefore = getToasts().length;
    await runCommand(
      cmd("test.fail", "F", "App", {
        run: () => Promise.reject(new Error("boom")),
      }),
      REPO,
    );
    expect(isPaletteOpen()).toBe(false);
    const last = getToasts().at(-1);
    expect(getToasts().length).toBeGreaterThan(toastsBefore);
    expect(last?.kind).toBe("error");
    expect(last?.message).toContain("boom");
  });

  it("executeCommandById resolves the ctx from the active tab", async () => {
    seedTab();
    vi.mocked(remotes).mockResolvedValueOnce([
      { name: "origin", url: "u", push_url: null },
    ]);
    vi.mocked(fetchRepo).mockResolvedValueOnce({
      received_bytes: 0,
      objects: 0,
      updated_refs: [],
    });
    await executeCommandById("branches.fetch");
    expect(remotes).toHaveBeenCalledWith("r1");
    expect(fetchRepo).toHaveBeenCalledWith("r1", {
      remote: "origin",
      prune: false,
      refs: [],
      depth: null,
    });
  });

  it("executeCommandById ignores unknown ids", async () => {
    await expect(executeCommandById("no.such.command")).resolves.toBeUndefined();
  });

  it("status.stage-all stages then refreshes", async () => {
    seedTab();
    vi.mocked(stageAll).mockResolvedValueOnce(undefined);
    await executeCommandById("status.stage-all");
    expect(stageAll).toHaveBeenCalledWith("r1", false);
  });

  it("status.stash-pop is a no-op hint when no stashes exist", async () => {
    seedTab();
    vi.mocked(stashList).mockResolvedValueOnce([]);
    const toastsBefore = getToasts().length;
    await executeCommandById("status.stash-pop");
    expect(stashList).toHaveBeenCalledWith("r1");
    expect(stashApply).not.toHaveBeenCalled();
    expect(getToasts().at(-1)?.kind).toBe("info");
    expect(getToasts().length).toBe(toastsBefore + 1);
  });

  it("status.stash-pop pops the latest stash", async () => {
    seedTab();
    vi.mocked(stashList).mockResolvedValueOnce([
      { ref: "stash@{0}", message: "wip" } as never,
    ]);
    vi.mocked(stashApply).mockResolvedValueOnce(undefined);
    await executeCommandById("status.stash-pop");
    expect(stashApply).toHaveBeenCalledWith("r1", 0, true);
  });

  it("safety.checkpoint-now guards with the manual reason", async () => {
    seedTab();
    vi.mocked(guardCheckpoint).mockResolvedValueOnce({ id: "cp1" } as never);
    vi.mocked(checkpoints).mockResolvedValueOnce([]);
    await executeCommandById("safety.checkpoint-now");
    expect(guardCheckpoint).toHaveBeenCalledWith("r1", "manual checkpoint");
  });

  it("branches.push requires a current branch", async () => {
    const toastsBefore = getToasts().length;
    await executeCommandById("branches.push"); // no seeded tab
    expect(pushRepo).not.toHaveBeenCalled();
    expect(getToasts().length).toBe(toastsBefore);
  });

  it("repo.open-folder picks then opens", async () => {
    vi.mocked(pickFolder).mockResolvedValueOnce("/picked/repo");
    vi.mocked(openRepo).mockResolvedValueOnce({
      repo_id: "r2",
      name: "repo",
      root: "/picked/repo",
    } as never);
    vi.mocked(repoStatus).mockResolvedValueOnce({} as never);
    await executeCommandById("repo.open-folder");
    expect(pickFolder).toHaveBeenCalled();
    expect(openRepo).toHaveBeenCalledWith("/picked/repo");
  });

  it("status.stash pushes with a palette message and refreshes", async () => {
    seedTab();
    vi.mocked(stashPush).mockResolvedValueOnce(undefined);
    vi.mocked(repoStatus).mockResolvedValueOnce({} as never);
    await executeCommandById("status.stash");
    expect(stashPush).toHaveBeenCalledWith("r1", "WIP (command palette)");
  });
});

describe("activeCtx", () => {
  it("mirrors the active tab, or nulls on the home view", () => {
    expect(activeCtx()).toEqual({ repoId: null, root: null });
    seedTab();
    expect(activeCtx()).toEqual({ repoId: "r1", root: "/repos/r1" });
  });
});

describe("theme commands (M7 I2)", () => {
  afterEach(() => {
    localStorage.removeItem(THEME_STORAGE_KEY);
    setThemePreference("system");
  });

  it("registers toggle + follow-system entries in the App section", () => {
    const toggle = commandById(COMMANDS, "app.theme-toggle");
    const system = commandById(COMMANDS, "app.theme-system");
    expect(toggle).toBeDefined();
    expect(system).toBeDefined();
    expect(toggle!.section).toBe("App");
    expect(system!.section).toBe("App");
    expect(toggle!.title.length).toBeGreaterThan(0);
    expect(system!.title.toLowerCase()).toContain("system");
  });

  it("toggle title reflects the live preference (names the target scheme)", async () => {
    await initTheme(); // back the toggle's currentScheme() read with real state
    setThemePreference("dark");
    expect(commandById(COMMANDS, "app.theme-toggle")!.title).toMatch(
      /switch to light theme/i,
    );
    setThemePreference("light");
    expect(commandById(COMMANDS, "app.theme-toggle")!.title).toMatch(
      /switch to dark theme/i,
    );
  });

  it("app.theme-toggle flips the pinned scheme and remembers it", async () => {
    await initTheme(); // currentScheme() must reflect applied state, not the "light" default
    const toggle = commandById(COMMANDS, "app.theme-toggle")!;

    // jsdom has no matchMedia: the applied scheme is light, so toggle → dark.
    await toggle.run(HOME);
    expect(getThemePreference()).toBe("dark");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");

    await toggle.run(HOME);
    expect(getThemePreference()).toBe("light");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("light");
  });

  it("app.theme-system resets to live OS follow", async () => {
    await initTheme();
    setThemePreference("dark");

    const system = commandById(COMMANDS, "app.theme-system")!;
    await system.run(HOME);

    expect(getThemePreference()).toBe("system");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("system");
  });
});
