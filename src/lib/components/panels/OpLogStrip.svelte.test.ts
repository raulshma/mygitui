/**
 * OpLogStrip: the newest hook/CLI output session renders with its op title,
 * can be collapsed/dismissed, and the component renders nothing when the
 * repo has no log session — including when ops are merely running (in-flight
 * op progress lives in the StatusBar, so this strip never mounts in flow for
 * a plain network op and twitch the workspace).
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { opStore } from "$lib/stores/ops.svelte";
import OpLogStrip from "./OpLogStrip.svelte";

// The op store binds `$lib/ipc/client` at module load (auth + cancel); the
// event seams are pulled in transitively. None are exercised in this suite.
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

afterEach(() => {
  cleanup();
  opStore.logs = {};
  opStore.ops = {};
});

describe("OpLogStrip", () => {
  it("renders nothing when the repo has no log session", () => {
    render(OpLogStrip, { repoId: "r1" });
    expect(screen.queryByText("Commit output")).toBeNull();
  });

  it("renders nothing while ops are merely running (progress moved to the StatusBar)", () => {
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
    // No op row, no cancel affordance, no strip section at all.
    expect(screen.queryByText("contacting remote")).toBeNull();
    expect(screen.queryByRole("button", { name: "Cancel Fetch" })).toBeNull();
    expect(screen.queryByRole("region")).toBeNull();
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
});
