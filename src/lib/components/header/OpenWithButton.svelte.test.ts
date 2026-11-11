import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorApp } from "$lib/ipc/types";
import { contextMenu } from "$lib/components/menu/contextMenuStore.svelte";
import ContextMenu from "$lib/components/menu/ContextMenu.svelte";
import { customApps } from "$lib/openwith/customApps.svelte";

const mocks = vi.hoisted(() => ({
  detectEditors: vi.fn(),
  openWith: vi.fn(),
  openWithCustom: vi.fn(),
}));

vi.mock("$lib/ipc/client", () => ({
  detectEditors: mocks.detectEditors,
  openWith: mocks.openWith,
  openWithCustom: mocks.openWithCustom,
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
    // The menu binds the app-wide singleton: start every test from empty.
    for (const app of customApps.list()) customApps.remove(app.id);
    vi.clearAllMocks();
  });

  afterEach(() => {
    cleanup();
    contextMenu.hide();
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

  it("lists custom apps after the detected ones and launches via openWithCustom", async () => {
    mocks.detectEditors.mockResolvedValue([
      app(),
      app({ id: "vscode", name: "VS Code", path: "C:/Programs/Code.exe" }),
    ]);
    mocks.openWithCustom.mockResolvedValue(undefined);

    const added = customApps.add({ name: "Neovim", command: "kitty {path}" });

    render(OpenWithButton, { root: "/tmp/repo" });
    render(ContextMenu);

    await screen.findByRole("button", { name: "Open with Explorer" });
    await fireEvent.click(
      screen.getByRole("button", { name: "Choose app to open the repository with" }),
    );

    const detected = await screen.findByText("VS Code");
    const custom = screen.getByText("Neovim");
    // Custom apps come after the detected ones (divider between).
    expect(
      detected.compareDocumentPosition(custom) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(screen.getByText("Manage custom apps…")).not.toBeNull();

    await fireEvent.click(custom);

    await waitFor(() =>
      expect(mocks.openWithCustom).toHaveBeenCalledWith("/tmp/repo", {
        id: added.id,
        name: "Neovim",
        command: "kitty {path}",
      }),
    );
    expect(mocks.openWith).not.toHaveBeenCalled();
    // The persisted last-used id carries the custom: namespace prefix.
    expect(localStorage.getItem("mygitui.openwith.last")).toBe(
      `custom:${added.id}`,
    );
  });

  it("restores a persisted custom last-used id from the store as the primary", async () => {
    mocks.detectEditors.mockResolvedValue([app()]);
    mocks.openWithCustom.mockResolvedValue(undefined);

    const added = customApps.add({ name: "Neovim", command: "kitty {path}" });
    localStorage.setItem("mygitui.openwith.last", `custom:${added.id}`);

    render(OpenWithButton, { root: "/tmp/repo" });

    const primary = await screen.findByRole("button", { name: "Open with Neovim" });
    await fireEvent.click(primary);

    await waitFor(() =>
      expect(mocks.openWithCustom).toHaveBeenCalledWith("/tmp/repo", {
        id: added.id,
        name: "Neovim",
        command: "kitty {path}",
      }),
    );
    expect(mocks.openWith).not.toHaveBeenCalled();
  });

  it("falls back to Explorer when the persisted custom id no longer exists", async () => {
    mocks.detectEditors.mockResolvedValue([app()]);
    localStorage.setItem("mygitui.openwith.last", "custom:gone-app");

    render(OpenWithButton, { root: "/tmp/repo" });

    await screen.findByRole("button", { name: "Open with Explorer" });
    expect(mocks.openWithCustom).not.toHaveBeenCalled();
  });

  it("Manage custom apps… opens the manager dialog", async () => {
    mocks.detectEditors.mockResolvedValue([app()]);

    render(OpenWithButton, { root: "/tmp/repo" });
    render(ContextMenu);

    await screen.findByRole("button", { name: "Open with Explorer" });
    await fireEvent.click(
      screen.getByRole("button", { name: "Choose app to open the repository with" }),
    );

    await fireEvent.click(await screen.findByText("Manage custom apps…"));

    expect(
      await screen.findByText('Custom "Open with" apps'),
    ).not.toBeNull();
    // The manager section is embedded inside the dialog.
    expect(screen.getByRole("button", { name: "Add custom app" })).not.toBeNull();
  });

  it("re-detects on every menu open and toasts detection failures once", async () => {
    // One resolution per probe: mount, open 1, open 2, open 3 (fail), open 4
    // (fail). The custom app keeps the chevron clickable across failures.
    const detected = [app(), app({ id: "vscode", name: "VS Code", path: "x" })];
    customApps.add({ name: "Neovim", command: "kitty {path}" });
    mocks.detectEditors
      .mockResolvedValueOnce(detected)
      .mockResolvedValueOnce(detected)
      .mockResolvedValueOnce([app()])
      .mockRejectedValueOnce(new Error("probe failed"))
      .mockRejectedValueOnce(new Error("probe failed again"));

    render(OpenWithButton, { root: "/tmp/repo" });
    render(ContextMenu);

    const chevron = await screen.findByRole("button", {
      name: "Choose app to open the repository with",
    });

    // First open: two apps.
    await fireEvent.click(chevron);
    expect(await screen.findByText("VS Code")).not.toBeNull();
    contextMenu.hide();

    // Second open: fresh detection finds only Explorer — the old session
    // cache is gone.
    await fireEvent.click(chevron);
    await waitFor(() => expect(screen.queryByText("VS Code")).toBeNull());
    expect(screen.getByText("Explorer")).not.toBeNull();
    contextMenu.hide();

    // The session-cached failure toast fires on the first failed probe…
    await fireEvent.click(chevron);
    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith(
        expect.stringContaining("Could not detect editors: probe failed"),
        { kind: "error" },
      ),
    );
    contextMenu.hide();

    // …and stays quiet for later failures (once-per-session).
    await fireEvent.click(chevron);
    await waitFor(() =>
      expect(
        vi.mocked(toast).mock.calls.filter((call) =>
          String(call[0]).includes("Could not detect editors"),
        ),
      ).toHaveLength(1),
    );
  });
});
