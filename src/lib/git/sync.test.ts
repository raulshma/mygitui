/**
 * Unit tests for the shared sync verbs ($lib/git/sync). The IPC client is
 * mocked - no Tauri runtime is touched. These tests pin remote selection and
 * option construction, because the header quick actions and the command
 * palette must always issue identical operations.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { NetStats, RemoteInfo } from "$lib/ipc/types";

const mocks = vi.hoisted(() => ({
  fetchRepo: vi.fn(),
  pullRepo: vi.fn(),
  pushRepo: vi.fn(),
  remotes: vi.fn(),
}));

vi.mock("$lib/ipc/client", () => ({
  fetchRepo: mocks.fetchRepo,
  pullRepo: mocks.pullRepo,
  pushRepo: mocks.pushRepo,
  remotes: mocks.remotes,
}));

import {
  fetchDefaultRemote,
  fetchOptions,
  netStatsSummary,
  primaryRemoteName,
  pullCurrentBranch,
  pullOptions,
  pushCurrentBranch,
  pushOptions,
  syncSkipMessage,
  syncSuccessMessage,
} from "./sync";

const STATS: NetStats = {
  received_bytes: 0,
  objects: 0,
  updated_refs: [],
};

function remote(name: string): RemoteInfo {
  return { name, url: `https://host/${name}.git`, push_url: null };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("primaryRemoteName", () => {
  it("prefers origin over the configured order", () => {
    expect(primaryRemoteName([remote("upstream"), remote("origin")])).toBe(
      "origin",
    );
  });

  it("falls back to the first configured remote", () => {
    expect(primaryRemoteName([remote("upstream"), remote("fork")])).toBe(
      "upstream",
    );
  });

  it("returns null when the repo has no remotes", () => {
    expect(primaryRemoteName([])).toBeNull();
  });
});

describe("option construction", () => {
  it("fetches every ref with the requested prune flag", () => {
    expect(fetchOptions("origin", true)).toEqual({
      remote: "origin",
      prune: true,
      refs: [],
      depth: null,
    });
  });

  it("pulls fast-forward only without rebasing", () => {
    expect(pullOptions("origin", "main")).toEqual({
      remote: "origin",
      branch: "main",
      ff_only: true,
      rebase: false,
    });
  });

  it("pushes plainly: no force, upstream change, tags, or deletes", () => {
    expect(pushOptions("origin", "main")).toEqual({
      remote: "origin",
      branch: "main",
      force: false,
      force_with_lease: false,
      set_upstream: false,
      refs: [],
      tags: false,
      delete: false,
    });
  });
});

describe("fetchDefaultRemote", () => {
  it("fetches the primary remote and reports it", async () => {
    mocks.remotes.mockResolvedValue([remote("upstream"), remote("origin")]);
    mocks.fetchRepo.mockResolvedValue(STATS);

    await expect(fetchDefaultRemote("r1")).resolves.toEqual({
      kind: "done",
      remote: "origin",
      stats: STATS,
    });
    expect(mocks.fetchRepo).toHaveBeenCalledWith(
      "r1",
      fetchOptions("origin", true),
    );
  });

  it("honors an explicit prune=false", async () => {
    mocks.remotes.mockResolvedValue([remote("origin")]);
    mocks.fetchRepo.mockResolvedValue(STATS);

    await fetchDefaultRemote("r1", false);

    expect(mocks.fetchRepo).toHaveBeenCalledWith(
      "r1",
      fetchOptions("origin", false),
    );
  });

  it("skips without calling the network when no remote is configured", async () => {
    mocks.remotes.mockResolvedValue([]);

    await expect(fetchDefaultRemote("r1")).resolves.toEqual({
      kind: "skipped",
      reason: "no-remote",
    });
    expect(mocks.fetchRepo).not.toHaveBeenCalled();
  });
});

describe("pullCurrentBranch", () => {
  it("pulls the branch from the primary remote", async () => {
    mocks.remotes.mockResolvedValue([remote("origin")]);
    mocks.pullRepo.mockResolvedValue(STATS);

    await expect(pullCurrentBranch("r1", "main")).resolves.toEqual({
      kind: "done",
      remote: "origin",
      stats: STATS,
    });
    expect(mocks.pullRepo).toHaveBeenCalledWith(
      "r1",
      pullOptions("origin", "main"),
    );
  });

  it("skips before resolving remotes when there is no branch", async () => {
    await expect(pullCurrentBranch("r1", null)).resolves.toEqual({
      kind: "skipped",
      reason: "no-branch",
    });
    expect(mocks.remotes).not.toHaveBeenCalled();
    expect(mocks.pullRepo).not.toHaveBeenCalled();
  });

  it("skips when no remote is configured", async () => {
    mocks.remotes.mockResolvedValue([]);

    await expect(pullCurrentBranch("r1", "main")).resolves.toEqual({
      kind: "skipped",
      reason: "no-remote",
    });
    expect(mocks.pullRepo).not.toHaveBeenCalled();
  });
});

describe("pushCurrentBranch", () => {
  it("pushes the branch to the primary remote", async () => {
    mocks.remotes.mockResolvedValue([remote("origin")]);
    mocks.pushRepo.mockResolvedValue(STATS);

    await expect(pushCurrentBranch("r1", "main")).resolves.toEqual({
      kind: "done",
      remote: "origin",
      stats: STATS,
    });
    expect(mocks.pushRepo).toHaveBeenCalledWith(
      "r1",
      pushOptions("origin", "main"),
    );
  });

  it("skips before resolving remotes when there is no branch", async () => {
    await expect(pushCurrentBranch("r1", "")).resolves.toEqual({
      kind: "skipped",
      reason: "no-branch",
    });
    expect(mocks.remotes).not.toHaveBeenCalled();
    expect(mocks.pushRepo).not.toHaveBeenCalled();
  });

  it("skips when no remote is configured", async () => {
    mocks.remotes.mockResolvedValue([]);

    await expect(pushCurrentBranch("r1", "main")).resolves.toEqual({
      kind: "skipped",
      reason: "no-remote",
    });
    expect(mocks.pushRepo).not.toHaveBeenCalled();
  });
});

describe("messages", () => {
  it("summarizes transfer stats for toasts", () => {
    expect(netStatsSummary(STATS)).toBe("up to date");
    expect(
      netStatsSummary({
        ...STATS,
        updated_refs: [["refs/heads/main", "abc"]],
      }),
    ).toBe("1 ref updated");
    expect(
      netStatsSummary({
        ...STATS,
        updated_refs: [
          ["refs/heads/main", "abc"],
          ["refs/heads/dev", "def"],
        ],
      }),
    ).toBe("2 refs updated");
  });

  it("names the remote and branch in success messages", () => {
    const done = { kind: "done" as const, remote: "origin", stats: STATS };
    expect(syncSuccessMessage("fetch", done)).toBe(
      "Fetched origin (up to date)",
    );
    expect(syncSuccessMessage("pull", done, "main")).toBe(
      "Pulled origin/main (up to date)",
    );
    expect(syncSuccessMessage("push", done, "main")).toBe(
      "Pushed main to origin (up to date)",
    );
  });

  it("explains preconditions in skip messages", () => {
    expect(syncSkipMessage("no-remote", "fetch")).toBe(
      "No remote configured for this repository",
    );
    expect(syncSkipMessage("no-branch", "push")).toBe(
      "No current branch to push",
    );
  });
});
