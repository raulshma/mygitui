/**
 * CommandPalette component tests: the overlay itself (the store pieces —
 * scorer, ranking, recents — live in `palette.svelte.test.ts`). Covers the
 * user-visible contract: opens via the store, filters per keystroke with
 * matched-position highlighting, keyboard navigation (arrows/Enter/Esc) and
 * the Recent block after a run. The IPC client is mocked (vi.mock) — the
 * tab store is real, seeded with a fake tab.
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
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
  stageAll,
  stashApply,
  stashList,
  stashPush,
} from "$lib/ipc/client";
import { resetTabStore, tabStore, type Tab } from "$lib/stores/tabs.svelte";
import { __resetKeybindsForTests } from "$lib/palette/keybinds";
import {
  closePalette,
  isPaletteOpen,
  openPalette,
  resetRecents,
} from "$lib/palette/palette.svelte";
import CommandPalette from "./CommandPalette.svelte";

vi.mock("$lib/ipc/client", () => ({
  // tabs.svelte
  openRepo: vi.fn(),
  closeRepo: vi.fn(),
  repoStatus: vi.fn(),
  onRepoChanged: vi.fn(),
  // $lib/events/repoEvents (imported by tabs.svelte) pulls the other event
  // seams in transitively; they are never called in this suite.
  onCloneProgress: vi.fn(),
  onOpProgress: vi.fn(),
  onOpLog: vi.fn(),
  onAuthRequest: vi.fn(),
  onActionOutput: vi.fn(),
  onPtyOutput: vi.fn(),
  onPtyExit: vi.fn(),
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

function seedTab(id = "r1"): void {
  tabStore.tabs.push({
    id,
    name: "r1",
    root: "/repos/r1",
    status: { branch: "main" } as never,
    statusError: null,
    generation: 0,
    deviceId: null,
    openError: null,
  } as Tab);
  tabStore.activeId = id;
}

/** Renders the palette (it draws nothing while closed) and opens it. */
async function openRendered(): Promise<void> {
  render(CommandPalette);
  openPalette();
  await tick();
}

/** The combobox input (the only labeled one in the overlay). */
function searchInput(): HTMLInputElement {
  return screen.getByLabelText("Search commands") as HTMLInputElement;
}

beforeEach(() => {
  __resetKeybindsForTests(localStorage);
  resetRecents(null);
  resetTabStore();
  seedTab();
});

afterEach(() => {
  closePalette();
  cleanup();
  resetRecents(null);
  resetTabStore();
  __resetKeybindsForTests();
});

describe("CommandPalette", () => {
  it("renders nothing while closed, then opens via the store", async () => {
    render(CommandPalette);
    await tick();
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();

    openPalette();
    await tick();
    expect(screen.getByRole("dialog", { name: "Command palette" })).toBeTruthy();
    // The combobox carries the active tab in its placeholder and has focus.
    const input = searchInput();
    expect(input.placeholder).toBe("Type a command… (r1)");
    expect(document.activeElement).toBe(input);
  });

  it("filters commands per keystroke (fuzzy scorer behavior)", async () => {
    await openRendered();
    await fireEvent.input(searchInput(), { target: { value: "stash" } });
    // Titles render as per-char spans, so match on the list's text content.
    // ("stash" is a fuzzy subsequence match — it may legitimately pull in
    // commands whose section/keywords contain the letters, e.g. "Stage all
    // changes"; what must disappear is everything without a subsequence.)
    const list = document.getElementById("palette-list")!;
    expect(list.textContent).toContain("Stash changes");
    expect(list.textContent).not.toContain("Refresh repository status");

    // A non-subsequence query empties the list into its no-match state.
    await fireEvent.input(searchInput(), { target: { value: "zzz-none" } });
    expect(screen.getByText(/No commands match/)).toBeTruthy();
  });

  it("highlights the matched title positions", async () => {
    await openRendered();
    // "pop latest stash" matches exactly one command; its title chars all
    // carry the .hl mark.
    await fireEvent.input(searchInput(), { target: { value: "pop latest stash" } });
    const options = [...document.querySelectorAll(".option")];
    expect(options.length).toBe(1);
    const label = options[0]!.querySelector(".label")!;
    const hits = [...label.querySelectorAll(".hl")];
    expect(hits.length).toBe("pop latest stash".length);
    expect(hits.map((hit) => hit.textContent).join("")).toBe("Pop latest stash");
  });

  it("Enter runs the top-ranked command and closes", async () => {
    await openRendered();
    vi.mocked(stageAll).mockResolvedValueOnce(undefined);
    await fireEvent.input(searchInput(), { target: { value: "stage all" } });
    await fireEvent.keyDown(searchInput(), { key: "Enter" });
    await waitFor(() =>
      expect(stageAll).toHaveBeenCalledWith("r1", false),
    );
    expect(isPaletteOpen()).toBe(false);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("arrow keys move the active option", async () => {
    await openRendered();
    const input = searchInput();
    expect(input.getAttribute("aria-activedescendant")).toBe("cmd-opt-0");
    await fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(input.getAttribute("aria-activedescendant")).toBe("cmd-opt-1");
    const active = document.getElementById("cmd-opt-1")!;
    expect(active.getAttribute("aria-selected")).toBe("true");
    await fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(input.getAttribute("aria-activedescendant")).toBe("cmd-opt-0");
  });

  it("Escape closes and returns focus to the trigger element", async () => {
    render(CommandPalette);
    const trigger = document.createElement("button");
    trigger.textContent = "opener";
    document.body.append(trigger);
    trigger.focus();

    openPalette();
    await tick();
    expect(document.activeElement).toBe(searchInput());

    await fireEvent.keyDown(searchInput(), { key: "Escape" });
    await tick();
    expect(isPaletteOpen()).toBe(false);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    trigger.remove();
  });

  it("shows a Recent section with the last-run command on reopen", async () => {
    await openRendered();
    vi.mocked(stageAll).mockResolvedValueOnce(undefined);
    await fireEvent.input(searchInput(), { target: { value: "stage all" } });
    await fireEvent.keyDown(searchInput(), { key: "Enter" });
    await waitFor(() => expect(isPaletteOpen()).toBe(false));

    // Reopen with an empty query: the Recent block sits on top.
    openPalette();
    await tick();
    const list = document.getElementById("palette-list")!;
    const firstSection = list.querySelector("li.section");
    expect(firstSection?.textContent).toBe("Recent");
    expect(list.textContent).toContain("Stage all changes");
  });
});
