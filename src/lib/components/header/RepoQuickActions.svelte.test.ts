/**
 * RepoQuickActions tests: the Sync split button runs the contextual verb
 * (pull when behind, push when ahead, fetch otherwise) with its accessible
 * name carrying branch + counts, success refreshes the owner, preconditions
 * disable (never toast-and-run), and ops started elsewhere disable/spin the
 * group.
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
  opStore.ops = {};
});

function button(name: string | RegExp): HTMLButtonElement {
  return screen.getByRole("button", { name }) as HTMLButtonElement;
}

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
