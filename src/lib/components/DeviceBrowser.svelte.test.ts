/**
 * DeviceBrowser tabs: Repositories is the default tab and auto-scans the
 * device home on mount; Browse files shows the one-level directory listing;
 * scan failures surface in the Repositories tab and "Open as project"
 * routes through the tab store's remote open.
 *
 * IPC is faked via the transport seam (`setTransport`, same as TabStrip's
 * suite); the tab store is mocked so open actions are asserted directly.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetTransport, setTransport, type Transport } from "$lib/ipc/client";

const openRemoteTab = vi.hoisted(() => vi.fn());

vi.mock("$lib/stores/tabs.svelte", () => ({ openRemoteTab }));

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
