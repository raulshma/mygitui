/**
 * StatusBar tests: the anti-twitch contract (the bar element stays mounted
 * at a fixed height while ops appear and disappear inside it), branch/sync/
 * changes reflect the active tab's status and wire their clicks (branch
 * switcher / Status panel focus via the UI bus), the sync item runs the
 * contextual verb through the shared sync helpers, background-repo ops carry
 * the tab name, cancel goes through the op store, and the auto-fetch chip
 * mirrors the cadence.
 */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { NetStats, OpProgress, RepoStatus, StatusEntry } from "$lib/ipc/types";

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

// StatusBar reads the store singletons (ops / tabs / autofetch /
// remoteDevices), which bind these client functions at module load. None of
// the commands run here except `repoStatus` (post-sync status refresh).
vi.mock("$lib/ipc/client", () => ({
  authRespond: vi.fn(),
  opCancel: vi.fn(),
  openRepo: vi.fn(),
  closeRepo: vi.fn(),
  repoStatus: vi.fn(),
  remoteRepoOpen: vi.fn(),
  branches: vi.fn(),
  fetchRepo: vi.fn(),
  remotes: vi.fn(),
  deviceAdd: vi.fn(),
  deviceAuthRespond: vi.fn(),
  deviceConnect: vi.fn(),
  deviceDisconnect: vi.fn(),
  deviceForgetHostKey: vi.fn(),
  deviceHostkeyRespond: vi.fn(),
  deviceList: vi.fn(),
  deviceRemove: vi.fn(),
  deviceUpdate: vi.fn(),
  conflicts: vi.fn(),
  commitActivity: vi.fn(),
  contributorStats: vi.fn(),
  projectDetect: vi.fn(),
  ptyCreate: vi.fn(),
  ptyKill: vi.fn(),
  ptyResize: vi.fn(),
  ptyWrite: vi.fn(),
  rshellCreate: vi.fn(),
  rshellKill: vi.fn(),
  rshellResize: vi.fn(),
  rshellWrite: vi.fn(),
  onRepoChanged: vi.fn(),
  onCloneProgress: vi.fn(),
  onOpProgress: vi.fn(),
  onOpLog: vi.fn(),
  onAuthRequest: vi.fn(),
  onActionOutput: vi.fn(),
  onPtyOutput: vi.fn(),
  onPtyExit: vi.fn(),
  onHostkeyRequest: vi.fn(),
  onDeviceAuthRequest: vi.fn(),
  onDeviceStatus: vi.fn(),
}));

import { opCancel } from "$lib/ipc/client";
import { opStore, type ActiveOp } from "$lib/stores/ops.svelte";
import { tabStore, type Tab } from "$lib/stores/tabs.svelte";
import { autofetch } from "$lib/stores/autofetch.svelte";
import { toast } from "$lib/toast";
import StatusBar from "./StatusBar.svelte";

const mockOpCancel = vi.mocked(opCancel);

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

function entry(path: string): StatusEntry {
  return { path, old_path: null, index: "unmodified", worktree: "modified" };
}

function tab(patch: Partial<Tab>): Tab {
  return {
    id: "r1",
    name: "mygitui",
    root: "/repo",
    status: null,
    statusError: null,
    generation: 0,
    deviceId: null,
    openError: null,
    ...patch,
  };
}

function progress(patch: Partial<OpProgress> & {
  op_id: string;
  repo_id: string;
}): OpProgress {
  return {
    kind: "fetch",
    message: "contacting remote",
    pct: null,
    done: false,
    error: null,
    outcome: null,
    ...patch,
  };
}

/** An in-flight (never-done) op object, for direct store seeding. */
function activeOp(patch: Partial<ActiveOp> & { op_id: string }): ActiveOp {
  return {
    kind: "fetch",
    message: "contacting remote",
    pct: null,
    done: false,
    error: null,
    cancelling: false,
    ...patch,
  };
}

function bar(): HTMLElement {
  return screen.getByRole("contentinfo", { name: "Status bar" });
}

function syncButton(): HTMLButtonElement {
  return screen.getByRole("button", {
    name: /Sync with remote/,
  }) as HTMLButtonElement;
}

beforeEach(() => {
  vi.clearAllMocks();
  tabStore.tabs = [];
  tabStore.activeId = null;
  opStore.ops = {};
  autofetch.config = {};
});

afterEach(() => {
  cleanup();
  tabStore.tabs = [];
  tabStore.activeId = null;
  opStore.ops = {};
  autofetch.config = {};
});

describe("StatusBar", () => {
  it("renders the fixed bar even with no status and no ops", () => {
    tabStore.tabs = [tab({ status: null })];
    tabStore.activeId = "r1";
    render(StatusBar);

    expect(bar().classList.contains("h-6")).toBe(true);
    // No branch / changes until a status lands; sync stays (fetch works).
    expect(screen.queryByText("main")).toBeNull();
    expect(screen.queryByLabelText(/in progress/)).toBeNull();
    expect(syncButton()).not.toBeNull();
  });

  it("shows branch, sync counts, and change count from the active status", () => {
    tabStore.tabs = [
      tab({
        status: status({
          ahead: 2,
          behind: 1,
          entries: [entry("a.ts"), entry("b.ts"), entry("a.ts")],
        }),
      }),
    ];
    tabStore.activeId = "r1";
    render(StatusBar);

    expect(screen.getByText("main")).not.toBeNull();
    expect(screen.getByText("2 changes")).not.toBeNull(); // unique paths
    expect(screen.getByText("1")).not.toBeNull(); // behind
    expect(screen.getByText("2")).not.toBeNull(); // ahead
  });

  it("branch and changes clicks emit the UI bus events", async () => {
    tabStore.tabs = [
      tab({ status: status({ entries: [entry("a.ts")] }) }),
    ];
    tabStore.activeId = "r1";
    render(StatusBar);

    const events: CustomEvent[] = [];
    const spy = (event: Event): void => {
      events.push(event as CustomEvent);
    };
    document.addEventListener("branch-switcher-open", spy);
    document.addEventListener("focus-panel", spy);
    try {
      await fireEvent.click(screen.getByRole("button", { name: /main/ }));
      await fireEvent.click(
        screen.getByRole("button", { name: /1 change/ }),
      );
    } finally {
      document.removeEventListener("branch-switcher-open", spy);
      document.removeEventListener("focus-panel", spy);
    }

    expect(events.map((e) => e.type)).toEqual([
      "branch-switcher-open",
      "focus-panel",
    ]);
    expect(events[0]!.detail).toEqual({ repoId: "r1" });
    expect(events[1]!.detail).toEqual({ panel: "status", repoId: "r1" });
  });

  it("runs the contextual verb (pull when behind) and refreshes status", async () => {
    mocks.pullCurrentBranch.mockResolvedValue({
      kind: "done",
      remote: "origin",
      stats: STATS,
    });
    tabStore.tabs = [tab({ status: status({ ahead: 2, behind: 1 }) })];
    tabStore.activeId = "r1";
    render(StatusBar);

    await fireEvent.click(syncButton());

    expect(mocks.pullCurrentBranch).toHaveBeenCalledWith("r1", "main");
    await waitFor(() => expect(toast).toHaveBeenCalled());
    expect(mocks.fetchDefaultRemote).not.toHaveBeenCalled();
  });

  it("runs fetch when the branch is in sync", async () => {
    mocks.fetchDefaultRemote.mockResolvedValue({
      kind: "done",
      remote: "origin",
      stats: STATS,
    });
    tabStore.tabs = [tab({ status: status() })];
    tabStore.activeId = "r1";
    render(StatusBar);

    await fireEvent.click(syncButton());

    expect(mocks.fetchDefaultRemote).toHaveBeenCalledWith("r1");
  });

  it("spins the sync item while a sync op runs, then recovers", async () => {
    tabStore.tabs = [tab({ status: status() })];
    tabStore.activeId = "r1";
    render(StatusBar);

    const btn = syncButton();
    expect(btn.getAttribute("aria-busy")).toBe("false");
    expect(btn.disabled).toBe(false);

    opStore.applyProgress(progress({ op_id: "op-1", repo_id: "r1" }));
    await tick();
    expect(btn.getAttribute("aria-busy")).toBe("true");
    expect(btn.disabled).toBe(true);

    opStore.applyProgress(
      progress({ op_id: "op-1", repo_id: "r1", done: true, outcome: "completed" }),
    );
    await tick();
    expect(btn.getAttribute("aria-busy")).toBe("false");
  });

  it("ops appear and vanish inside the bar without unmounting it", async () => {
    mockOpCancel.mockResolvedValue(true);
    tabStore.tabs = [tab({ status: status() })];
    tabStore.activeId = "r1";
    render(StatusBar);
    const barEl = bar();

    opStore.applyProgress(
      progress({
        op_id: "op-1",
        repo_id: "r1",
        message: "fetching origin",
        pct: 40,
      }),
    );
    await tick();

    // Same DOM node — the bar never mounts/unmounts (the twitch fix).
    expect(bar()).toBe(barEl);
    expect(screen.getByText("Fetch")).not.toBeNull();
    expect(screen.getByText("fetching origin")).not.toBeNull();
    expect(screen.getByText("40%")).not.toBeNull();

    await fireEvent.click(
      screen.getByRole("button", { name: "Cancel Fetch" }),
    );
    expect(mockOpCancel).toHaveBeenCalledWith("r1", "op-1");

    opStore.applyProgress(
      progress({ op_id: "op-1", repo_id: "r1", done: true, outcome: "completed" }),
    );
    await tick();

    expect(bar()).toBe(barEl);
    expect(screen.queryByText("fetching origin")).toBeNull();
    expect(screen.queryByRole("button", { name: "Cancel Fetch" })).toBeNull();
  });

  it("background-repo ops carry the tab name", async () => {
    tabStore.tabs = [
      tab({ id: "r1", name: "mygitui", root: "/repo", status: status() }),
      tab({ id: "r2", name: "other-repo", root: "/other", status: null }),
    ];
    tabStore.activeId = "r1";
    render(StatusBar);

    opStore.ops = { r2: [activeOp({ op_id: "op-2", kind: "push" })] };
    await tick();

    expect(screen.getByText("other-repo")).not.toBeNull();
    expect(screen.getByText("Push")).not.toBeNull();
  });

  it("shows the merge state chip while a merge is in progress", () => {
    tabStore.tabs = [tab({ status: status({ merging: true }) })];
    tabStore.activeId = "r1";
    render(StatusBar);

    expect(screen.getByText("Merge in progress")).not.toBeNull();
  });

  it("auto-fetch chip mirrors the repo cadence (hidden when off) and opens the Remotes panel", async () => {
    tabStore.tabs = [tab({ status: status() })];
    tabStore.activeId = "r1";
    const { unmount } = render(StatusBar);
    expect(screen.queryByText("auto 5m")).toBeNull();
    unmount();

    autofetch.config = { "/repo": 5 };
    render(StatusBar);
    await tick();
    expect(screen.getByText("auto 5m")).not.toBeNull();

    const events: CustomEvent[] = [];
    const spy = (event: Event): void => {
      events.push(event as CustomEvent);
    };
    document.addEventListener("focus-panel", spy);
    try {
      await fireEvent.click(
        screen.getByRole("button", { name: "auto 5m" }),
      );
    } finally {
      document.removeEventListener("focus-panel", spy);
    }
    expect(events[0]?.detail).toEqual({ panel: "remotes", repoId: "r1" });
  });

  it("shows the remote device connection for remote tabs", () => {
    tabStore.tabs = [tab({ status: status(), deviceId: "dev-1" })];
    tabStore.activeId = "r1";
    render(StatusBar);

    // Unknown device id → disconnected → "offline" (mirrors the header chip).
    expect(screen.getByText("offline")).not.toBeNull();
  });

  it("detached HEAD labels the branch item with the short sha", () => {
    tabStore.tabs = [
      tab({ status: status({ detached: true, branch: null }) }),
    ];
    tabStore.activeId = "r1";
    render(StatusBar);

    expect(screen.getByText("detached @ abcdef1")).not.toBeNull();
  });
});
