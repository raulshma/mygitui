import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";
import type { WorktreeInfo } from "$lib/ipc/types";

const mocks = vi.hoisted(() => ({
  branches: vi.fn(),
  pickFolder: vi.fn(),
  worktreeAdd: vi.fn(),
  worktreePrune: vi.fn(),
  worktreeRemove: vi.fn(),
  worktrees: vi.fn(),
}));

vi.mock("$lib/ipc/client", () => ({
  branches: mocks.branches,
  pickFolder: mocks.pickFolder,
  worktreeAdd: mocks.worktreeAdd,
  worktreePrune: mocks.worktreePrune,
  worktreeRemove: mocks.worktreeRemove,
  worktrees: mocks.worktrees,
}));

vi.mock("$lib/stores/tabs.svelte", () => ({
  openTab: vi.fn(async () => {}),
}));

vi.mock("$lib/toast", () => ({
  toast: vi.fn(),
  dismissToast: vi.fn(),
  pauseToast: vi.fn(),
  resumeToast: vi.fn(),
  getToasts: vi.fn(() => []),
}));

import { toast } from "$lib/toast";
import WorktreePanel from "./WorktreePanel.svelte";

function linkedWorktree(patch: Partial<WorktreeInfo> = {}): WorktreeInfo {
  return {
    path: "/tmp/repo-wt-feature",
    name: "wt-feature",
    branch: "feature",
    head: "abcdef1234567890",
    detached: false,
    locked: false,
    prunable: null,
    is_main: false,
    ...patch,
  };
}

describe("WorktreePanel remove flow", () => {
  it("invokes worktree_remove after the confirm dialog and toasts on success", async () => {
    mocks.worktrees.mockResolvedValueOnce([linkedWorktree()]).mockResolvedValueOnce([]);
    mocks.branches.mockResolvedValue([]);
    mocks.worktreeRemove.mockResolvedValue(undefined);

    render(WorktreePanel, { repoId: "r1", root: "/tmp/repo" });

    // List loaded: the linked worktree row is present.
    await screen.findByText("wt-feature");

    // Row remove → in-row confirm → ConfirmDialog confirm.
    await fireEvent.click(
      screen.getByRole("button", { name: "Remove worktree wt-feature" }),
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Confirm remove" }),
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Remove worktree" }),
    );

    await waitFor(() =>
      expect(mocks.worktreeRemove).toHaveBeenCalledWith(
        "r1",
        "wt-feature",
        false,
      ),
    );
    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith("Removed worktree wt-feature", {
        kind: "success",
      }),
    );
    // The panel reloaded and the removed worktree is gone from the list.
    await waitFor(() =>
      expect(screen.queryByText("wt-feature")).toBeNull(),
    );
  });

  it("shows per-row removal progress while the backend op runs", async () => {
    mocks.worktrees.mockResolvedValue([linkedWorktree()]);
    mocks.branches.mockResolvedValue([]);
    let settleRemove: (() => void) | undefined;
    mocks.worktreeRemove.mockReturnValue(
      new Promise<void>((resolve) => {
        settleRemove = resolve;
      }),
    );

    render(WorktreePanel, { repoId: "r1", root: "/tmp/repo" });
    await screen.findByText("wt-feature");

    await fireEvent.click(
      screen.getByRole("button", { name: "Remove worktree wt-feature" }),
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Confirm remove" }),
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Remove worktree" }),
    );

    // While the IPC is in flight the row swaps its actions for progress.
    const progress = await screen.findByRole("status");
    expect(progress.textContent).toContain("Removing");

    settleRemove?.();
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
  });
});
