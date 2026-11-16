/**
 * Unit tests for the dynamic window title store (`$lib/stores/
 * windowTitle.svelte`). The IPC client is fully mocked (`vi.mock`) — no
 * Tauri runtime is touched; outside a webview `syncWindowTitle` only
 * mirrors the derived title into `windowTitle.current` and never calls
 * the window API.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  closeRepo,
  onRepoChanged,
  openRepo,
  remoteRepoOpen,
  repoStatus,
} from "$lib/ipc/client";
import { resetRepoEvents } from "$lib/events/repoEvents";
import type { RepoStatus, StatusEntry } from "$lib/ipc/types";
import { openTab, resetTabStore, tabStore } from "$lib/stores/tabs.svelte";
import {
  syncWindowTitle,
  windowTitle,
} from "$lib/stores/windowTitle.svelte";

vi.mock("$lib/ipc/client", () => ({
  openRepo: vi.fn(),
  closeRepo: vi.fn(),
  repoStatus: vi.fn(),
  remoteRepoOpen: vi.fn(),
  onRepoChanged: vi.fn(),
  // Seams tabs.svelte (and its cache stores) import transitively; never
  // called in this suite.
  onCloneProgress: vi.fn(),
  onOpProgress: vi.fn(),
  onOpLog: vi.fn(),
  onAuthRequest: vi.fn(),
  onActionOutput: vi.fn(),
  onPtyOutput: vi.fn(),
  onPtyExit: vi.fn(),
  conflicts: vi.fn(),
  commitActivity: vi.fn(),
  contributorStats: vi.fn(),
  projectDetect: vi.fn(),
}));

const mockOpenRepo = vi.mocked(openRepo);
const mockRepoStatus = vi.mocked(repoStatus);

function repoInfo(repoId: string, root: string, name: string) {
  return {
    repo_id: repoId,
    root,
    name,
    bare: false,
    git_dir: `${root}/.git`,
    device_id: null,
  };
}

function status(branch: string, entries: StatusEntry[] = []): RepoStatus {
  return {
    branch,
    head: "abc123def456",
    detached: false,
    ahead: 0,
    behind: 0,
    merging: false,
    rebasing: false,
    sequencer: false,
    bisecting: false,
    entries,
  };
}

const ALPHA = repoInfo("repo-1", "C:/repos/alpha", "alpha");
const BETA = repoInfo("repo-2", "C:/repos/beta", "beta");

const DIRTY_ENTRY: StatusEntry = {
  path: "src/main.rs",
  old_path: null,
  index: "modified",
  worktree: "unmodified",
};

beforeEach(() => {
  mockOpenRepo.mockReset();
  mockRepoStatus.mockReset();
  mockRepoStatus.mockResolvedValue(status("main"));
});

afterEach(() => {
  resetTabStore(); // deregisters the store's listener
  resetRepoEvents(); // drops the module-owned backend subscription
  // Drop the popout query so later suites see a main-window URL.
  window.history.replaceState(null, "", "/");
});

describe("syncWindowTitle", () => {
  it("derives the title from the active tab with the dirty marker", async () => {
    mockOpenRepo.mockResolvedValue(ALPHA);
    mockRepoStatus.mockResolvedValue(status("main", [DIRTY_ENTRY]));
    await openTab("C:/repos/alpha");
    // openTab resolves before the background status fetch lands.
    await vi.waitFor(() => {
      expect(tabStore.active?.status?.entries).toHaveLength(1);
    });

    syncWindowTitle();

    expect(windowTitle.current).toBe("alpha * — mygitui");
  });

  it("skips the title sync in popout webviews so the native title set by open_popout survives", async () => {
    mockOpenRepo.mockResolvedValue(BETA);
    await openTab("C:/repos/beta");
    syncWindowTitle();
    expect(windowTitle.current).toBe("beta — mygitui");

    // A popout URL (`?panel=…`) flips the guard: no tab-store read, no
    // push — `windowTitle.current` stays at the last main-window value.
    window.history.replaceState(null, "", "/?panel=diff&repo=repo-1");
    mockOpenRepo.mockResolvedValue(ALPHA);
    await openTab("C:/repos/alpha");
    syncWindowTitle();

    expect(windowTitle.current).toBe("beta — mygitui");
  });
});
