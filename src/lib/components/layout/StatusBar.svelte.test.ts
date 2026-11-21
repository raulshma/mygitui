/**
 * StatusBar tests: the anti-twitch contract (the bar element stays mounted
 * at a fixed height while ops appear and disappear inside it), branch/sync/
 * changes reflect the active tab's status and wire their clicks (branch
 * switcher / Status panel focus via the UI bus), the sync item runs the
 * contextual verb through the shared sync helpers, background-repo ops carry
 * the tab name, cancel goes through the op store, the auto-fetch chip
 * mirrors the cadence, and every network op row is click-through — to the
 * log overlay when hook output exists, otherwise to the recent-ops popover
 * (exclusively with it).
 */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
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
vi.mock("$lib/ipc/client", async (importOriginal) => ({
  // Real module first: its pure envelope helpers (ipcErrorText,
  // parseIpcErrorEnvelope) stay real for the op store's error display and
  // cancellation classification; the command stubs below override.
  ...(await importOriginal<typeof import("$lib/ipc/client")>()),
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
  onRepoWatchState: vi.fn(),
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
  opStore.logs = {};
  opStore.history = {};
  autofetch.config = {};
});

afterEach(() => {
  cleanup();
  tabStore.tabs = [];
  tabStore.activeId = null;
  opStore.ops = {};
  opStore.logs = {};
  opStore.history = {};
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

  it("hook-bearing op rows toggle the log overlay (click 'pushing to …')", async () => {
    tabStore.tabs = [tab({ status: status() })];
    tabStore.activeId = "r1";
    render(StatusBar);

    opStore.applyProgress(
      progress({
        op_id: "op-1",
        repo_id: "r1",
        kind: "push",
        message: "pushing to origin",
      }),
    );
    // A new session opens the overlay once (the pre-existing cue)…
    opStore.applyLog({
      repo_id: "r1",
      op_id: "op-1",
      kind: "push",
      line: "husky > pre-push (node v22)",
    });
    await tick();
    // The overlay strip (a named <section> → role="region"; the chip shows
    // the same title text, so query by role).
    expect(screen.getByRole("region", { name: "Push output" })).not.toBeNull();

    // …and the op row's main segment toggles it closed and back open, so a
    // user who dismissed the output can bring it back from the op itself.
    const rowText = screen.getByText("pushing to origin");
    expect(rowText.closest("button")).not.toBeNull();
    await fireEvent.click(rowText);
    await tick();
    expect(screen.queryByRole("region", { name: "Push output" })).toBeNull();
    expect(rowText.closest("button")?.getAttribute("aria-expanded")).toBe(
      "false",
    );

    await fireEvent.click(rowText);
    await tick();
    expect(screen.getByRole("region", { name: "Push output" })).not.toBeNull();
    expect(screen.getByText("husky > pre-push (node v22)")).not.toBeNull();
    expect(rowText.closest("button")?.getAttribute("aria-expanded")).toBe(
      "true",
    );
  });

  it("log-less network op rows open the recent-ops popover", async () => {
    tabStore.tabs = [tab({ status: status() })];
    tabStore.activeId = "r1";
    render(StatusBar);

    opStore.applyProgress(
      progress({
        op_id: "op-1",
        repo_id: "r1",
        kind: "push",
        message: "pushing to origin",
        pct: 12,
      }),
    );
    await tick();

    // No op-log session, but a network op: the row's main segment is a
    // real button (like the log rows) whose accessible name carries the
    // op message.
    const rowText = screen.getByText("pushing to origin");
    const rowButton = rowText.closest("button");
    expect(rowButton).not.toBeNull();
    expect(rowButton?.getAttribute("aria-expanded")).toBe("false");

    // Clicking it opens the recent-ops popover anchored to the row,
    // listing the in-flight op.
    await fireEvent.click(rowText);
    await tick();
    const popover = screen.getByRole("dialog", { name: "Recent operations" });
    const running = within(popover).getByRole("list", {
      name: "Running operations",
    });
    expect(within(running).getByText("pushing to origin")).not.toBeNull();
    expect(within(running).getByText("12%")).not.toBeNull();
    expect(within(running).getByText("running")).not.toBeNull();
    expect(rowButton?.getAttribute("aria-expanded")).toBe("true");

    // Toggling the row again closes it.
    await fireEvent.click(rowText);
    await tick();
    expect(
      screen.queryByRole("dialog", { name: "Recent operations" }),
    ).toBeNull();
    expect(rowButton?.getAttribute("aria-expanded")).toBe("false");
  });

  it("non-network op rows without hook output stay inert", async () => {
    tabStore.tabs = [tab({ status: status() })];
    tabStore.activeId = "r1";
    render(StatusBar);

    opStore.applyProgress(
      progress({
        op_id: "op-1",
        repo_id: "r1",
        kind: "stage",
        message: "staging changes",
      }),
    );
    await tick();

    // No op-log session and nothing to view: the row is plain text.
    expect(screen.getByText("staging changes").closest("button")).toBeNull();
    expect(screen.queryByText("output")).toBeNull();
  });

  it("the row popover flips its in-flight listing to completed history as the op finishes", async () => {
    tabStore.tabs = [tab({ status: status() })];
    tabStore.activeId = "r1";
    render(StatusBar);

    opStore.applyProgress(
      progress({
        op_id: "op-1",
        repo_id: "r1",
        kind: "pull",
        message: "pulling origin",
        pct: 55,
      }),
    );
    await tick();
    await fireEvent.click(screen.getByText("pulling origin"));
    await tick();

    const popover = screen.getByRole("dialog", { name: "Recent operations" });
    expect(
      within(popover).getByRole("list", { name: "Running operations" }),
    ).not.toBeNull();
    expect(
      within(popover).queryByRole("list", { name: "Recent operations list" }),
    ).toBeNull();

    opStore.applyProgress(
      progress({
        op_id: "op-1",
        repo_id: "r1",
        kind: "pull",
        message: "pulling origin",
        done: true,
        outcome: "completed",
      }),
    );
    await tick();

    // The same popover, still open and visible (now anchored to the idle
    // chip the finished op leaves behind), lists the completed entry.
    const donePopover = screen.getByRole("dialog", {
      name: "Recent operations",
    });
    expect(
      within(donePopover).queryByRole("list", { name: "Running operations" }),
    ).toBeNull();
    const history = within(donePopover).getByRole("list", {
      name: "Recent operations list",
    });
    expect(within(history).getByText("Pull")).not.toBeNull();
    expect(within(history).getByText("succeeded")).not.toBeNull();
  });

  it("opening the popover closes the log overlay and vice versa", async () => {
    tabStore.tabs = [tab({ status: status() })];
    tabStore.activeId = "r1";
    render(StatusBar);

    // A push with hook output: the new session auto-opens the overlay.
    opStore.applyProgress(
      progress({
        op_id: "op-1",
        repo_id: "r1",
        kind: "push",
        message: "pushing to origin",
      }),
    );
    opStore.applyLog({
      repo_id: "r1",
      op_id: "op-1",
      kind: "push",
      line: "husky > pre-push",
    });
    await tick();
    expect(screen.getByRole("region", { name: "Push output" })).not.toBeNull();

    // The op completes; the idle chip appears next to the open overlay.
    opStore.applyProgress(
      progress({
        op_id: "op-1",
        repo_id: "r1",
        kind: "push",
        message: "pushing to origin",
        done: true,
        outcome: "completed",
      }),
    );
    await tick();

    // Opening the popover from the chip closes the overlay…
    await fireEvent.click(
      screen.getByRole("button", { name: /Recent operations — last:/ }),
    );
    await tick();
    expect(screen.queryByRole("region", { name: "Push output" })).toBeNull();
    expect(
      screen.getByRole("dialog", { name: "Recent operations" }),
    ).not.toBeNull();

    // …and opening the overlay from the log chip closes the popover.
    await fireEvent.click(
      screen.getByRole("button", { name: "Show Push output" }),
    );
    await tick();
    expect(screen.getByRole("region", { name: "Push output" })).not.toBeNull();
    expect(
      screen.queryByRole("dialog", { name: "Recent operations" }),
    ).toBeNull();
  });

  it("clicking a background repo's op row opens that repo's log", async () => {
    tabStore.tabs = [
      tab({ id: "r1", name: "mygitui", root: "/repo", status: status() }),
      tab({ id: "r2", name: "other-repo", root: "/other", status: null }),
    ];
    tabStore.activeId = "r1";
    render(StatusBar);

    // Logs for a background repo never auto-open (the cue tracks the active
    // repo) — its op row is the only way to surface them.
    opStore.ops = {
      r2: [activeOp({ op_id: "op-2", kind: "push", message: "pushing to origin" })],
    };
    opStore.applyLog({
      repo_id: "r2",
      op_id: "op-2",
      kind: "push",
      line: "r2 pre-push hook line",
    });
    await tick();
    expect(screen.queryByRole("region", { name: "Push output" })).toBeNull();

    await fireEvent.click(screen.getByText("pushing to origin"));
    await tick();
    expect(screen.getByRole("region", { name: "Push output" })).not.toBeNull();
    expect(screen.getByText("r2 pre-push hook line")).not.toBeNull();
  });

  it("clicking a background repo's log-less op row opens that repo's popover", async () => {
    tabStore.tabs = [
      tab({ id: "r1", name: "mygitui", root: "/repo", status: status() }),
      tab({ id: "r2", name: "other-repo", root: "/other", status: null }),
    ];
    tabStore.activeId = "r1";
    render(StatusBar);

    // The row targets ITS repo: the popover lists r2's in-flight op, not
    // the active repo's (empty) ops.
    opStore.ops = {
      r2: [
        activeOp({
          op_id: "op-2",
          kind: "push",
          message: "pushing to origin",
        }),
      ],
    };
    await tick();

    await fireEvent.click(screen.getByText("pushing to origin"));
    await tick();

    const popover = screen.getByRole("dialog", { name: "Recent operations" });
    const running = within(popover).getByRole("list", {
      name: "Running operations",
    });
    expect(within(running).getByText("pushing to origin")).not.toBeNull();
    expect(within(running).getByText("Push")).not.toBeNull();
  });

  it("the cadence chip mirrors the repo cadence and its popover changes it", async () => {
    // Workstream D §8.6: the "auto 5m" text became an icon whose
    // accessible name carries the cadence; the click opens the cadence
    // popover whose radio rows write the per-repo override (the old
    // Remotes-panel click-through stays as the popover's footer row).
    tabStore.tabs = [tab({ status: status() })];
    tabStore.activeId = "r1";

    // Off is still reachable (the chip renders dimmed with an "off" name).
    const first = render(StatusBar);
    expect(
      screen.getByRole("button", {
        name: "Auto-fetch off — click to set a cadence",
      }),
    ).not.toBeNull();
    first.unmount();

    autofetch.config = { "/repo": 5 };
    render(StatusBar);
    await tick();
    const chip = screen.getByRole("button", {
      name: "Auto-fetch every 5 min",
    });

    await fireEvent.click(chip);
    const popover = await screen.findByRole("dialog", {
      name: "Auto-fetch cadence",
    });
    // Cadence rows are pressed toggle-buttons (not a roving radiogroup).
    // (Scoped to the git-fetch section: the E1 GitHub cadence section below
    // offers its own "Every 5 minutes" row.)
    const fetchGroup = within(popover).getByRole("group", {
      name: "Fetch every",
    });
    expect(
      within(fetchGroup)
        .getByRole("button", { name: "Every 5 minutes" })
        .getAttribute("aria-pressed"),
    ).toBe("true");

    // Picking a cadence writes the per-repo override immediately.
    await fireEvent.click(
      within(fetchGroup).getByRole("button", { name: "Every 15 minutes" }),
    );
    expect(autofetch.config["/repo"]).toBe(15);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Auto-fetch every 15 min" }),
      ).not.toBeNull(),
    );

    // The old click-through survives as the popover's footer row.
    const events: CustomEvent[] = [];
    const spy = (event: Event): void => {
      events.push(event as CustomEvent);
    };
    document.addEventListener("focus-panel", spy);
    try {
      await fireEvent.click(
        within(popover).getByRole("button", {
          name: "More fetch settings — Remotes panel",
        }),
      );
    } finally {
      document.removeEventListener("focus-panel", spy);
    }
    expect(events[0]?.detail).toEqual({ panel: "remotes", repoId: "r1" });
    expect(
      screen.queryByRole("dialog", { name: "Auto-fetch cadence" }),
    ).toBeNull();
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
