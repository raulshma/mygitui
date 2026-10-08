/**
 * Unit tests for the forge store (`$lib/stores/forge.svelte`). The IPC
 * client is fully mocked (`vi.mock`) — no Tauri runtime is touched.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { forgeContext, forgeStatus, prChecks, prList } from "$lib/ipc/client";
import type { ForgeContext, ForgeStatus, PrInfo } from "$lib/ipc/client";
import { checksKey, ForgeStore, forgeStore } from "$lib/stores/forge.svelte";

vi.mock("$lib/ipc/client", () => ({
  forgeStatus: vi.fn(),
  forgeContext: vi.fn(),
  prList: vi.fn(),
  prChecks: vi.fn(),
}));

const mockStatus = vi.mocked(forgeStatus);
const mockContext = vi.mocked(forgeContext);
const mockPrList = vi.mocked(prList);
const mockPrChecks = vi.mocked(prChecks);

const STATUS: ForgeStatus = { available: true, version: "2.63.3", authed: true };
const CONTEXT: ForgeContext = {
  owner: "acme",
  repo: "widget",
  branch: "feat",
  remote_url: "git@github.com:acme/widget.git",
};

function pr(number: number, head = "feat"): PrInfo {
  return {
    number,
    title: `PR ${number}`,
    head_ref_name: head,
    base_ref_name: "main",
    state: "OPEN",
    is_draft: false,
    url: `https://github.com/acme/widget/pull/${number}`,
    created_at: null,
  };
}

beforeEach(() => {
  vi.resetAllMocks();
  mockStatus.mockResolvedValue(STATUS);
  mockContext.mockResolvedValue(CONTEXT);
  mockPrList.mockResolvedValue([pr(1), pr(2)]);
  mockPrChecks.mockResolvedValue([
    { name: "build", state: "pass" },
    { name: "lint", state: "fail" },
  ]);
});

describe("ForgeStore", () => {
  it("ensure loads status (once), context and prs in parallel", async () => {
    const store = new ForgeStore();
    await store.ensure("r1");
    expect(mockStatus).toHaveBeenCalledTimes(1);
    expect(mockContext).toHaveBeenCalledWith("r1");
    expect(mockPrList).toHaveBeenCalledWith("r1");
    expect(store.status).toEqual(STATUS);
    expect(store.contextFor("r1")).toEqual(CONTEXT);
    expect(store.prsFor("r1")).toHaveLength(2);
    // Second ensure keeps the known status but refreshes repo data.
    await store.ensure("r1");
    expect(mockStatus).toHaveBeenCalledTimes(1);
    expect(mockPrList).toHaveBeenCalledTimes(2);
  });

  it("status failure degrades to unavailable without throwing", async () => {
    mockStatus.mockRejectedValue(new Error("boom"));
    const store = new ForgeStore();
    await store.refreshStatus();
    expect(store.status).toEqual({ available: false, version: "", authed: false });
    expect(store.statusLoading).toBe(false);
  });

  it("context failure lands in contextErrors and clears on success", async () => {
    mockContext.mockRejectedValue(new Error("origin is not a GitHub remote: x"));
    const store = new ForgeStore();
    await store.refreshContext("r1");
    expect(store.contextFor("r1")).toBeNull();
    expect(store.contextErrorFor("r1")).toContain("not a GitHub");
    mockContext.mockResolvedValue(CONTEXT);
    await store.refreshContext("r1");
    expect(store.contextErrorFor("r1")).toBeNull();
  });

  it("pr list failure resolves [] and clears the loading flag", async () => {
    mockPrList.mockRejectedValue(new Error("gh auth required"));
    const store = new ForgeStore();
    const result = await store.refreshPrs("r1");
    expect(result).toEqual([]);
    expect(store.prsFor("r1")).toEqual([]);
    expect(store.prsLoadingFor("r1")).toBe(false);
  });

  it("checks are lazy and keyed per repo+number", async () => {
    const store = new ForgeStore();
    expect(store.checksFor("r1", 1)).toBeNull();
    const checks = await store.refreshChecks("r1", 1);
    expect(mockPrChecks).toHaveBeenCalledWith("r1", 1);
    expect(checks).toHaveLength(2);
    expect(store.checksFor("r1", 1)).toEqual([
      { name: "build", state: "pass" },
      { name: "lint", state: "fail" },
    ]);
    expect(store.checksFor("r2", 1)).toBeNull();
    expect(checksKey("r1", 1)).toBe("r1:1");
    expect(store.checksLoadingFor("r1", 1)).toBe(false);
  });

  it("stale responses are dropped (latest seq wins)", async () => {
    let resolveFirst: (value: PrInfo[]) => void = () => {};
    mockPrList.mockImplementationOnce(
      () =>
        new Promise<PrInfo[]>((resolve) => {
          resolveFirst = resolve;
        }),
    );
    const store = new ForgeStore();
    const first = store.refreshPrs("r1");
    const second = store.refreshPrs("r1");
    resolveFirst([pr(99)]);
    await Promise.all([first, second]);
    // The slow first response (stale seq) must not overwrite the second's.
    expect(store.prsFor("r1")).toEqual([pr(1), pr(2)]);
    expect(store.prsLoadingFor("r1")).toBe(false);
  });

  it("afterCreate refreshes context and prs", async () => {
    const store = new ForgeStore();
    await store.afterCreate("r1");
    expect(mockContext).toHaveBeenCalledWith("r1");
    expect(mockPrList).toHaveBeenCalledWith("r1");
  });
});

describe("singleton", () => {
  it("exports a shared ForgeStore instance", () => {
    expect(forgeStore).toBeInstanceOf(ForgeStore);
  });
});
