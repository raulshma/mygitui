/**
 * Unit tests for the forge store (`$lib/stores/forge.svelte`). The IPC
 * client is fully mocked (`vi.mock`) — no Tauri runtime is touched.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  forgeContext,
  forgeStatus,
  prChecks,
  prCheckout,
  prClose,
  prComment,
  prList,
  prMerge,
  prReady,
  prReopen,
  prReview,
  workflowDispatch,
  workflowList,
  workflowRunCancel,
  workflowRunDetail,
  workflowRunLogs,
  workflowRunRerun,
  workflowRuns,
} from "$lib/ipc/client";
import type {
  ForgeContext,
  ForgeStatus,
  PrInfo,
  WorkflowInfo,
  WorkflowRun,
  WorkflowRunDetail,
} from "$lib/ipc/client";
import {
  checksKey,
  ForgeStore,
  forgeStore,
  runKey,
  runLogsKey,
} from "$lib/stores/forge.svelte";

vi.mock("$lib/ipc/client", () => ({
  forgeStatus: vi.fn(),
  forgeContext: vi.fn(),
  prList: vi.fn(),
  prChecks: vi.fn(),
  prMerge: vi.fn(),
  prClose: vi.fn(),
  prReopen: vi.fn(),
  prReady: vi.fn(),
  prCheckout: vi.fn(),
  prComment: vi.fn(),
  prReview: vi.fn(),
  workflowRuns: vi.fn(),
  workflowList: vi.fn(),
  workflowRunDetail: vi.fn(),
  workflowRunLogs: vi.fn(),
  workflowRunCancel: vi.fn(),
  workflowRunRerun: vi.fn(),
  workflowDispatch: vi.fn(),
}));

const mockStatus = vi.mocked(forgeStatus);
const mockContext = vi.mocked(forgeContext);
const mockPrList = vi.mocked(prList);
const mockPrChecks = vi.mocked(prChecks);
const mockPrMerge = vi.mocked(prMerge);
const mockPrClose = vi.mocked(prClose);
const mockPrReopen = vi.mocked(prReopen);
const mockPrReady = vi.mocked(prReady);
const mockPrCheckout = vi.mocked(prCheckout);
const mockPrComment = vi.mocked(prComment);
const mockPrReview = vi.mocked(prReview);
const mockRuns = vi.mocked(workflowRuns);
const mockWorkflows = vi.mocked(workflowList);
const mockRunDetail = vi.mocked(workflowRunDetail);
const mockRunLogs = vi.mocked(workflowRunLogs);
const mockRunCancel = vi.mocked(workflowRunCancel);
const mockRunRerun = vi.mocked(workflowRunRerun);
const mockDispatch = vi.mocked(workflowDispatch);

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
    updated_at: null,
    merged_at: null,
    closed_at: null,
    author: "",
    body: "",
    additions: 0,
    deletions: 0,
    changed_files: 0,
    review_decision: "",
    mergeable: "MERGEABLE",
    labels: [],
  };
}

function run(id: number, patch: Partial<WorkflowRun> = {}): WorkflowRun {
  return {
    database_id: id,
    number: id,
    name: "CI",
    display_title: `run ${id}`,
    event: "push",
    head_branch: "main",
    head_sha: "abc",
    status: "completed",
    conclusion: "success",
    url: `https://github.com/acme/widget/actions/runs/${id}`,
    created_at: null,
    updated_at: null,
    started_at: null,
    attempt: 1,
    workflow_database_id: 9,
    ...patch,
  };
}

const WORKFLOWS: WorkflowInfo[] = [
  { id: 9, name: "CI", path: ".github/workflows/ci.yml", state: "active" },
];

const DETAIL: WorkflowRunDetail = {
  run: run(101, { conclusion: "failure" }),
  jobs: [
    {
      database_id: 900,
      name: "build",
      status: "completed",
      conclusion: "failure",
      started_at: null,
      completed_at: null,
      url: "https://github.com/acme/widget/actions/runs/101/job/900",
      steps: [],
    },
  ],
};

beforeEach(() => {
  vi.resetAllMocks();
  mockStatus.mockResolvedValue(STATUS);
  mockContext.mockResolvedValue(CONTEXT);
  mockPrList.mockResolvedValue([pr(1), pr(2)]);
  mockPrChecks.mockResolvedValue([
    { name: "build", state: "pass" },
    { name: "lint", state: "fail" },
  ]);
  mockPrMerge.mockResolvedValue("merged");
  mockPrClose.mockResolvedValue("closed");
  mockPrReopen.mockResolvedValue("reopened");
  mockPrReady.mockResolvedValue("ready");
  mockPrCheckout.mockResolvedValue("checked out");
  mockPrComment.mockResolvedValue("commented");
  mockPrReview.mockResolvedValue("reviewed");
  mockRuns.mockResolvedValue([run(101), run(102, { status: "in_progress", conclusion: "" })]);
  mockWorkflows.mockResolvedValue(WORKFLOWS);
  mockRunDetail.mockResolvedValue(DETAIL);
  mockRunLogs.mockResolvedValue("line 1\nline 2\n");
  mockRunCancel.mockResolvedValue("cancelled");
  mockRunRerun.mockResolvedValue("rerun");
  mockDispatch.mockResolvedValue("dispatched");
});

describe("ForgeStore", () => {
  it("ensure loads status (once), context and prs in parallel", async () => {
    const store = new ForgeStore();
    await store.ensure("r1");
    expect(mockStatus).toHaveBeenCalledTimes(1);
    expect(mockContext).toHaveBeenCalledWith("r1");
    expect(mockPrList).toHaveBeenCalledWith("r1", "open");
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
    expect(mockPrList).toHaveBeenCalledWith("r1", "open");
  });

  it("stores the PR state filter and reuses it on refresh", async () => {
    const store = new ForgeStore();
    await store.refreshPrs("r1", "all");
    expect(mockPrList).toHaveBeenLastCalledWith("r1", "all");
    expect(store.filterFor("r1")).toBe("all");
    await store.refreshPrs("r1");
    expect(mockPrList).toHaveBeenLastCalledWith("r1", "all");
    expect(store.filterFor("r2")).toBe("open");
  });

  it("merge delegates, clears busy and refreshes list/checks/context", async () => {
    const store = new ForgeStore();
    const result = await store.mergePr("r1", 7, "squash", true, false);
    expect(result).toBe("merged");
    expect(mockPrMerge).toHaveBeenCalledWith("r1", 7, "squash", true, false);
    expect(store.isBusy("r1", 7)).toBe(false);
    expect(mockPrList).toHaveBeenCalledWith("r1", "open");
    expect(mockPrChecks).toHaveBeenCalledWith("r1", 7);
    expect(mockContext).toHaveBeenCalledWith("r1");
  });

  it("comment/review/close/reopen/ready/checkout delegate to their commands", async () => {
    const store = new ForgeStore();
    await store.commentPr("r1", 1, "hello");
    expect(mockPrComment).toHaveBeenCalledWith("r1", 1, "hello");
    await store.reviewPr("r1", 1, "request-changes", "please fix");
    expect(mockPrReview).toHaveBeenCalledWith("r1", 1, "request-changes", "please fix");
    await store.closePr("r1", 1, null);
    expect(mockPrClose).toHaveBeenCalledWith("r1", 1, null);
    await store.reopenPr("r1", 1, "reopening");
    expect(mockPrReopen).toHaveBeenCalledWith("r1", 1, "reopening");
    await store.setDraft("r1", 1, true);
    expect(mockPrReady).toHaveBeenCalledWith("r1", 1, false);
    await store.setDraft("r1", 1, false);
    expect(mockPrReady).toHaveBeenLastCalledWith("r1", 1, true);
    await store.checkoutPr("r1", 1);
    expect(mockPrCheckout).toHaveBeenCalledWith("r1", 1);
  });

  it("marks one PR busy while its action is in flight", async () => {
    let resolveMerge: (value: string) => void = () => {};
    mockPrMerge.mockImplementationOnce(
      () =>
        new Promise<string>((resolve) => {
          resolveMerge = resolve;
        }),
    );
    const store = new ForgeStore();
    const pending = store.mergePr("r1", 3, "merge", false, false);
    expect(store.isBusy("r1", 3)).toBe(true);
    resolveMerge("merged");
    await pending;
    expect(store.isBusy("r1", 3)).toBe(false);
  });

  it("ensureActions loads runs and workflow files", async () => {
    const store = new ForgeStore();
    await store.ensureActions("r1");
    expect(mockRuns).toHaveBeenCalledWith("r1", {});
    expect(mockWorkflows).toHaveBeenCalledWith("r1");
    expect(store.runsFor("r1")).toHaveLength(2);
    expect(store.workflowsFor("r1")).toEqual(WORKFLOWS);
    expect(store.runsLoadingFor("r1")).toBe(false);
    expect(store.workflowsLoadingFor("r1")).toBe(false);
  });

  it("run filters are stored and reused; failures surface as errors", async () => {
    const store = new ForgeStore();
    await store.refreshRuns("r1", { branch: "main", status: "failure" });
    expect(mockRuns).toHaveBeenCalledWith("r1", {
      branch: "main",
      status: "failure",
    });
    expect(store.runFiltersFor("r1")).toEqual({
      branch: "main",
      status: "failure",
    });
    await store.refreshRuns("r1");
    expect(mockRuns).toHaveBeenLastCalledWith("r1", {
      branch: "main",
      status: "failure",
    });

    mockRuns.mockRejectedValueOnce(new Error("gh exploded"));
    const result = await store.refreshRuns("r1");
    expect(result).toEqual([]);
    expect(store.runsErrorFor("r1")).toContain("gh exploded");

    await store.refreshRuns("r1");
    expect(store.runsErrorFor("r1")).toBeNull();
  });

  it("run details and logs are lazy and keyed per run/job", async () => {
    const store = new ForgeStore();
    expect(store.detailFor("r1", 101)).toBeNull();
    expect(await store.loadRunDetail("r1", 101)).toEqual(DETAIL);
    expect(mockRunDetail).toHaveBeenCalledWith("r1", 101);
    expect(store.detailFor("r1", 101)).toEqual(DETAIL);
    expect(store.detailLoadingFor("r1", 101)).toBe(false);
    expect(runKey("r1", 101)).toBe("r1:101");

    const logs = await store.loadRunLogs("r1", 101, 900, true);
    expect(logs).toContain("line 1");
    expect(mockRunLogs).toHaveBeenCalledWith("r1", 101, 900, true);
    expect(store.logsFor("r1", 101, 900, true)).toBe(logs);
    expect(store.logsFor("r1", 101, null, false)).toBeNull();
    expect(runLogsKey("r1", 101, 900, true)).toBe("r1:101:900:failed");
    expect(store.logsLoadingFor("r1", 101, 900, true)).toBe(false);
  });

  it("merges a fresher detail run back into the cached list", async () => {
    const store = new ForgeStore();
    await store.refreshRuns("r1");
    expect(store.runsFor("r1")[0]!.conclusion).toBe("success");
    mockRunDetail.mockResolvedValueOnce({
      ...DETAIL,
      run: run(101, { conclusion: "failure" }),
    });
    await store.loadRunDetail("r1", 101);
    expect(store.runsFor("r1")[0]!.conclusion).toBe("failure");
    expect(store.runsFor("r1")[1]!.database_id).toBe(102);

    // Without a cached list there is nothing to merge and no phantom row.
    const other = new ForgeStore();
    await other.loadRunDetail("r1", 101);
    expect(other.runsFor("r1")).toEqual([]);
  });

  it("silent run refreshes keep the loading flag untouched", async () => {
    let resolveRuns: (value: WorkflowRun[]) => void = () => {};
    mockRuns.mockImplementationOnce(
      () =>
        new Promise<WorkflowRun[]>((resolve) => {
          resolveRuns = resolve;
        }),
    );
    const store = new ForgeStore();
    const pending = store.refreshRuns("r1", {}, true);
    expect(store.runsLoadingFor("r1")).toBe(false);
    resolveRuns([run(101)]);
    await pending;
    expect(store.runsFor("r1")).toHaveLength(1);
    expect(store.runsLoadingFor("r1")).toBe(false);
  });

  it("cancel and rerun refresh the list/detail and toggle busy", async () => {
    let resolveCancel: (value: string) => void = () => {};
    mockRunCancel.mockImplementationOnce(
      () =>
        new Promise<string>((resolve) => {
          resolveCancel = resolve;
        }),
    );
    const store = new ForgeStore();
    await store.loadRunLogs("r1", 101);
    const pending = store.cancelRun("r1", 101);
    expect(store.isRunBusy("r1", 101)).toBe(true);
    resolveCancel("cancelled");
    await pending;
    expect(mockRunCancel).toHaveBeenCalledWith("r1", 101);
    expect(mockRuns).toHaveBeenCalledWith("r1", {});
    expect(mockRunDetail).toHaveBeenCalledWith("r1", 101);
    expect(store.isRunBusy("r1", 101)).toBe(false);
    expect(store.logsFor("r1", 101, null, false)).toBeNull();

    await store.rerunRun("r1", 101, true);
    expect(mockRunRerun).toHaveBeenCalledWith("r1", 101, true);
  });

  it("dispatchWorkflow delegates and refreshes runs", async () => {
    const store = new ForgeStore();
    const out = await store.dispatchWorkflow("r1", "ci.yml", "main", [
      { key: "dry_run", value: "true" },
    ]);
    expect(out).toBe("dispatched");
    expect(mockDispatch).toHaveBeenCalledWith("r1", "ci.yml", "main", [
      { key: "dry_run", value: "true" },
    ]);
    expect(mockRuns).toHaveBeenCalledWith("r1", {});
    expect(store.isDispatchBusy("r1")).toBe(false);
  });
});

describe("singleton", () => {
  it("exports a shared ForgeStore instance", () => {
    expect(forgeStore).toBeInstanceOf(ForgeStore);
  });
});
