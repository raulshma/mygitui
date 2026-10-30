/**
 * BranchSwitcher tests: the popover opens from the header chip and lists
 * local + remote branches (BranchPanel row language, tracked remotes
 * hidden), the search filters both sections and offers the VS Code style
 * create row (clicking it calls the shared controller), local rows switch
 * through the controller, remote rows check out remotely, and Escape
 * closes and restores trigger focus. The controller factory is asked for
 * per-repoId (a pinned controller would mutate the wrong repo after an
 * un-keyed repo-tab switch), the typed `branch-switcher-open` event opens
 * for matching/any-repo targets and ignores foreign ones, and reopening
 * refreshes the lists. The BranchOpsController and the IPC layer are
 * mocked, so the suite stays hermetic even before/without the real
 * controller module.
 */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  BranchInfo,
  RemoteBranchInfo,
  RepoStatus,
} from "$lib/ipc/types";

const mocks = vi.hoisted(() => {
  // Fake BranchOpsController matching the controller's surface the
  // switcher consumes; dialogs never open because the dialog states stay
  // null and `busy` stays false (BranchActionDialogs renders nothing).
  const controller = {
    switchChoice: null,
    confirmDelete: null,
    previewDelete: null,
    busy: false,
    setList: vi.fn(),
    setReload: vi.fn(),
    headBranch: vi.fn(() => null),
    create: vi.fn(async () => true),
    checkoutRemote: vi.fn(async () => "next"),
    requestSwitch: vi.fn(async () => {}),
    chooseStash: vi.fn(async () => {}),
    chooseForce: vi.fn(async () => {}),
    cancelChoice: vi.fn(),
    requestDelete: vi.fn(async () => {}),
    confirmMergedDelete: vi.fn(async () => {}),
    confirmForceDelete: vi.fn(async () => {}),
    cancelDelete: vi.fn(),
    rename: vi.fn(async () => {}),
    push: vi.fn(async () => {}),
  };
  return {
    controller,
    // Records every factory call (repoId + onMutated) while handing back
    // the singleton, so existing per-method assertions keep working AND a
    // repoId-pin regression (factory asked only once) is observable.
    branchOpsController: vi.fn(
      (_repoId: string, _onMutated?: () => void) => controller,
    ),
    branches: vi.fn(),
    remoteBranches: vi.fn(),
    pushRepo: vi.fn(),
    fetchDefaultRemote: vi.fn(),
  };
});

vi.mock("$lib/ipc/client", () => ({
  branches: mocks.branches,
  remoteBranches: mocks.remoteBranches,
  pushRepo: mocks.pushRepo,
}));

vi.mock("$lib/git/branchOps.svelte", () => ({
  branchOpsController: mocks.branchOpsController,
}));

vi.mock("$lib/git/sync", () => ({
  fetchDefaultRemote: mocks.fetchDefaultRemote,
  syncSkipMessage: (reason: string, verb: string) =>
    `skipped ${reason} ${verb}`,
  syncSuccessMessage: (
    verb: string,
    outcome: { remote: string },
  ) => `done ${verb} ${outcome.remote}`,
}));

vi.mock("$lib/toast", () => ({
  toast: vi.fn(),
  dismissToast: vi.fn(),
  pauseToast: vi.fn(),
  resumeToast: vi.fn(),
  getToasts: vi.fn(() => []),
}));

import BranchSwitcher from "./BranchSwitcher.svelte";

function branch(patch: Partial<BranchInfo> = {}): BranchInfo {
  return {
    name: "feature",
    sha: "bbbb2223334445566",
    upstream: null,
    ahead: 0,
    behind: 0,
    gone: false,
    is_head: false,
    ...patch,
  };
}

function remoteBranch(patch: Partial<RemoteBranchInfo> = {}): RemoteBranchInfo {
  return {
    remote: "origin",
    name: "next",
    sha: "cccc3334445556667",
    tracked_by: null,
    ...patch,
  };
}

function status(patch: Partial<RepoStatus> = {}): RepoStatus {
  return {
    branch: "main",
    head: "aaaa1112223334445",
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

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  cleanup();
});

function openSwitcher(): Promise<HTMLElement> {
  return (async () => {
    await fireEvent.click(
      screen.getByRole("button", { name: "Switch branch" }),
    );
    return await screen.findByRole("dialog", { name: "Branch switcher" });
  })();
}

describe("BranchSwitcher", () => {
  it("opens from the header chip and lists local + remote branches", async () => {
    mocks.branches.mockResolvedValue([
      branch({ name: "main", sha: "aaaa1112223334445", is_head: true }),
      branch({ name: "feature" }),
    ]);
    mocks.remoteBranches.mockResolvedValue([remoteBranch()]);

    render(BranchSwitcher, { repoId: "r1", status: status() });
    const dialog = await openSwitcher();

    // Both sections render with the BranchPanel row language: mono names,
    // short shas, and the HEAD dot row.
    expect(within(dialog).getByText("Local branches")).not.toBeNull();
    expect(within(dialog).getByText("Remote branches")).not.toBeNull();
    expect(within(dialog).getByText("main")).not.toBeNull();
    expect(within(dialog).getByText("feature")).not.toBeNull();
    expect(within(dialog).getByText("aaaa111")).not.toBeNull();
    expect(within(dialog).getByText("origin/next")).not.toBeNull();
    expect(within(dialog).getByText("cccc333")).not.toBeNull();
    expect(within(dialog).getByTitle("Current branch")).not.toBeNull();
  });

  it("filters both sections as the query is typed", async () => {
    mocks.branches.mockResolvedValue([
      branch({ name: "main", is_head: true }),
      branch({ name: "feature" }),
    ]);
    mocks.remoteBranches.mockResolvedValue([remoteBranch()]);

    render(BranchSwitcher, { repoId: "r1", status: status() });
    const dialog = await openSwitcher();

    const input = screen.getByPlaceholderText("Search or create branch");
    await fireEvent.input(input, { target: { value: "fea" } });

    expect(within(dialog).queryByText("main")).toBeNull();
    expect(within(dialog).getByText("feature")).not.toBeNull();
    expect(within(dialog).queryByText("origin/next")).toBeNull();
  });

  it("offers a create row for non-matching queries and creates on click", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([]);

    render(BranchSwitcher, { repoId: "r1", status: status() });
    const dialog = await openSwitcher();

    const input = screen.getByPlaceholderText("Search or create branch");
    await fireEvent.input(input, { target: { value: "newbr" } });

    await fireEvent.click(
      await within(dialog).findByText('Create branch "newbr"'),
    );

    await waitFor(() =>
      expect(mocks.controller.create).toHaveBeenCalledWith("newbr"),
    );
    // Success closes the popover.
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Branch switcher" }),
      ).toBeNull(),
    );
  });

  it("switches through the controller when a local row is clicked", async () => {
    mocks.branches.mockResolvedValue([
      branch({ name: "main", is_head: true }),
      branch({ name: "feature" }),
    ]);
    mocks.remoteBranches.mockResolvedValue([]);

    render(BranchSwitcher, { repoId: "r1", status: status() });
    const dialog = await openSwitcher();

    await fireEvent.click(within(dialog).getByText("feature"));

    await waitFor(() =>
      expect(mocks.controller.requestSwitch).toHaveBeenCalledWith("feature"),
    );
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Branch switcher" }),
      ).toBeNull(),
    );
  });

  it("checks out a remote branch through the controller", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([remoteBranch()]);

    render(BranchSwitcher, { repoId: "r1", status: status() });
    const dialog = await openSwitcher();

    await fireEvent.click(within(dialog).getByText("origin/next"));

    await waitFor(() =>
      expect(mocks.controller.checkoutRemote).toHaveBeenCalledWith(
        "origin",
        "next",
      ),
    );
    // The popover closes right away; success feedback is the controller's
    // (it toasts + reloads internally).
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Branch switcher" }),
      ).toBeNull(),
    );
  });

  it("hides remote branches a local branch already tracks", async () => {
    mocks.branches.mockResolvedValue([
      branch({ name: "main", is_head: true, upstream: "origin/main" }),
    ]);
    mocks.remoteBranches.mockResolvedValue([
      remoteBranch({
        name: "main",
        sha: "dddd4445556667778",
        tracked_by: "main",
      }),
      remoteBranch({ name: "lonely", sha: "eeee5556667778889" }),
    ]);

    render(BranchSwitcher, { repoId: "r1", status: status() });
    const dialog = await openSwitcher();

    expect(within(dialog).queryByText("dddd444")).toBeNull();
    expect(within(dialog).getByText("eeee555")).not.toBeNull();
  });

  it("closes on Escape and returns focus to the trigger", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([]);

    render(BranchSwitcher, { repoId: "r1", status: status() });
    await openSwitcher();

    const input = screen.getByPlaceholderText("Search or create branch");
    await fireEvent.keyDown(input, { key: "Escape" });

    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Branch switcher" }),
      ).toBeNull(),
    );
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Switch branch" }),
    );
  });

  it("asks the controller factory per repoId when the prop changes", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([]);

    // RepoView is mounted un-keyed, so a repo-tab switch changes the
    // repoId prop on the SAME instance — the controller must be recreated
    // for the new repository (a pinned one would mutate the old repo).
    // `onMutated` is omitted here, so factory calls are [repoId, undefined].
    const { rerender } = render(BranchSwitcher, {
      repoId: "r1",
      status: status(),
    });
    await rerender({ repoId: "r2", status: status() });

    expect(
      mocks.branchOpsController.mock.calls.map((call) => call[0]),
    ).toEqual(["r1", "r2"]);
  });

  it("opens for matching branch-switcher-open events and ignores foreign repos", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([]);

    render(BranchSwitcher, { repoId: "r1", status: status() });

    // Another repository's event must not open the popover.
    document.dispatchEvent(
      new CustomEvent("branch-switcher-open", { detail: { repoId: "r2" } }),
    );
    expect(
      screen.queryByRole("dialog", { name: "Branch switcher" }),
    ).toBeNull();

    // Matching repo opens…
    document.dispatchEvent(
      new CustomEvent("branch-switcher-open", { detail: { repoId: "r1" } }),
    );
    expect(
      await screen.findByRole("dialog", { name: "Branch switcher" }),
    ).not.toBeNull();

    // …and an undefined target ("the active repo") opens too.
    await fireEvent.keyDown(
      screen.getByPlaceholderText("Search or create branch"),
      { key: "Escape" },
    );
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Branch switcher" }),
      ).toBeNull(),
    );
    document.dispatchEvent(
      new CustomEvent("branch-switcher-open", { detail: {} }),
    );
    expect(
      await screen.findByRole("dialog", { name: "Branch switcher" }),
    ).not.toBeNull();
  });

  it("refreshes the branch lists on every open", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([]);

    render(BranchSwitcher, { repoId: "r1", status: status() });
    await openSwitcher();

    const input = screen.getByPlaceholderText("Search or create branch");
    await fireEvent.keyDown(input, { key: "Escape" });
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Branch switcher" }),
      ).toBeNull(),
    );

    // Branches created elsewhere (BranchPanel, terminal) after the first
    // open must show up when the popover reopens.
    mocks.branches.mockResolvedValue([
      branch({ name: "main", is_head: true }),
      branch({ name: "added-elsewhere" }),
    ]);
    const dialog = await openSwitcher();
    expect(
      await within(dialog).findByText("added-elsewhere"),
    ).not.toBeNull();
    expect(mocks.branches).toHaveBeenCalledTimes(2);
  });
});
