/**
 * Unit tests for the shared branch-operations controller
 * (`$lib/git/branchOps`). The IPC client is fully mocked (`vi.mock` +
 * `vi.hoisted`, same seam as the store suites) — no Tauri runtime is
 * touched. The real tab store feeds dirty-worktree state (`resetTabStore`
 * between tests) and toasts are asserted through the real toast store so
 * the exact user-visible texts stay pinned.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  // Branch-op verbs the controller drives directly.
  branchCheckoutRemote: vi.fn(),
  branchCreate: vi.fn(),
  branchDelete: vi.fn(),
  branchRename: vi.fn(),
  branchSwitch: vi.fn(),
  guardCheckpoint: vi.fn(),
  opsPreview: vi.fn(),
  pushRepo: vi.fn(),
  remotes: vi.fn(),
  stashPush: vi.fn(),
  // Pulled in transitively by ops.svelte / tabs.svelte / repoEvents; the
  // controller never calls them, but the mock must cover the imports.
  authRespond: vi.fn(),
  closeRepo: vi.fn(),
  openRepo: vi.fn(),
  remoteRepoOpen: vi.fn(),
  repoStatus: vi.fn(),
  onRepoChanged: vi.fn(),
  onCloneProgress: vi.fn(),
  onActionOutput: vi.fn(),
  onPtyOutput: vi.fn(),
  onPtyExit: vi.fn(),
  onAuthRequest: vi.fn(),
  onOpLog: vi.fn(),
  onOpProgress: vi.fn(),
}));

vi.mock("$lib/ipc/client", () => ({ ...mocks }));

import type { BranchInfo, PreviewInfo, StatusEntry } from "$lib/ipc/types";
import { getToasts } from "$lib/toast";
import { opStore } from "$lib/stores/ops.svelte";
import { resetTabStore, tabStore, type Tab } from "$lib/stores/tabs.svelte";
import { branchOpsController } from "./branchOps.svelte";

// ---------------------------------------------------------------------------
// Fixtures + harness
// ---------------------------------------------------------------------------

const REPO = "repo-1";

function branch(patch: Partial<BranchInfo> & { name: string }): BranchInfo {
  return {
    sha: "abcdef1234567890",
    upstream: null,
    ahead: 0,
    behind: 0,
    gone: false,
    is_head: false,
    ...patch,
  };
}

/** Unmerged-delete preview (non-empty files → not "merged"). */
function unmergedPreview(): PreviewInfo {
  return {
    kind: "branch_delete",
    summary: "unmerged — deleting loses commits",
    files: [{ path: "src/a.ts", change: "deleted" }],
  };
}

/** Merged-delete preview: empty file list + "merged …" summary (the pure
 * `isMergedPreview` predicate in safetyModel keys off exactly this shape). */
function mergedPreview(): PreviewInfo {
  return {
    kind: "branch_delete",
    summary: "merged into main — no commits would be lost",
    files: [],
  };
}

function tabWith(entries: StatusEntry[]): Tab {
  return {
    id: REPO,
    name: "repo",
    root: "/repo",
    generation: 0,
    deviceId: null,
    openError: null,
    status: {
      branch: "main",
      head: "abcdef1234567890",
      detached: false,
      ahead: 0,
      behind: 0,
      merging: false,
      rebasing: false,
      sequencer: false,
      entries,
    },
  };
}

const CLEAN: StatusEntry[] = [];
const DIRTY: StatusEntry[] = [
  { path: "a.ts", old_path: null, index: "unmodified", worktree: "modified" },
];

/** Controller with reload + onMutated spies wired the way the panel does. */
function makeController() {
  const onMutated = vi.fn();
  const reload = vi.fn();
  const ops = branchOpsController(REPO, onMutated);
  ops.setReload(reload);
  return { ops, onMutated, reload };
}

beforeEach(() => {
  resetTabStore();
  for (const mock of Object.values(mocks)) mock.mockReset();
  mocks.branchSwitch.mockResolvedValue(undefined);
  mocks.branchCreate.mockResolvedValue(undefined);
  mocks.branchDelete.mockResolvedValue(undefined);
  mocks.branchRename.mockResolvedValue(undefined);
  mocks.stashPush.mockResolvedValue(undefined);
  mocks.guardCheckpoint.mockResolvedValue({
    id: "cp-1",
    reason: "pre-branch-delete",
    ref_name: "refs/mygitui/checkpoints/cp-1",
    created_at: 0,
    branch: "main",
    has_worktree_state: true,
  });
  mocks.remotes.mockResolvedValue([]);
  mocks.pushRepo.mockResolvedValue({ received_bytes: 0, objects: 0, updated_refs: [] });
  mocks.opsPreview.mockResolvedValue(mergedPreview());
});

afterEach(() => {
  opStore.stop();
  resetTabStore();
});

// ---------------------------------------------------------------------------
// Switch flow
// ---------------------------------------------------------------------------

describe("switch flow", () => {
  it("switches directly when the worktree is clean", async () => {
    tabStore.tabs = [tabWith(CLEAN)];
    const { ops, reload, onMutated } = makeController();

    await ops.requestSwitch("feature");

    expect(mocks.branchSwitch).toHaveBeenCalledWith(REPO, "feature", false);
    expect(ops.switchChoice).toBeNull();
    // Completed mutation → reload hook + owner notification.
    expect(reload).toHaveBeenCalled();
    expect(onMutated).toHaveBeenCalled();
  });

  it("opens the choice dialog instead of switching when the worktree is dirty", async () => {
    tabStore.tabs = [tabWith(DIRTY)];
    const { ops } = makeController();

    await ops.requestSwitch("feature");

    expect(ops.switchChoice).toEqual({ name: "feature" });
    expect(mocks.branchSwitch).not.toHaveBeenCalled();
  });

  it("chooseStash pushes a pre-switch stash, then switches non-forced", async () => {
    tabStore.tabs = [tabWith(DIRTY)];
    const { ops } = makeController();
    await ops.requestSwitch("feature");

    await ops.chooseStash();

    expect(mocks.stashPush).toHaveBeenCalledWith(
      REPO,
      "pre-switch stash (switching to feature)",
    );
    expect(mocks.branchSwitch).toHaveBeenCalledWith(REPO, "feature", false);
    expect(ops.switchChoice).toBeNull();
  });

  it("chooseForce switches with force (discarding modifications)", async () => {
    tabStore.tabs = [tabWith(DIRTY)];
    const { ops } = makeController();
    await ops.requestSwitch("feature");

    await ops.chooseForce();

    expect(mocks.branchSwitch).toHaveBeenCalledWith(REPO, "feature", true);
    expect(ops.switchChoice).toBeNull();
    expect(mocks.stashPush).not.toHaveBeenCalled();
  });

  it("cancelChoice closes the dialog without touching the backend", async () => {
    tabStore.tabs = [tabWith(DIRTY)];
    const { ops } = makeController();
    await ops.requestSwitch("feature");

    ops.cancelChoice();

    expect(ops.switchChoice).toBeNull();
    expect(mocks.branchSwitch).not.toHaveBeenCalled();
    expect(mocks.stashPush).not.toHaveBeenCalled();
  });

  it("toasts `${label} failed` when the switch fails", async () => {
    tabStore.tabs = [tabWith(CLEAN)];
    const { ops } = makeController();
    mocks.branchSwitch.mockRejectedValueOnce(new Error("conflict"));

    await ops.requestSwitch("feature");

    expect(
      getToasts().some(
        (t) => t.kind === "error" && t.message === "Switch to feature failed: conflict",
      ),
    ).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Delete flow
// ---------------------------------------------------------------------------

describe("delete flow", () => {
  it("routes a merged delete to the confirm dialog (into the HEAD branch)", async () => {
    const { ops } = makeController();
    ops.setList([branch({ name: "main", is_head: true }), branch({ name: "feature" })]);
    mocks.opsPreview.mockResolvedValueOnce(mergedPreview());

    await ops.requestDelete(branch({ name: "feature" }));

    expect(mocks.opsPreview).toHaveBeenCalledWith(REPO, "branch_delete", {
      name: "feature",
      into: "main",
    });
    expect(ops.confirmDelete).toEqual({ name: "feature" });
    expect(ops.previewDelete).toBeNull();
  });

  it("falls back to `into: HEAD` when no HEAD branch is in the list", async () => {
    const { ops } = makeController();

    await ops.requestDelete(branch({ name: "feature" }));

    expect(mocks.opsPreview).toHaveBeenCalledWith(REPO, "branch_delete", {
      name: "feature",
      into: "HEAD",
    });
    expect(ops.confirmDelete).toEqual({ name: "feature" });
  });

  it("confirmMergedDelete deletes non-forced and closes the dialog", async () => {
    const { ops } = makeController();
    await ops.requestDelete(branch({ name: "feature" }));

    await ops.confirmMergedDelete();

    expect(mocks.branchDelete).toHaveBeenCalledWith(REPO, "feature", false);
    expect(ops.confirmDelete).toBeNull();
  });

  it("routes an unmerged delete to the preview dialog", async () => {
    const { ops } = makeController();
    const info = unmergedPreview();
    mocks.opsPreview.mockResolvedValueOnce(info);

    await ops.requestDelete(branch({ name: "feature" }));

    expect(ops.previewDelete).toEqual({ name: "feature", info });
    expect(ops.confirmDelete).toBeNull();
  });

  it("confirmForceDelete(true) creates a guard checkpoint then force-deletes", async () => {
    const { ops } = makeController();
    mocks.opsPreview.mockResolvedValueOnce(unmergedPreview());
    await ops.requestDelete(branch({ name: "feature" }));

    await ops.confirmForceDelete(true);

    expect(mocks.guardCheckpoint).toHaveBeenCalledWith(REPO, "pre-branch-delete");
    expect(mocks.branchDelete).toHaveBeenCalledWith(REPO, "feature", true);
    expect(
      getToasts().some(
        (t) =>
          t.kind === "success" &&
          t.message ===
            "Force deleted feature — undo available (checkpoint cp-1)",
      ),
    ).toBe(true);
    expect(ops.previewDelete).toBeNull();
  });

  it("confirmForceDelete(false) skips the checkpoint", async () => {
    const { ops } = makeController();
    mocks.opsPreview.mockResolvedValueOnce(unmergedPreview());
    await ops.requestDelete(branch({ name: "feature" }));

    await ops.confirmForceDelete(false);

    expect(mocks.guardCheckpoint).not.toHaveBeenCalled();
    expect(mocks.branchDelete).toHaveBeenCalledWith(REPO, "feature", true);
  });

  it("toasts when the delete preview fails and opens no dialog", async () => {
    const { ops } = makeController();
    mocks.opsPreview.mockRejectedValueOnce(new Error("boom"));

    await ops.requestDelete(branch({ name: "feature" }));

    expect(
      getToasts().some(
        (t) =>
          t.kind === "error" && t.message === "Cannot preview delete of feature: boom",
      ),
    ).toBe(true);
    expect(ops.confirmDelete).toBeNull();
    expect(ops.previewDelete).toBeNull();
  });

  it("cancelDelete closes both delete dialogs without acting", async () => {
    const { ops } = makeController();
    mocks.opsPreview.mockResolvedValueOnce(unmergedPreview());
    await ops.requestDelete(branch({ name: "feature" }));

    ops.cancelDelete();

    expect(ops.previewDelete).toBeNull();
    expect(mocks.branchDelete).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// Push
// ---------------------------------------------------------------------------

describe("push", () => {
  it("prefers origin over other remotes and pushes with the panel's options", async () => {
    const { ops } = makeController();
    mocks.remotes.mockResolvedValueOnce([
      { name: "upstream", url: "u", push_url: null },
      { name: "origin", url: "o", push_url: null },
    ]);
    mocks.pushRepo.mockResolvedValueOnce({
      received_bytes: 0,
      objects: 0,
      updated_refs: [["refs/heads/feature", "abcdef1"]],
    });

    await ops.push(branch({ name: "feature" }), true);

    expect(mocks.pushRepo).toHaveBeenCalledWith(REPO, {
      remote: "origin",
      branch: "feature",
      force: false,
      force_with_lease: false,
      set_upstream: true,
      refs: [],
      tags: false,
      delete: false,
    });
    expect(
      getToasts().some(
        (t) => t.kind === "success" && t.message === "Pushed feature (1 ref updated)",
      ),
    ).toBe(true);
  });

  it("falls back to the first remote when origin is absent", async () => {
    const ops = branchOpsController(REPO);
    mocks.remotes.mockResolvedValueOnce([
      { name: "upstream", url: "u", push_url: null },
    ]);

    await ops.push(branch({ name: "feature" }), false);

    expect(mocks.pushRepo).toHaveBeenCalledWith(
      REPO,
      expect.objectContaining({ remote: "upstream", set_upstream: false }),
    );
  });

  it("toasts instead of pushing when no remote is configured", async () => {
    const { ops } = makeController();
    mocks.remotes.mockResolvedValueOnce([]);

    await ops.push(branch({ name: "feature" }), false);

    expect(mocks.pushRepo).not.toHaveBeenCalled();
    expect(
      getToasts().some(
        (t) =>
          t.kind === "error" && t.message === "No remote configured — cannot push",
      ),
    ).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Rename / create / remote checkout
// ---------------------------------------------------------------------------

describe("rename", () => {
  it("no-ops (no IPC) on an empty or unchanged name", async () => {
    const { ops } = makeController();

    await ops.rename("feature", "feature");
    await ops.rename("feature", "   ");

    expect(mocks.branchRename).not.toHaveBeenCalled();
  });

  it("renames via the backend on success", async () => {
    const { ops, reload } = makeController();

    await ops.rename("feature", "renamed");

    expect(mocks.branchRename).toHaveBeenCalledWith(REPO, "feature", "renamed");
    expect(reload).toHaveBeenCalled();
  });

  it("toasts `${label} failed` when the rename fails", async () => {
    const { ops } = makeController();
    mocks.branchRename.mockRejectedValueOnce(new Error("branch exists"));

    await ops.rename("feature", "renamed");

    expect(
      getToasts().some(
        (t) =>
          t.kind === "error" &&
          t.message === "Rename feature → renamed failed: branch exists",
      ),
    ).toBe(true);
  });
});

describe("create", () => {
  it("creates from HEAD with checkout and reports success", async () => {
    const { ops } = makeController();

    await expect(ops.create("new-branch")).resolves.toBe(true);

    expect(mocks.branchCreate).toHaveBeenCalledWith(REPO, "new-branch", true);
    expect(
      getToasts().some(
        (t) => t.kind === "success" && t.message === "Created branch new-branch",
      ),
    ).toBe(true);
  });

  it("returns false and toasts when the create fails", async () => {
    const { ops } = makeController();
    mocks.branchCreate.mockRejectedValueOnce(new Error("already exists"));

    await expect(ops.create("new-branch")).resolves.toBe(false);

    expect(
      getToasts().some(
        (t) =>
          t.kind === "error" &&
          t.message === "Create branch new-branch failed: already exists",
      ),
    ).toBe(true);
  });

  it("returns false silently for an empty name", async () => {
    const { ops } = makeController();

    await expect(ops.create("   ")).resolves.toBe(false);
    expect(mocks.branchCreate).not.toHaveBeenCalled();
  });
});

describe("checkoutRemote", () => {
  it("creates + checks out the tracking branch and returns the local name", async () => {
    const { ops } = makeController();
    mocks.branchCheckoutRemote.mockResolvedValueOnce("feature");

    await expect(ops.checkoutRemote("origin", "feature")).resolves.toBe("feature");

    expect(mocks.branchCheckoutRemote).toHaveBeenCalledWith(REPO, "origin", "feature");
    expect(
      getToasts().some(
        (t) =>
          t.kind === "success" &&
          t.message === "Checked out feature (tracking origin/feature)",
      ),
    ).toBe(true);
  });

  it("resolves null and toasts when the checkout fails", async () => {
    const { ops } = makeController();
    mocks.branchCheckoutRemote.mockRejectedValueOnce(new Error("no such remote"));

    await expect(ops.checkoutRemote("origin", "feature")).resolves.toBeNull();

    expect(
      getToasts().some(
        (t) =>
          t.kind === "error" && t.message === "Checkout feature failed: no such remote",
      ),
    ).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Busy + list wiring
// ---------------------------------------------------------------------------

describe("busy and list wiring", () => {
  it("tracks branch-kind ops for this repo via busy", () => {
    const { ops } = makeController();
    expect(ops.busy).toBe(false);

    opStore.applyProgress({
      op_id: "a",
      repo_id: REPO,
      kind: "branch",
      message: "switching",
      pct: null,
      done: false,
      error: null,
    });
    expect(ops.busy).toBe(true);

    // A different kind or repo must not count.
    opStore.applyProgress({
      op_id: "b",
      repo_id: REPO,
      kind: "fetch",
      message: "fetching",
      pct: null,
      done: false,
      error: null,
    });
    expect(ops.busy).toBe(true);

    opStore.applyProgress({
      op_id: "a",
      repo_id: REPO,
      kind: "branch",
      message: "switching",
      pct: null,
      done: true,
      error: null,
    });
    expect(ops.busy).toBe(false);
  });

  it("headBranch reads the fed list; create still mutates without it", async () => {
    const { ops } = makeController();
    expect(ops.headBranch()).toBeNull();

    ops.setList([branch({ name: "main", is_head: true }), branch({ name: "dev" })]);
    expect(ops.headBranch()?.name).toBe("main");

    await ops.create("from-head");
    expect(mocks.branchCreate).toHaveBeenCalledWith(REPO, "from-head", true);
  });
});
