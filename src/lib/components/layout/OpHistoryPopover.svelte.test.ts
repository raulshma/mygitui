/**
 * OpHistoryPopover tests: the in-flight section lists running ops (kind,
 * message, percent, "running" chip) in a distinct list above the completed
 * history ring, the idle path renders the completed list exactly as
 * before, and Escape / outside press report `onClose` to the owner.
 */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The popover reads the op store singleton, which binds these client
// functions at module load. None of the commands run here.
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

vi.mock("$lib/toast", () => ({
  toast: vi.fn(),
  dismissToast: vi.fn(),
  pauseToast: vi.fn(),
  resumeToast: vi.fn(),
  getToasts: vi.fn(() => []),
}));

import { opStore, type ActiveOp, type OpHistoryEntry } from "$lib/stores/ops.svelte";
import OpHistoryPopover from "./OpHistoryPopover.svelte";

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

/** A completed-op history entry, for direct store seeding. */
function historyEntry(
  patch: Partial<OpHistoryEntry> & { op_id: string },
): OpHistoryEntry {
  return {
    kind: "pull",
    message: "pulling origin",
    outcome: "succeeded",
    error: null,
    started_at: Date.now() - 5_000,
    finished_at: Date.now(),
    ...patch,
  };
}

function popover(): HTMLElement {
  return screen.getByRole("dialog", { name: "Recent operations" });
}

beforeEach(() => {
  vi.clearAllMocks();
  opStore.ops = {};
  opStore.history = {};
});

afterEach(() => {
  cleanup();
  opStore.ops = {};
  opStore.history = {};
});

describe("OpHistoryPopover", () => {
  it("lists in-flight ops in a distinct section above the completed history", () => {
    opStore.ops = {
      r1: [
        activeOp({ op_id: "op-1", kind: "push", message: "pushing to origin", pct: 40 }),
      ],
    };
    opStore.history = {
      r1: [
        historyEntry({
          op_id: "op-2",
          outcome: "failed",
          error: "could not resolve host",
        }),
      ],
    };
    render(OpHistoryPopover, { props: { repoId: "r1", anchor: undefined, onClose: vi.fn() } });

    const panel = popover();
    const running = within(panel).getByRole("list", {
      name: "Running operations",
    });
    expect(within(running).getByText("Push")).not.toBeNull();
    expect(within(running).getByText("pushing to origin")).not.toBeNull();
    expect(within(running).getByText("40%")).not.toBeNull();
    expect(within(running).getByText("running")).not.toBeNull();

    const history = within(panel).getByRole("list", {
      name: "Recent operations list",
    });
    expect(within(history).getByText("Pull")).not.toBeNull();
    expect(within(history).getByText("failed")).not.toBeNull();
    expect(within(history).getByText("could not resolve host")).not.toBeNull();

    // The live section sits above the ring.
    expect(
      running.compareDocumentPosition(history) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(within(panel).getByText("2 ops")).not.toBeNull();
  });

  it("renders the completed list exactly as before when nothing is in flight", () => {
    opStore.history = { r1: [historyEntry({ op_id: "op-2" })] };
    render(OpHistoryPopover, { props: { repoId: "r1", anchor: undefined, onClose: vi.fn() } });

    // No in-flight section; the history list is the only one.
    expect(screen.queryByRole("list", { name: "Running operations" })).toBeNull();
    const history = screen.getByRole("list", {
      name: "Recent operations list",
    });
    expect(within(history).getByText("succeeded")).not.toBeNull();
    expect(within(popover()).getByText("1 op")).not.toBeNull();
  });

  it("hides the empty history list while only in-flight ops are listed", () => {
    opStore.ops = {
      r1: [activeOp({ op_id: "op-1", kind: "fetch", message: "fetching origin" })],
    };
    render(OpHistoryPopover, { props: { repoId: "r1", anchor: undefined, onClose: vi.fn() } });

    expect(
      within(popover()).getByRole("list", { name: "Running operations" }),
    ).not.toBeNull();
    expect(
      screen.queryByRole("list", { name: "Recent operations list" }),
    ).toBeNull();
  });

  it("Escape and outside press report onClose", () => {
    const onClose = vi.fn();
    render(OpHistoryPopover, { props: { repoId: "r1", anchor: undefined, onClose } });

    fireEvent.keyDown(popover(), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);

    // Presses outside the panel (and anchor) close it.
    onClose.mockClear();
    fireEvent.pointerDown(document.body);
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
