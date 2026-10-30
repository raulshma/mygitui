/**
 * RepoQuickActions tests: the Sync split button runs the contextual verb
 * (pull when behind, push when ahead, fetch otherwise) with its accessible
 * name carrying branch + counts, success refreshes the owner, preconditions
 * disable (never toast-and-run), and ops started elsewhere disable/spin the
 * group. The header pins: the chevron menu's trailing pin action pins/unpins
 * verbs (via the shared context menu), pinned verbs render as icon segments
 * in the group, clicking one runs the shared verb, right-click unpins, and
 * segments disable without a current branch or while a sync runs.
 */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { NetStats, RepoStatus } from "$lib/ipc/types";
import { opStore } from "$lib/stores/ops.svelte";
import { contextMenu } from "$lib/components/menu/contextMenuStore.svelte";
import ContextMenu from "$lib/components/menu/ContextMenu.svelte";
import { headerPins } from "$lib/stores/headerPins.svelte";

const mocks = vi.hoisted(() => ({
  fetchDefaultRemote: vi.fn(),
  pullCurrentBranch: vi.fn(),
  pushCurrentBranch: vi.fn(),
}));

vi.mock("$lib/git/sync", () => ({
  fetchDefaultRemote: mocks.fetchDefaultRemote,
  pullCurrentBranch: mocks.pullCurrentBranch,
  pushCurrentBranch: mocks.pushCurrentBranch,
  syncSkipMessage: (reason: string, verb: string) =>
    `skipped ${reason} ${verb}`,
  syncSuccessMessage: (
    verb: string,
    outcome: { remote: string },
    branch: string | null,
  ) => `done ${verb} ${outcome.remote} ${branch ?? ""}`,
}));

vi.mock("$lib/toast", () => ({
  toast: vi.fn(),
  dismissToast: vi.fn(),
  pauseToast: vi.fn(),
  resumeToast: vi.fn(),
  getToasts: vi.fn(() => []),
}));

import { toast } from "$lib/toast";
import RepoQuickActions from "./RepoQuickActions.svelte";

const STATS: NetStats = {
  received_bytes: 0,
  objects: 0,
  updated_refs: [],
};

function status(patch: Partial<RepoStatus> = {}): RepoStatus {
  return {
    branch: "main",
    head: "abcdef1234567890",
    detached: false,
    ahead: 2,
    behind: 1,
    merging: false,
    rebasing: false,
    sequencer: false,
    entries: [],
    ...patch,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  opStore.ops = {};
});

afterEach(() => {
  cleanup();
  contextMenu.hide();
  headerPins.clear();
  opStore.ops = {};
});

function button(name: string | RegExp): HTMLButtonElement {
  return screen.getByRole("button", { name }) as HTMLButtonElement;
}

function pinButton(verb: string): HTMLButtonElement {
  const found = document.querySelector<HTMLButtonElement>(
    `[data-sync-pin="${verb}"]`,
  );
  expect(found).not.toBeNull();
  return found!;
}

/** Opens the chevron menu (rendering ContextMenu alongside is required). */
async function openMenu(): Promise<void> {
  await fireEvent.click(button("More sync actions"));
  expect(contextMenu.open).toBe(true);
}

describe("RepoQuickActions header pins", () => {
  it("pins a verb from the menu's trailing pin action", async () => {
    render(RepoQuickActions, { repoId: "r1", status: status() });
    render(ContextMenu);

    await openMenu();
    // Entries render in fetch/pull/push order — the first pin action pins Fetch.
    await fireEvent.click(screen.getAllByTitle("Pin to header")[0]!);
    expect(headerPins.pins).toEqual(["fetch"]);
    expect(contextMenu.open).toBe(false);
    expect(pinButton("fetch")).not.toBeNull();
  });

  it("the menu action flips to Unpin for pinned verbs", async () => {
    headerPins.pin("pull");
    render(RepoQuickActions, { repoId: "r1", status: status() });
    render(ContextMenu);

    await openMenu();
    expect(screen.getByTitle("Unpin from header")).not.toBeNull();
    // The other two verbs still offer pinning.
    expect(screen.getAllByTitle("Pin to header")).toHaveLength(2);
  });

  it("renders pinned verbs as segments in fetch/pull/push order", () => {
    headerPins.pin("push");
    headerPins.pin("fetch");
    render(RepoQuickActions, { repoId: "r1", status: status() });

    // Pin order is push→fetch, but the row never reshuffles.
    const verbs = [
      ...document.querySelectorAll("[data-sync-pin]"),
    ].map((el) => el.getAttribute("data-sync-pin"));
    expect(verbs).toEqual(["fetch", "push"]);
  });

  it("a pinned segment runs the verb and reports success", async () => {
    mocks.pullCurrentBranch.mockResolvedValue({
      kind: "done",
      remote: "origin",
      stats: STATS,
    });
    const onMutated = vi.fn();

    headerPins.pin("pull");
    render(RepoQuickActions, { repoId: "r1", status: status(), onMutated });
    render(ContextMenu);

    await fireEvent.click(pinButton("pull"));

    await waitFor(() =>
      expect(mocks.pullCurrentBranch).toHaveBeenCalledWith("r1", "main"),
    );
    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith("done pull origin main", {
        kind: "success",
      }),
    );
    expect(onMutated).toHaveBeenCalledTimes(1);
  });

  it("unpins via the pinned segment's right-click menu", async () => {
    headerPins.pin("push");
    render(RepoQuickActions, { repoId: "r1", status: status() });
    render(ContextMenu);

    await fireEvent.contextMenu(pinButton("push"));
    expect(contextMenu.open).toBe(true);
    await fireEvent.click(screen.getByText("Unpin from header"));
    expect(headerPins.pins).toEqual([]);
    expect(document.querySelector('[data-sync-pin="push"]')).toBeNull();
  });

  it("a pinned pull disables without a current branch; fetch stays", () => {
    headerPins.pin("pull");
    headerPins.pin("fetch");
    render(RepoQuickActions, {
      repoId: "r1",
      status: status({ branch: null, detached: true, ahead: 0, behind: 0 }),
    });

    // Fetch works branchless; pull/push need a current branch.
    expect(pinButton("fetch").disabled).toBe(false);
    expect(pinButton("pull").disabled).toBe(true);
  });

  it("segments disable and announce progress while their verb runs", async () => {
    let resolveFetch: (value: {
      kind: "done";
      remote: string;
      stats: NetStats;
    }) => void = () => {};
    mocks.fetchDefaultRemote.mockReturnValue(
      new Promise((resolve) => (resolveFetch = resolve)),
    );

    headerPins.pin("fetch");
    render(RepoQuickActions, { repoId: "r1", status: status() });

    await fireEvent.click(pinButton("fetch"));
    // In-flight: busy + the segment announces the running verb.
    expect(pinButton("fetch").disabled).toBe(true);
    expect(pinButton("fetch").getAttribute("aria-busy")).toBe("true");

    resolveFetch({ kind: "done", remote: "origin", stats: STATS });
    await waitFor(() =>
      expect(pinButton("fetch").getAttribute("aria-busy")).toBe("false"),
    );
    expect(pinButton("fetch").disabled).toBe(false);
  });
});

describe("RepoQuickActions", () => {
  it("renders one Sync split button; behind wins the contextual verb", () => {
    render(RepoQuickActions, { repoId: "r1", status: status() });

    expect(
      screen.getByRole("group", { name: "Git sync actions" }),
    ).not.toBeNull();
    expect(
      button("Sync with remote (pull main, 1 behind)").disabled,
    ).toBe(false);
    expect(button("More sync actions").disabled).toBe(false);
  });

  it("runs the contextual push and refreshes the repo on success", async () => {
    mocks.pushCurrentBranch.mockResolvedValue({
      kind: "done",
      remote: "origin",
      stats: STATS,
    });
    const onMutated = vi.fn();

    render(RepoQuickActions, {
      repoId: "r1",
      status: status({ behind: 0 }),
      onMutated,
    });

    await fireEvent.click(button("Sync with remote (push main, 2 ahead)"));

    await waitFor(() =>
      expect(mocks.pushCurrentBranch).toHaveBeenCalledWith("r1", "main"),
    );
    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith("done push origin main", {
        kind: "success",
      }),
    );
    expect(onMutated).toHaveBeenCalledTimes(1);
  });

  it("reports a no-remote skip instead of calling the network", async () => {
    mocks.fetchDefaultRemote.mockResolvedValue({
      kind: "skipped",
      reason: "no-remote",
    });
    const onMutated = vi.fn();

    render(RepoQuickActions, {
      repoId: "r1",
      status: status({ ahead: 0, behind: 0 }),
      onMutated,
    });

    await fireEvent.click(button("Sync with remote (fetch)"));

    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith("skipped no-remote fetch", {
        kind: "error",
      }),
    );
    expect(onMutated).not.toHaveBeenCalled();
  });

  it("toasts backend failures", async () => {
    mocks.fetchDefaultRemote.mockRejectedValue(new Error("boom"));

    render(RepoQuickActions, {
      repoId: "r1",
      status: status({ ahead: 0, behind: 0 }),
    });

    await fireEvent.click(button("Sync with remote (fetch)"));

    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith("Fetch failed: boom", {
        kind: "error",
      }),
    );
  });

  it("keeps fetch (the contextual verb) available on a detached HEAD", () => {
    render(RepoQuickActions, {
      repoId: "r1",
      status: status({ branch: null, detached: true, ahead: 0, behind: 0 }),
    });

    expect(button("Sync with remote (fetch)").disabled).toBe(false);
    expect(button("More sync actions").disabled).toBe(false);
  });

  it("disables the main segment when the contextual verb needs a branch", () => {
    render(RepoQuickActions, {
      repoId: "r1",
      status: status({ branch: null, detached: true, ahead: 0, behind: 1 }),
    });

    expect(
      button("Sync with remote (pull current branch, 1 behind)").disabled,
    ).toBe(true);
  });

  it("disables the group while another surface's sync op runs", () => {
    opStore.ops = {
      r1: [
        {
          op_id: "op-1",
          kind: "push",
          message: "pushing main to origin",
          pct: 40,
          done: false,
          error: null,
        },
      ],
    };

    render(RepoQuickActions, { repoId: "r1", status: status() });

    expect(
      screen
        .getByRole("group", { name: "Git sync actions" })
        .getAttribute("aria-busy"),
    ).toBe("true");
    // The main segment announces the running verb while busy.
    const main = button("Push in progress…");
    expect(main.disabled).toBe(true);
    expect(main.getAttribute("aria-busy")).toBe("true");
    expect(button("More sync actions").disabled).toBe(true);
  });
});
