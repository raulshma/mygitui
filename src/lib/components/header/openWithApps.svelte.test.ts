/**
 * "Open with" module (`openWithApps.svelte.ts`): the logic behind the repo
 * header's overflow submenu (the Workstream A home of the old
 * `OpenWithButton` split button). Covers the same behaviors the button's
 * tests did: last-used persistence (incl. the `custom:` namespace),
 * fallbacks for stale ids, launch routing detected vs custom, launch/detect
 * failure toasts (detection once per session), and menu entry ordering.
 */
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import type { EditorApp } from "$lib/ipc/types";
import { contextMenu } from "$lib/components/menu/contextMenuStore.svelte";
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
import {
  currentEntry,
  customEntries,
  openWithEntry,
  refreshApps,
} from "./openWithApps.svelte";

function app(patch: Partial<EditorApp> = {}): EditorApp {
  return { id: "explorer", name: "Explorer", path: "explorer", ...patch };
}

beforeEach(async () => {
  localStorage.clear();
  // The module binds the app-wide singleton: start every test from empty.
  for (const a of customApps.list()) customApps.remove(a.id);
  vi.clearAllMocks();
  // The last-used id is reactive state now, re-synced from storage on each
  // probe (that is how the component reaches it) — probe like the mount does.
  mocks.detectEditors.mockResolvedValue([]);
  await refreshApps();
});

afterEach(() => {
  contextMenu.hide();
});

describe("openWithApps currentEntry (last-used fallbacks)", () => {
  it("falls back to Explorer (default) before any choice is persisted", () => {
    expect(currentEntry([app()])?.id).toBe("explorer");
  });

  it("prefers the persisted last-used detected app", async () => {
    localStorage.setItem("mygitui.openwith.last", "vscode");
    await refreshApps();
    const entry = currentEntry([
      app(),
      { id: "vscode", name: "VS Code", path: "C:/Programs/Code.exe" },
    ]);
    expect(entry?.name).toBe("VS Code");
  });

  it("restores a persisted custom last-used id from the store", async () => {
    const added = customApps.add({ name: "Neovim", command: "kitty {path}" });
    localStorage.setItem("mygitui.openwith.last", `custom:${added.id}`);
    await refreshApps();
    const entry = currentEntry([app()]);
    expect(entry?.custom).toEqual({
      id: added.id,
      name: "Neovim",
      command: "kitty {path}",
    });
  });

  it("falls back to Explorer when the persisted custom id no longer exists", async () => {
    localStorage.setItem("mygitui.openwith.last", "custom:gone-app");
    await refreshApps();
    expect(currentEntry([app()])?.id).toBe("explorer");
  });

  it("reflects a launch in currentEntry without a re-probe (reactive mirror)", async () => {
    // The pinned split button's main segment derives from currentEntry; a
    // launch must flip it live (no menu re-open to re-read storage).
    mocks.openWith.mockResolvedValue(undefined);
    await openWithEntry("/tmp/repo", { id: "vscode", name: "VS Code" });
    expect(
      currentEntry([app(), { id: "vscode", name: "VS Code", path: "code" }])?.id,
    ).toBe("vscode");
  });
});

describe("openWithApps openWithEntry (launch + persistence)", () => {
  it("opens with a detected app and persists the choice", async () => {
    mocks.openWith.mockResolvedValue(undefined);

    await openWithEntry("/tmp/repo", { id: "explorer", name: "Explorer" });

    await vi.waitFor(() =>
      expect(mocks.openWith).toHaveBeenCalledWith("/tmp/repo", "explorer", undefined),
    );
    expect(mocks.openWithCustom).not.toHaveBeenCalled();
    expect(localStorage.getItem("mygitui.openwith.last")).toBe("explorer");
  });

  it("launches custom apps via openWithCustom and persists the namespaced id", async () => {
    mocks.openWithCustom.mockResolvedValue(undefined);
    const added = customApps.add({ name: "Neovim", command: "kitty {path}" });

    await openWithEntry("/tmp/repo", {
      id: `custom:${added.id}`,
      name: "Neovim",
      custom: { id: added.id, name: "Neovim", command: "kitty {path}" },
    });

    await vi.waitFor(() =>
      expect(mocks.openWithCustom).toHaveBeenCalledWith("/tmp/repo", {
        id: added.id,
        name: "Neovim",
        command: "kitty {path}",
      }),
    );
    expect(mocks.openWith).not.toHaveBeenCalled();
    expect(localStorage.getItem("mygitui.openwith.last")).toBe(
      `custom:${added.id}`,
    );
  });

  it("toasts when the launch fails", async () => {
    mocks.openWith.mockRejectedValue("spawn failed");

    await openWithEntry("/tmp/repo", { id: "explorer", name: "Explorer" });

    await vi.waitFor(() =>
      expect(toast).toHaveBeenCalledWith(
        expect.stringContaining("Could not open Explorer"),
        { kind: "error" },
      ),
    );
  });

  it("normalizes ssh uri root to clean remote filesystem path and extracts deviceId", async () => {
    mocks.openWith.mockResolvedValue(undefined);

    await openWithEntry("ssh://dev-1/var/www/my-repo", {
      id: "vscode",
      name: "VS Code",
    });

    await vi.waitFor(() =>
      expect(mocks.openWith).toHaveBeenCalledWith(
        "/var/www/my-repo",
        "vscode",
        "dev-1",
      ),
    );
  });

  it("preserves explicit deviceId while normalizing ssh uri root", async () => {
    mocks.openWith.mockResolvedValue(undefined);

    await openWithEntry(
      "ssh://dev-1/var/www/my-repo",
      { id: "vscode", name: "VS Code" },
      "dev-1",
    );

    await vi.waitFor(() =>
      expect(mocks.openWith).toHaveBeenCalledWith(
        "/var/www/my-repo",
        "vscode",
        "dev-1",
      ),
    );
  });
});

describe("openWithApps refreshApps (detection)", () => {
  it("resolves the detected apps", async () => {
    mocks.detectEditors.mockResolvedValue([
      app(),
      app({ id: "vscode", name: "VS Code", path: "x" }),
    ]);
    await expect(refreshApps()).resolves.toHaveLength(2);
  });

  it("toasts detection failures once per session, then stays quiet", async () => {
    mocks.detectEditors
      .mockRejectedValueOnce(new Error("probe failed"))
      .mockRejectedValue(new Error("probe failed again"));

    await refreshApps();
    await vi.waitFor(() =>
      expect(toast).toHaveBeenCalledWith(
        expect.stringContaining("Could not detect editors: probe failed"),
        { kind: "error" },
      ),
    );

    // Second failure: the once-per-session guard keeps it quiet.
    await refreshApps();
    expect(
      vi.mocked(toast).mock.calls.filter((call) =>
        String(call[0]).includes("Could not detect editors"),
      ),
    ).toHaveLength(1);
  });
});

describe("openWithApps menu entries", () => {
  it("lists custom apps (namespaced ids) for the menu", () => {
    const added = customApps.add({ name: "Neovim", command: "kitty {path}" });
    expect(customEntries()).toEqual([
      {
        id: `custom:${added.id}`,
        name: "Neovim",
        custom: { id: added.id, name: "Neovim", command: "kitty {path}" },
      },
    ]);
  });
});
