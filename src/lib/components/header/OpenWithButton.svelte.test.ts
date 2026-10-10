import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorApp } from "$lib/ipc/types";

const mocks = vi.hoisted(() => ({
  detectEditors: vi.fn(),
  openWith: vi.fn(),
}));

vi.mock("$lib/ipc/client", () => ({
  detectEditors: mocks.detectEditors,
  openWith: mocks.openWith,
}));

vi.mock("$lib/toast", () => ({
  toast: vi.fn(),
  dismissToast: vi.fn(),
  pauseToast: vi.fn(),
  resumeToast: vi.fn(),
  getToasts: vi.fn(() => []),
}));

import { toast } from "$lib/toast";
import OpenWithButton from "./OpenWithButton.svelte";

function app(patch: Partial<EditorApp> = {}): EditorApp {
  return { id: "explorer", name: "Explorer", path: "explorer", ...patch };
}

describe("OpenWithButton", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
  });

  it("opens with the last-used app on primary click and persists the choice", async () => {
    mocks.detectEditors.mockResolvedValue([
      app(),
      app({ id: "vscode", name: "VS Code", path: "C:/Programs/Code.exe" }),
    ]);
    mocks.openWith.mockResolvedValue(undefined);

    render(OpenWithButton, { root: "/tmp/repo" });

    // Primary starts as Explorer (default), then opens with it.
    const primary = await screen.findByRole("button", { name: "Open with Explorer" });
    await fireEvent.click(primary);

    await waitFor(() =>
      expect(mocks.openWith).toHaveBeenCalledWith("/tmp/repo", "explorer"),
    );

    // Re-render: the persisted choice is restored (still Explorer).
    expect(localStorage.getItem("mygitui.openwith.last")).toBe("explorer");
  });

  it("primary reopens with the app chosen from the menu", async () => {
    mocks.detectEditors.mockResolvedValue([
      app(),
      app({ id: "vscode", name: "VS Code", path: "C:/Programs/Code.exe" }),
    ]);
    mocks.openWith.mockResolvedValue(undefined);

    // Pre-seed the last-used choice: VS Code.
    localStorage.setItem("mygitui.openwith.last", "vscode");

    render(OpenWithButton, { root: "/tmp/repo" });

    const primary = await screen.findByRole("button", { name: "Open with VS Code" });
    await fireEvent.click(primary);

    await waitFor(() =>
      expect(mocks.openWith).toHaveBeenCalledWith("/tmp/repo", "vscode"),
    );
  });

  it("toasts when the launch fails", async () => {
    mocks.detectEditors.mockResolvedValue([app()]);
    mocks.openWith.mockRejectedValue("spawn failed");

    render(OpenWithButton, { root: "/tmp/repo" });
    const primary = await screen.findByRole("button", { name: "Open with Explorer" });
    await fireEvent.click(primary);

    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith(
        expect.stringContaining("Could not open Explorer"),
        { kind: "error" },
      ),
    );
  });
});
