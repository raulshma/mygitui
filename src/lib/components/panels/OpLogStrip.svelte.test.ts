/**
 * OpLogStrip: newest hook/CLI output is rendered with its op title, can be
 * collapsed/dismissed, and the component renders nothing when the repo has no
 * log session.
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, describe, expect, it } from "vitest";
import { opStore } from "$lib/stores/ops.svelte";
import OpLogStrip from "./OpLogStrip.svelte";

afterEach(() => {
  cleanup();
  opStore.logs = {};
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
});
