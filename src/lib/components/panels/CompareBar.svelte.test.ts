/**
 * CompareBar tests (phase 5 item 50): both compare modes render and
 * switch, the two-dot path diffs base→target directly, the three-dot path
 * resolves repoMergeBase first (and shows a hint instead of diffing when
 * the histories are unrelated), the target side can be a symbolic side
 * (worktree/index/HEAD), and the mode/target-kind choices persist per repo
 * root. The IPC layer and toast are mocked (same seam as the BranchPanel
 * suite); prefs ride the real localStorage (cleared per test).
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  repoDiff: vi.fn(),
  repoMergeBase: vi.fn(),
  repoRefs: vi.fn(),
  toast: vi.fn(),
}));

vi.mock("$lib/ipc/client", () => ({ ...mocks }));
vi.mock("$lib/toast", () => ({
  toast: mocks.toast,
  dismissToast: vi.fn(),
  pauseToast: vi.fn(),
  resumeToast: vi.fn(),
  getToasts: vi.fn(() => []),
}));

import CompareBar from "./CompareBar.svelte";
import { COMPARE_PREFS_PREFIX } from "./compare";
import type { FileDiff } from "$lib/ipc/types";

const FILES: FileDiff[] = [
  {
    path: "a.txt",
    old_path: null,
    binary: false,
    is_image: false,
    additions: 1,
    deletions: 0,
    hunks: [],
  },
];

function type(element: Element, value: string): void {
  fireEvent.input(element, { target: { value } });
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  mocks.repoRefs.mockResolvedValue([["main", "aaaa1111"], ["feature", "bbbb2222"]]);
  mocks.repoDiff.mockResolvedValue(FILES);
  mocks.repoMergeBase.mockResolvedValue("cccc3333");
});

afterEach(() => {
  cleanup();
  localStorage.clear();
});

describe("CompareBar modes", () => {
  it("renders both mode buttons (two-dot active by default)", () => {
    render(CompareBar, { repoId: "r1", root: "/repos/demo", onCompare: () => {} });

    const twoDot = screen.getByRole("button", { name: "A..B" });
    const threeDot = screen.getByRole("button", { name: "A...B" });
    expect(twoDot.getAttribute("aria-pressed")).toBe("true");
    expect(threeDot.getAttribute("aria-pressed")).toBe("false");
  });

  it("switches to three-dot and persists the choice per repo root", async () => {
    const { unmount } = render(CompareBar, {
      repoId: "r1",
      root: "/repos/demo",
      onCompare: () => {},
    });

    await fireEvent.click(screen.getByRole("button", { name: "A...B" }));
    expect(
      screen.getByRole("button", { name: "A...B" }).getAttribute("aria-pressed"),
    ).toBe("true");
    expect(localStorage.getItem(COMPARE_PREFS_PREFIX + "/repos/demo")).toBe(
      '{"mode":"threedot","targetKind":"commit"}',
    );

    // a fresh mount for the same root restores the persisted mode
    unmount();
    render(CompareBar, { repoId: "r2", root: "/repos/demo", onCompare: () => {} });
    expect(
      screen.getByRole("button", { name: "A...B" }).getAttribute("aria-pressed"),
    ).toBe("true");
  });

  it("two-dot compare diffs base→target directly and emits the result", async () => {
    const onCompare = vi.fn();
    render(CompareBar, { repoId: "r1", root: "/repos/demo", onCompare });

    type(screen.getByLabelText("Base ref"), "main");
    type(screen.getByLabelText("Target ref"), "feature");
    await fireEvent.click(screen.getByRole("button", { name: "Compare" }));

    await waitFor(() => {
      expect(onCompare).toHaveBeenCalledTimes(1);
    });
    expect(mocks.repoMergeBase).not.toHaveBeenCalled();
    expect(mocks.repoDiff).toHaveBeenCalledWith(
      "r1",
      { commit: "main" },
      { commit: "feature" },
    );
    const [files, base, target, sides] = onCompare.mock.calls[0]!;
    expect(files).toEqual(FILES);
    expect(base).toBe("main");
    expect(target).toBe("feature");
    expect(sides).toEqual({
      old: { commit: "main" },
      new: { commit: "feature" },
    });
  });

  it("three-dot resolves the merge base first and diffs base→target", async () => {
    const onCompare = vi.fn();
    render(CompareBar, { repoId: "r1", root: "/repos/demo", onCompare });

    await fireEvent.click(screen.getByRole("button", { name: "A...B" }));
    type(screen.getByLabelText("Base ref"), "main");
    type(screen.getByLabelText("Target ref"), "feature");
    await fireEvent.click(screen.getByRole("button", { name: "Compare" }));

    await waitFor(() => {
      expect(onCompare).toHaveBeenCalledTimes(1);
    });
    expect(mocks.repoMergeBase).toHaveBeenCalledWith("r1", "main", "feature");
    expect(mocks.repoDiff).toHaveBeenCalledWith(
      "r1",
      { commit: "cccc3333" },
      { commit: "feature" },
    );
    // the emitted base label carries the resolved merge base
    const [, base, target, sides] = onCompare.mock.calls[0]!;
    expect(base).toContain("cccc3333");
    expect(target).toBe("feature");
    expect(sides).toEqual({
      old: { commit: "cccc3333" },
      new: { commit: "feature" },
    });
  });

  it("three-dot without a merge base shows a hint and diffs nothing", async () => {
    mocks.repoMergeBase.mockResolvedValue(null);
    render(CompareBar, { repoId: "r1", root: "/repos/demo", onCompare: () => {} });

    await fireEvent.click(screen.getByRole("button", { name: "A...B" }));
    type(screen.getByLabelText("Base ref"), "orphan-a");
    type(screen.getByLabelText("Target ref"), "orphan-b");
    await fireEvent.click(screen.getByRole("button", { name: "Compare" }));

    await waitFor(() => {
      expect(screen.getByRole("status").textContent).toContain("no merge base");
    });
    expect(mocks.repoDiff).not.toHaveBeenCalled();
  });

  it("symbolic target sides disable the ref input and diff that side", async () => {
    const onCompare = vi.fn();
    render(CompareBar, { repoId: "r1", root: "/repos/demo", onCompare });

    type(screen.getByLabelText("Base ref"), "main");
    const targetInput = screen.getByLabelText("Target ref") as HTMLInputElement;
    expect(targetInput.disabled).toBe(false);

    await fireEvent.change(screen.getByLabelText("Target side"), {
      target: { value: "worktree" },
    });
    expect(targetInput.disabled).toBe(true);
    expect(localStorage.getItem(COMPARE_PREFS_PREFIX + "/repos/demo")).toBe(
      '{"mode":"twodot","targetKind":"worktree"}',
    );

    await fireEvent.click(screen.getByRole("button", { name: "Compare" }));
    await waitFor(() => {
      expect(onCompare).toHaveBeenCalledTimes(1);
    });
    expect(mocks.repoDiff).toHaveBeenCalledWith("r1", { commit: "main" }, "worktree");
    const [, base, target] = onCompare.mock.calls[0]!;
    expect(base).toBe("main");
    expect(target).toBe("worktree");
  });

  it("prefers the persisted target kind on mount and labels the compare", async () => {
    localStorage.setItem(
      COMPARE_PREFS_PREFIX + "/repos/demo",
      '{"mode":"twodot","targetKind":"head"}',
    );
    render(CompareBar, { repoId: "r1", root: "/repos/demo", onCompare: () => {} });

    expect((screen.getByLabelText("Target ref") as HTMLInputElement).disabled).toBe(
      true,
    );
    expect(
      (screen.getByLabelText("Target side") as HTMLSelectElement).value,
    ).toBe("head");
  });
});

describe("CompareBar basics", () => {
  it("requires both refs before diffing", async () => {
    render(CompareBar, { repoId: "r1", onCompare: () => {} });

    await fireEvent.click(screen.getByRole("button", { name: "Compare" }));
    expect(mocks.toast).toHaveBeenCalled();
    expect(mocks.repoDiff).not.toHaveBeenCalled();
  });

  it("toasts a descriptive failure when the diff rejects", async () => {
    mocks.repoDiff.mockRejectedValue(new Error("boom"));
    render(CompareBar, { repoId: "r1", onCompare: () => {} });

    type(screen.getByLabelText("Base ref"), "main");
    type(screen.getByLabelText("Target ref"), "feature");
    await fireEvent.click(screen.getByRole("button", { name: "Compare" }));

    await waitFor(() => {
      expect(mocks.toast).toHaveBeenCalledWith(
        expect.stringContaining("boom"),
        expect.objectContaining({ kind: "error" }),
      );
    });
  });
});
