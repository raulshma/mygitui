/**
 * DeviceBrowser tabs: Repositories is the default tab and auto-scans the
 * device home on mount; Browse files shows the one-level directory listing;
 * scan failures surface in the Repositories tab, "Open as project" routes
 * through the tab store's remote open, and clicking a browsed file seeds
 * the containing repo's workspace session (the editor flow) before opening
 * or activating its tab.
 *
 * IPC is faked via the transport seam (`setTransport`, same as TabStrip's
 * suite); the tab store and workspace session are mocked so open/seed
 * actions are asserted directly.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetTransport, setTransport, type Transport } from "$lib/ipc/client";

const openRemoteTab = vi.hoisted(() => vi.fn());
const setActive = vi.hoisted(() => vi.fn());
const mockTabs = vi.hoisted(() => [] as Array<{ deviceId: string | null; openError: string | null; root: string; id: string }>);
const workspacePatch = vi.hoisted(() => vi.fn());
const workspaceGet = vi.hoisted(() => vi.fn<() => unknown>(() => null));
const toastMock = vi.hoisted(() => vi.fn());

/** The canonical ssh:// URI builder (mirrors $lib/stores/tabs.svelte). */
function sshUri(deviceId: string, path: string): string {
  return `ssh://${deviceId}/${path.replace(/^\/+/, "")}`;
}

vi.mock("$lib/stores/tabs.svelte", () => ({
  openRemoteTab,
  setActive,
  sshUri,
  // `tabStore` is only read for open-tab matching; tests assign the array.
  get tabStore() {
    return { tabs: mockTabs };
  },
}));
vi.mock("$lib/stores/workspaceSession.svelte", () => ({
  workspaceSession: { patch: workspacePatch, get: workspaceGet },
}));
vi.mock("$lib/toast", () => ({ toast: toastMock }));

import DeviceBrowser from "./DeviceBrowser.svelte";

const DEVICE = "dev-1";

interface DirEntry {
  name: string;
  path: string;
  kind: string;
  size: number | null;
}

function dirListing(entries: DirEntry[]) {
  return { path: "/home/user", entries, truncated: false };
}

/** Transport fake over the three device commands the browser drives. */
function makeTransport(overrides: {
  scan?: unknown[] | Error;
  listing?: DirEntry[] | Error;
}): Transport {
  return async (command: string) => {
    switch (command) {
      case "device_scan_repos":
        if (overrides.scan instanceof Error) throw overrides.scan;
        return overrides.scan ?? [];
      case "device_browse_dir":
        if (overrides.listing instanceof Error) throw overrides.listing;
        return dirListing(overrides.listing ?? []);
      default:
        return null;
    }
  };
}

beforeEach(() => {
  openRemoteTab.mockReset();
  setActive.mockReset();
  workspacePatch.mockReset();
  workspaceGet.mockReset();
  workspaceGet.mockReturnValue(null);
  toastMock.mockReset();
  mockTabs.length = 0;
  setTransport(makeTransport({}));
});

afterEach(() => {
  cleanup();
  resetTransport();
});

describe("DeviceBrowser tabs", () => {
  it("defaults to the Repositories tab and auto-scans the home directory", async () => {
    const scan = vi.fn();
    const transport: Transport = async (command) => {
      if (command === "device_scan_repos") {
        scan();
        return [
          { name: "alpha", path: "/home/user/alpha" },
          { name: "beta", path: "/home/user/code/beta" },
        ];
      }
      return dirListing([]);
    };
    setTransport(transport);

    render(DeviceBrowser, { deviceId: DEVICE });
    expect(
      await screen.findByText("alpha", { selector: ".row-name" }),
    ).toBeTruthy();
    expect(screen.getByText("beta", { selector: ".row-name" })).toBeTruthy();
    expect(scan).toHaveBeenCalledTimes(1);
    // Summary + open actions render with the results.
    expect(screen.getByText(/2 repositories under the home directory/)).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Open as project" })).toHaveLength(2);
  });

  it("shows the empty state when the scan finds nothing", async () => {
    render(DeviceBrowser, { deviceId: DEVICE });
    expect(
      await screen.findByText("No repositories found under the home directory."),
    ).toBeTruthy();
  });

  it("surfaces scan failures in the Repositories tab", async () => {
    setTransport(
      makeTransport({ scan: new Error("device offline"), listing: [] }),
    );
    render(DeviceBrowser, { deviceId: DEVICE });
    expect(await screen.findByText("device offline")).toBeTruthy();
  });

  it("opens a scanned repo through the tab store's remote open", async () => {
    setTransport(async (command: string) => {
      if (command === "device_scan_repos") {
        return [{ name: "alpha", path: "/home/user/alpha" }];
      }
      return dirListing([]);
    });
    render(DeviceBrowser, { deviceId: DEVICE });
    const open = await screen.findByRole("button", { name: "Open as project" });
    await open.click();
    await tick();
    expect(openRemoteTab).toHaveBeenCalledWith(DEVICE, "/home/user/alpha");
  });

  it("Browse files shows the breadcrumb and the directory listing", async () => {
    setTransport(
      makeTransport({
        scan: [],
        listing: [
          { name: "code", path: "/home/user/code", kind: "dir", size: null },
          { name: "notes.txt", path: "/home/user/notes.txt", kind: "file", size: 12 },
        ],
      }),
    );
    render(DeviceBrowser, { deviceId: DEVICE });
    await screen.findByText("No repositories found under the home directory.");

    fireEvent.click(screen.getByRole("tab", { name: "Browse files" }));
    expect(await screen.findByText("code", { selector: ".row-name" })).toBeTruthy();
    // Files render static (no button), dirs navigate.
    expect(screen.getByText("notes.txt", { selector: ".row-name" })).toBeTruthy();
    expect(screen.getByText("Home")).toBeTruthy();
    expect(screen.getByRole("button", { name: "code" })).toBeTruthy();
  });

  it("returns to repos without re-scanning (results are kept)", async () => {
    const scan = vi.fn();
    const transport: Transport = async (command) => {
      if (command === "device_scan_repos") {
        scan();
        return [{ name: "alpha", path: "/home/user/alpha" }];
      }
      return dirListing([]);
    };
    setTransport(transport);

    render(DeviceBrowser, { deviceId: DEVICE });
    await screen.findByText("alpha", { selector: ".row-name" });
    expect(scan).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("tab", { name: "Browse files" }));
    await screen.findByText("Home");
    fireEvent.click(screen.getByRole("tab", { name: "Repositories" }));
    await screen.findByText("alpha", { selector: ".row-name" });
    expect(scan).toHaveBeenCalledTimes(1);
  });
});

describe("DeviceBrowser file open (item 60)", () => {
  /** Renders with one scanned repo and browses into its directory. */
  async function browseRepoFiles(): Promise<void> {
    setTransport(async (command) => {
      if (command === "device_scan_repos") {
        return [{ name: "alpha", path: "/srv/app" }];
      }
      if (command === "device_browse_dir") {
        return dirListing([
          { name: "app", path: "/srv/app/app", kind: "dir", size: null },
          { name: "main.ts", path: "/srv/app/main.ts", kind: "file", size: 10 },
        ]);
      }
      return dirListing([]);
    });
    render(DeviceBrowser, { deviceId: DEVICE });
    await screen.findByText("alpha", { selector: ".row-name" });
    fireEvent.click(screen.getByRole("tab", { name: "Browse files" }));
    await screen.findByText("main.ts", { selector: ".row-name" });
  }

  it("a file inside an OPEN repo tab activates that tab and seeds the editor session", async () => {
    await browseRepoFiles();
    const URI = sshUri(DEVICE, "/srv/app");
    mockTabs.push({ deviceId: DEVICE, openError: null, root: URI, id: "r1" });

    fireEvent.click(screen.getByRole("button", { name: /main\.ts/ }));
    await tick();

    // The workspace session is seeded (repo-relative path, files center)
    // and the tab is activated — the RepoView switch restore opens it.
    expect(workspacePatch).toHaveBeenCalledWith(URI, {
      openFiles: ["main.ts"],
      activeFile: "main.ts",
      centerKind: "files",
    });
    expect(setActive).toHaveBeenCalledWith("r1");
    expect(openRemoteTab).not.toHaveBeenCalled();
  });

  it("merges into the repo's existing session instead of clobbering it", async () => {
    await browseRepoFiles();
    const URI = sshUri(DEVICE, "/srv/app");
    mockTabs.push({ deviceId: DEVICE, openError: null, root: URI, id: "r1" });
    workspaceGet.mockReturnValue({
      openFiles: ["README.md"],
      activeFile: "README.md",
    });

    fireEvent.click(screen.getByRole("button", { name: /main\.ts/ }));
    await tick();

    expect(workspacePatch).toHaveBeenCalledWith(URI, {
      openFiles: ["README.md", "main.ts"],
      activeFile: "main.ts",
      centerKind: "files",
    });
  });

  it("without an open tab, the nearest scanned repo is opened and seeded", async () => {
    await browseRepoFiles();

    fireEvent.click(screen.getByRole("button", { name: /main\.ts/ }));
    await tick();

    expect(openRemoteTab).toHaveBeenCalledWith(DEVICE, "/srv/app");
    expect(workspacePatch).toHaveBeenCalledWith(sshUri(DEVICE, "/srv/app"), {
      openFiles: ["main.ts"],
      activeFile: "main.ts",
      centerKind: "files",
    });
    expect(setActive).not.toHaveBeenCalled();
  });

  it("a file outside every open tab and scanned repo stays read-only guidance", async () => {
    setTransport(async (command) => {
      if (command === "device_scan_repos") {
        return [{ name: "alpha", path: "/srv/other" }];
      }
      if (command === "device_browse_dir") {
        return dirListing([
          { name: "main.ts", path: "/srv/app/main.ts", kind: "file", size: 10 },
        ]);
      }
      return dirListing([]);
    });
    render(DeviceBrowser, { deviceId: DEVICE });
    await screen.findByText("alpha", { selector: ".row-name" });
    fireEvent.click(screen.getByRole("tab", { name: "Browse files" }));
    await screen.findByText("main.ts", { selector: ".row-name" });

    fireEvent.click(screen.getByRole("button", { name: /main\.ts/ }));
    await tick();

    expect(workspacePatch).not.toHaveBeenCalled();
    expect(setActive).not.toHaveBeenCalled();
    expect(openRemoteTab).not.toHaveBeenCalled();
    // Read-only guidance toast instead of a silent no-op.
    expect(toastMock).toHaveBeenCalledWith(
      "Open the repository containing this file to edit its files.",
      { kind: "info" },
    );
  });

  it("the longest matching open-tab root wins (repo inside repo)", async () => {
    setTransport(async (command) => {
      if (command === "device_scan_repos") {
        return [
          { name: "app", path: "/srv" },
          { name: "nested", path: "/srv/app" },
        ];
      }
      if (command === "device_browse_dir") {
        return dirListing([
          { name: "main.ts", path: "/srv/app/main.ts", kind: "file", size: 10 },
        ]);
      }
      return dirListing([]);
    });
    render(DeviceBrowser, { deviceId: DEVICE });
    await screen.findByText("app", { selector: ".row-name" });
    fireEvent.click(screen.getByRole("tab", { name: "Browse files" }));
    await screen.findByText("main.ts", { selector: ".row-name" });
    mockTabs.push(
      { deviceId: DEVICE, openError: null, root: sshUri(DEVICE, "/srv"), id: "r-outer" },
      { deviceId: DEVICE, openError: null, root: sshUri(DEVICE, "/srv/app"), id: "r-inner" },
    );

    fireEvent.click(screen.getByRole("button", { name: /main\.ts/ }));
    await tick();

    // /srv/app is the longer (more specific) containing root.
    expect(workspacePatch).toHaveBeenCalledWith(
      sshUri(DEVICE, "/srv/app"),
      expect.objectContaining({ activeFile: "main.ts" }),
    );
    expect(setActive).toHaveBeenCalledWith("r-inner");
  });
});
