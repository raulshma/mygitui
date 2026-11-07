/**
 * OpLogStrip: newest hook/CLI output is rendered with its op title, can be
 * collapsed/dismissed, active ops render a Cancel affordance, and the
 * component renders nothing when the repo has no log session and no running
 * ops.
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { opCancel } from "$lib/ipc/client";
import { opStore, type ActiveOp } from "$lib/stores/ops.svelte";
import OpLogStrip from "./OpLogStrip.svelte";

// The Cancel affordance goes through `opCancel` (via the op store). Mocked
// so the cancelling state is observable without a Tauri runtime; nothing
// else in this suite touches the IPC client.
vi.mock("$lib/ipc/client", () => ({
  authRespond: vi.fn(),
  opCancel: vi.fn(),
  onAuthRequest: vi.fn(),
  onOpProgress: vi.fn(),
  onOpLog: vi.fn(),
  onRepoChanged: vi.fn(),
  onCloneProgress: vi.fn(),
  onActionOutput: vi.fn(),
  onPtyOutput: vi.fn(),
  onPtyExit: vi.fn(),
}));

const mockOpCancel = vi.mocked(opCancel);

afterEach(() => {
  cleanup();
  opStore.logs = {};
  opStore.ops = {};
  mockOpCancel.mockReset();
});

describe("OpLogStrip", () => {
  it("renders nothing when the repo has no log session", () => {
    render(OpLogStrip, { repoId: "r1" });
    expect(screen.queryByText("Commit output")).toBeNull();
  });

  it("shows hook lines, collapses, and dismisses the session", async () => {
    opStore.logs = {
      r1: {
        op_id: "op-1",
        kind: "commit",
        lines: ["pre-commit: lint", "pre-commit: tests"],
        done: true,
        error: null,
      },
    };
    render(OpLogStrip, { repoId: "r1" });

    expect(screen.getByText("Commit output")).not.toBeNull();
    expect(screen.getByText(/pre-commit: lint/)).not.toBeNull();
    expect(screen.getByText(/pre-commit: tests/)).not.toBeNull();
    expect(screen.getByText("2 lines")).not.toBeNull();

    await fireEvent.click(
      screen.getByRole("button", { name: "Collapse output" }),
    );
    expect(screen.queryByLabelText("Commit output lines")).toBeNull();

    await fireEvent.click(screen.getByRole("button", { name: "Dismiss output" }));
    expect(screen.queryByText("Commit output")).toBeNull();
  });

  it("labels failed and running sessions", () => {
    opStore.logs = {
      r1: {
        op_id: "op-1",
        kind: "push",
        lines: ["remote: rejected"],
        done: true,
        error: "non-fast-forward",
      },
    };
    const { unmount } = render(OpLogStrip, { repoId: "r1" });
    expect(screen.getByText("Push output")).not.toBeNull();
    expect(
      screen.getByRole("region", { name: "Push output" }).classList.contains(
        "failed",
      ),
    ).toBe(true);
    unmount();

    opStore.logs = {
      r1: {
        op_id: "op-2",
        kind: "push",
        lines: ["running pre-push"],
        done: false,
        error: null,
      },
    };
    render(OpLogStrip, { repoId: "r1" });
    expect(
      screen.getByRole("region", { name: "Push output" }).getAttribute(
        "aria-busy",
      ),
    ).toBe("true");
  });

  it("renders active ops with a Cancel affordance that flips to Cancelling…", async () => {
    // A never-settling cancel keeps the flag up for the assertions.
    mockOpCancel.mockReturnValue(new Promise<boolean>(() => {}));
    const op: ActiveOp = {
      op_id: "op-9",
      kind: "push",
      message: "sending pack",
      pct: 40,
      done: false,
      error: null,
      cancelling: false,
    };
    opStore.ops = { r1: [op] };
    const { unmount } = render(OpLogStrip, { repoId: "r1" });

    // The op row renders even without any log output.
    expect(screen.getByText("sending pack")).not.toBeNull();
    expect(screen.getByText("40%")).not.toBeNull();

    await fireEvent.click(screen.getByRole("button", { name: "Cancel Push" }));

    // Store flipped the flag synchronously (as a new op object); the row
    // swaps to a spinner.
    expect(mockOpCancel).toHaveBeenCalledWith("r1", "op-9");
    expect(opStore.opsFor("r1")[0]?.cancelling).toBe(true);
    expect(screen.getByText("Cancelling…")).not.toBeNull();
    expect(screen.queryByRole("button", { name: "Cancel Push" })).toBeNull();
    unmount();
  });

  it("shows a cancellable op row even when no log session exists", () => {
    opStore.ops = {
      r1: [
        {
          op_id: "op-7",
          kind: "fetch",
          message: "contacting remote",
          pct: null,
          done: false,
          error: null,
          cancelling: false,
        },
      ],
    };
    render(OpLogStrip, { repoId: "r1" });
    expect(screen.getByText("Fetch")).not.toBeNull();
    expect(screen.getByRole("button", { name: "Cancel Fetch" })).not.toBeNull();
  });
});
