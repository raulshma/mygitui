/**
 * BranchPanel tests: both sections render (locals with the pinned HEAD row,
 * remotes grouped per remote with tracking info), the filter spans both
 * sections, the toolbar Fetch runs the shared fetch verb and reloads both
 * lists (skip = no reload), a remote row's checkout goes through the
 * controller's dirty-gated requestCheckoutRemote, and the remote delete
 * flows from the row menu through the ConfirmDialog into `remoteBranchDelete`
 * + reload. The show-more expander caps each section at the model's page
 * size. The controller factory and the IPC layer are mocked (same seam as
 * the BranchSwitcher / WorktreePanel suites).
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
import type { BranchInfo, RemoteBranchInfo } from "$lib/ipc/types";

const mocks = vi.hoisted(() => {
  // Fake BranchOpsController matching the surface the panel consumes;
  // dialogs never open because the dialog states stay closed (busy false).
  const controller = {
    switchChoice: { open: false },
    confirmDelete: { open: false },
    previewDelete: { open: false },
    busy: false,
    setList: vi.fn(),
    setReload: vi.fn(),
    headBranch: vi.fn(() => null),
    create: vi.fn(async () => true),
    checkoutRemote: vi.fn(async () => "next"),
    requestCheckoutRemote: vi.fn(async () => {}),
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
    branchOpsController: vi.fn(
      (_repoId: string, _onMutated?: () => void) => controller,
    ),
    remoteBranchDelete: vi.fn(async () => true),
    branches: vi.fn(),
    branchCreate: vi.fn(),
    branchDelete: vi.fn(),
    branchIsMerged: vi.fn(),
    remoteBranches: vi.fn(),
    remotes: vi.fn(),
    fetchRepo: vi.fn(),
    fetchDefaultRemote: vi.fn(),
    // Filled by the transitive dialog/section components; never called here.
    archiveSpec: vi.fn(),
    guardCheckpoint: vi.fn(),
    mergeBranch: vi.fn(),
    opsPreview: vi.fn(),
    pickSaveFile: vi.fn(),
    pushRepo: vi.fn(),
    repoRefs: vi.fn(),
    repoStatus: vi.fn(),
    resetRepo: vi.fn(),
    tagCreate: vi.fn(),
    tagCreateSigned: vi.fn(),
    tagDelete: vi.fn(),
    tagList: vi.fn(),
  };
});

vi.mock("$lib/ipc/client", () => ({ ...mocks }));

vi.mock("$lib/git/branchOps.svelte", () => ({
  branchOpsController: mocks.branchOpsController,
  remoteBranchDelete: mocks.remoteBranchDelete,
}));

vi.mock("$lib/git/sync", () => ({
  fetchDefaultRemote: mocks.fetchDefaultRemote,
  fetchOptions: (remote: string, prune: boolean) => ({
    remote,
    prune,
    refs: [],
    depth: null,
  }),
  netStatsSummary: () => "2 refs updated",
  syncSkipMessage: (reason: string, verb: string) => `skip ${reason} ${verb}`,
  syncSuccessMessage: (verb: string, outcome: { remote: string }) =>
    `done ${verb} ${outcome.remote}`,
}));

vi.mock("$lib/toast", () => ({
  toast: vi.fn(),
  dismissToast: vi.fn(),
  pauseToast: vi.fn(),
  resumeToast: vi.fn(),
  getToasts: vi.fn(() => []),
}));

import { toast } from "$lib/toast";
import { contextMenu } from "$lib/components/menu/contextMenuStore.svelte";
import BranchPanel from "./BranchPanel.svelte";

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

beforeEach(() => {
  vi.clearAllMocks();
  mocks.branches.mockResolvedValue([]);
  mocks.remoteBranches.mockResolvedValue([]);
  mocks.remotes.mockResolvedValue([{ name: "origin", url: "u", push_url: null }]);
});

afterEach(() => {
  cleanup();
  contextMenu.hide();
});

function renderPanel(onMutated?: () => void): void {
  render(BranchPanel, { repoId: "r1", ...(onMutated ? { onMutated } : {}) });
}

describe("BranchPanel sections", () => {
  it("renders local rows (HEAD pinned) and grouped remote rows with tracking info", async () => {
    mocks.branches.mockResolvedValue([
      branch({ name: "feature" }),
      branch({ name: "main", is_head: true, upstream: "origin/main", ahead: 1 }),
    ]);
    mocks.remoteBranches.mockResolvedValue([
      remoteBranch({ name: "main", tracked_by: "main" }),
      remoteBranch({ name: "feature", tracked_by: "feature" }),
      remoteBranch({ name: "next" }),
    ]);

    renderPanel();
    // Branch names also appear in the create form's "from" select, so
    // wait on the section header and scope row queries to their list.
    await screen.findByText("Local (2)");

    // Section headers carry the counts.
    expect(screen.getByText("Remote (3)")).not.toBeNull();

    // Local rows in one list; HEAD first (pinned), then alphabetical.
    const localList = screen.getByRole("list", { name: "Local branches" });
    const localNames = within(localList).getAllByText(/^(main|feature)$/);
    expect(localNames[0].textContent).toBe("main");
    expect(within(localList).getByText("feature")).not.toBeNull();

    // Remote rows show the SHORT name (group header carries the remote).
    const originGroup = screen.getByRole("list", {
      name: "origin remote branches",
    });
    expect(within(originGroup).getByText("main")).not.toBeNull();
    expect(within(originGroup).getByText("next")).not.toBeNull();
    // Tracked row shows the tracking local + the checked-out badge; the
    // untracked row says so plainly.
    expect(within(originGroup).getByText("→ main")).not.toBeNull();
    expect(within(originGroup).getByText("not checked out")).not.toBeNull();
    // Checked-out remote: HEAD chip, no checkout button; a remote tracked by
    // a NON-current local offers a switch to it; untracked offers checkout.
    expect(within(originGroup).queryByLabelText("Checkout origin/main")).toBeNull();
    expect(
      within(originGroup).getByLabelText("Checkout origin/next"),
    ).not.toBeNull();
    expect(within(originGroup).getByLabelText("Switch to feature")).not.toBeNull();
    // Ahead/behind of the tracking local branch is surfaced on the remote row.
    expect(within(originGroup).getByText("↑1 ↓0")).not.toBeNull();
  });

  it("activates a tracked remote row as a switch to its local branch", async () => {
    mocks.branches.mockResolvedValue([
      branch({ name: "main", is_head: true }),
      branch({ name: "feature" }),
    ]);
    mocks.remoteBranches.mockResolvedValue([
      remoteBranch({ name: "feature", tracked_by: "feature" }),
    ]);

    renderPanel();
    const originGroup = await screen.findByRole("list", {
      name: "origin remote branches",
    });
    await fireEvent.click(
      within(originGroup).getByLabelText("Switch to feature"),
    );

    await waitFor(() =>
      expect(mocks.controller.requestSwitch).toHaveBeenCalledWith("feature"),
    );
    expect(mocks.controller.requestCheckoutRemote).not.toHaveBeenCalled();
  });

  it("applies one filter across both sections", async () => {
    mocks.branches.mockResolvedValue([
      branch({ name: "main", is_head: true }),
      branch({ name: "feature" }),
    ]);
    mocks.remoteBranches.mockResolvedValue([
      remoteBranch({ name: "next" }),
      remoteBranch({ remote: "fork", name: "feature-x" }),
    ]);

    renderPanel();
    await screen.findByText("Local (2)");

    await fireEvent.input(screen.getByLabelText("Filter branches"), {
      target: { value: "feat" },
    });

    const localList = screen.getByRole("list", { name: "Local branches" });
    expect(within(localList).queryByText("main")).toBeNull();
    expect(within(localList).getByText("feature")).not.toBeNull();
    // Remote rows match on remote/name; only fork/feature-x survives.
    expect(screen.queryByLabelText("Checkout origin/next")).toBeNull();
    expect(
      screen.getByLabelText("Checkout fork/feature-x"),
    ).not.toBeNull();
  });
});

describe("BranchPanel empty states", () => {
  it("guides when the repo has neither local nor remote branches", async () => {
    renderPanel();
    await screen.findByText("No branches.");
    expect(
      screen.getByText(
        "Create a branch above, or fetch to discover remote branches.",
      ),
    ).not.toBeNull();
  });

  it("hints fetch when remotes exist but no remote branch was discovered", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([]);

    renderPanel();
    await screen.findByText("Local (1)");
    expect(
      screen.getByText("No remote branches — fetch to discover them."),
    ).not.toBeNull();
  });

  it("points at the Remotes panel when no remote is configured", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([]);
    mocks.remotes.mockResolvedValue([]);

    renderPanel();
    await screen.findByText("Local (1)");
    expect(
      screen.getByText("No remotes configured — add one in the Remotes panel."),
    ).not.toBeNull();
  });
});

describe("BranchPanel toolbar fetch", () => {
  it("fetches the primary remote and reloads both lists on success", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([remoteBranch({ name: "next" })]);
    mocks.fetchDefaultRemote.mockResolvedValue({
      kind: "done",
      remote: "origin",
      stats: { received_bytes: 0, objects: 0, updated_refs: [] },
    });

    renderPanel();
    await screen.findByText("Local (1)");
    expect(mocks.branches).toHaveBeenCalledTimes(1);

    await fireEvent.click(
      screen.getByRole("button", { name: "Fetch remote branches" }),
    );

    await waitFor(() => expect(mocks.fetchDefaultRemote).toHaveBeenCalledWith("r1"));
    // Reload: both lists fetched again, success toast.
    await waitFor(() => expect(mocks.branches).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith("done fetch origin", { kind: "success" }),
    );
  });

  it("toasts the skip reason and does not reload when no remote is configured", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([]);
    mocks.fetchDefaultRemote.mockResolvedValue({
      kind: "skipped",
      reason: "no-remote",
    });

    renderPanel();
    await screen.findByText("Local (1)");

    await fireEvent.click(
      screen.getByRole("button", { name: "Fetch remote branches" }),
    );

    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith("skip no-remote fetch", {
        kind: "error",
      }),
    );
    await waitFor(() => expect(mocks.branches).toHaveBeenCalledTimes(1));
  });

  it("fetches a single remote from its group header (prune on)", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([
      remoteBranch({ name: "next" }),
      remoteBranch({ remote: "fork", name: "x" }),
    ]);

    renderPanel();
    await screen.findByText("next");

    await fireEvent.click(screen.getByRole("button", { name: "Fetch fork" }));

    await waitFor(() =>
      expect(mocks.fetchRepo).toHaveBeenCalledWith("r1", {
        remote: "fork",
        prune: true,
        refs: [],
        depth: null,
      }),
    );
    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith("Fetched fork (2 refs updated)", {
        kind: "success",
      }),
    );
    await waitFor(() => expect(mocks.branches).toHaveBeenCalledTimes(2));
  });
});

describe("BranchPanel remote actions", () => {
  it("checks a remote branch out through the controller's gated request", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([remoteBranch({ name: "next" })]);

    renderPanel();
    const row = await screen.findByLabelText("Checkout origin/next");
    await fireEvent.click(row);

    await waitFor(() =>
      expect(mocks.controller.requestCheckoutRemote).toHaveBeenCalledWith(
        "origin",
        "next",
      ),
    );
  });

  it("deletes a remote branch via the row menu + confirm dialog", async () => {
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue([remoteBranch({ name: "next" })]);
    const onMutated = vi.fn();

    renderPanel(onMutated);
    await screen.findByText("next");

    await fireEvent.click(
      screen.getByLabelText("More actions for origin/next"),
    );
    // The context menu store holds the entries; run the destructive one.
    const entries = contextMenu.entries;
    const del = entries[entries.length - 1];
    expect(del).toMatchObject({ id: "delete", label: "Delete remote branch…" });
    (del as { run: () => void }).run();

    const confirm = await screen.findByRole("button", {
      name: "Delete remote branch",
    });
    await fireEvent.click(confirm);

    await waitFor(() =>
      expect(mocks.remoteBranchDelete).toHaveBeenCalledWith(
        "r1",
        "origin",
        "next",
      ),
    );
    // Success refreshes both lists + notifies the owner.
    await waitFor(() => expect(mocks.branches).toHaveBeenCalledTimes(2));
    expect(onMutated).toHaveBeenCalled();
  });
});

describe("BranchPanel show-more guard", () => {
  it("renders the first page of locals behind a 'Show all' expander", async () => {
    const many = Array.from({ length: 210 }, (_, i) =>
      branch({ name: `b${String(i).padStart(3, "0")}` }),
    );
    mocks.branches.mockResolvedValue(many);

    renderPanel();
    const localList = await screen.findByRole("list", {
      name: "Local branches",
    });
    // 200 rows + the expander row.
    expect(within(localList).getAllByRole("listitem")).toHaveLength(201);
    expect(
      within(localList).getByText("Show all 210 local branches"),
    ).not.toBeNull();

    await fireEvent.click(
      within(localList).getByText("Show all 210 local branches"),
    );
    await waitFor(() =>
      expect(within(localList).getAllByRole("listitem")).toHaveLength(210),
    );
    expect(within(localList).queryByText(/Show all/)).toBeNull();
  });

  it("expands a remote group past the page size too", async () => {
    const many = Array.from({ length: 205 }, (_, i) =>
      remoteBranch({ name: `r${String(i).padStart(3, "0")}` }),
    );
    mocks.branches.mockResolvedValue([branch({ name: "main", is_head: true })]);
    mocks.remoteBranches.mockResolvedValue(many);

    renderPanel();
    const originList = await screen.findByRole("list", {
      name: "origin remote branches",
    });
    expect(within(originList).getAllByRole("listitem")).toHaveLength(201);

    await fireEvent.click(
      within(originList).getByText("Show all 205 origin branches"),
    );
    await waitFor(() =>
      expect(within(originList).getAllByRole("listitem")).toHaveLength(205),
    );
  });
});

describe("BranchPanel stale-response guard", () => {
  it("drops a branch list that lands after the repo switched", async () => {
    // Repo A's list is deferred: it resolves only after the panel was
    // re-pointed at repo B (keep-alive panels reload on repoId change).
    let resolveR1: (list: BranchInfo[]) => void = () => {};
    const r1Pending = new Promise<BranchInfo[]>((resolve) => {
      resolveR1 = resolve;
    });
    const r2List = [branch({ name: "r2-main", is_head: true })];
    mocks.branches.mockImplementation((id: string) =>
      id === "r1" ? r1Pending : Promise.resolve(r2List),
    );

    const { rerender } = render(BranchPanel, { repoId: "r1" });
    // The first reload parks on the deferred r1 response.
    await waitFor(() => expect(mocks.branches).toHaveBeenCalledWith("r1"));

    rerender({ repoId: "r2" });
    await waitFor(() =>
      expect(mocks.controller.setList).toHaveBeenCalledWith(r2List),
    );

    // The slow r1 response lands after the switch: neither the panel nor
    // the (repo-scoped) ops controller may see the stale list.
    resolveR1([branch({ name: "stale-a" }), branch({ name: "stale-b" })]);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(mocks.controller.setList).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("stale-a")).toBeNull();
    expect(screen.queryByText("stale-b")).toBeNull();
  });
});
