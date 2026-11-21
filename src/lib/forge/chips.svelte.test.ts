/**
 * Forge chips store tests (E1 §9.2–9.3): scheduler-lane wiring of the
 * snapshot polling (active 60 s / inactive 5 m / remote tabs none / cadence
 * off), snapshot cache integration (hash-skip, failure kinds, strict
 * rejections for backoff), the tab attention badges and the failure toasts —
 * plus the run-view log lane's migration contract (fake clock: 3.5 s cadence,
 * signature-gated log refetch, untracked scope switch, pause).
 *
 * The IPC client is mocked (vi.mock over the real module, StatusBar idiom);
 * timers are vitest fakes for the shared scheduler and a hand-rolled fake
 * clock for the PollScheduler-injection lane test.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { tick } from "svelte";
import {
  forgeContext,
  forgeSnapshot,
  forgeStatus,
  remotes,
  repoStatus,
  openRepo,
  closeRepo,
  remoteRepoOpen,
  workflowRunDetail,
  workflowRunLogs,
  branches,
  fetchRepo,
  prList,
  prChecks,
  workflowRuns,
  workflowList,
} from "$lib/ipc/client";
import type {
  ForgeSnapshot,
  NetStats,
  RemoteInfo,
  WorkflowRunDetail,
} from "$lib/ipc/types";
import { getToasts } from "$lib/toast";
import { dismissToast } from "$lib/toast";
import { appSettings } from "$lib/state/appSettings.svelte";
import { resetRepoEvictors } from "$lib/state/repoEvictors";
import { PollScheduler } from "$lib/stores/pollScheduler.svelte";
import { pollScheduler } from "$lib/stores/pollScheduler.svelte";
import { forgeStore } from "$lib/stores/forge.svelte";
import { tabStore, type Tab } from "$lib/stores/tabs.svelte";
import { forgeChips, snapshotLaneKey } from "./chips.svelte";
import { runDetailSignature } from "$lib/components/forge/forgeModel";

vi.mock("$lib/ipc/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/ipc/client")>()),
  forgeStatus: vi.fn(),
  forgeContext: vi.fn(),
  forgeSnapshot: vi.fn(),
  remotes: vi.fn(),
  repoStatus: vi.fn(),
  openRepo: vi.fn(),
  closeRepo: vi.fn(),
  remoteRepoOpen: vi.fn(),
  workflowRunDetail: vi.fn(),
  workflowRunLogs: vi.fn(),
  branches: vi.fn(),
  fetchRepo: vi.fn(),
  prList: vi.fn(),
  prChecks: vi.fn(),
  workflowRuns: vi.fn(),
  workflowList: vi.fn(),
}));

const mockForgeStatus = vi.mocked(forgeStatus);
const mockForgeSnapshot = vi.mocked(forgeSnapshot);
const mockForgeContext = vi.mocked(forgeContext);
const mockRunDetail = vi.mocked(workflowRunDetail);
const mockRunLogs = vi.mocked(workflowRunLogs);

function tab(patch: Partial<Tab> & { id: string }): Tab {
  return {
    name: patch.id,
    root: `/repos/${patch.id}`,
    status: null,
    statusError: null,
    generation: 0,
    deviceId: null,
    openError: null,
    ...patch,
  };
}

/** A minimal ready snapshot; `hash` varies a part so callers control change. */
function snapshot(hash: number, patch: Partial<ForgeSnapshot> = {}): ForgeSnapshot {
  return {
    branch: "feat",
    head_sha: "abc123",
    pushed: true,
    pr: null,
    head_checks: [],
    pr_head_sha: null,
    pr_head_checks: null,
    run: null,
    hashes: { pr: hash, head_checks: 0, pr_head_checks: 0, run: 0 },
    error: null,
    ...patch,
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  mockForgeStatus.mockResolvedValue({ available: true, version: "2.63.3", authed: true });
  mockForgeContext.mockRejectedValue(new Error("not mocked here") as never);
  appSettings.update({
    forgeSnapshotSeconds: 60,
    forgeFailureToasts: true,
    forgeActivityToasts: false,
  });
  tabStore.tabs = [];
  tabStore.activeId = null;
  pollScheduler.stopAll();
  // The chips singleton keeps its own lane bookkeeping — reset it so every
  // test starts with a clean scheduler + store pair.
  forgeChips.clear("r1");
  forgeChips.clear("r2");
  forgeStore.dropSnapshot("r1");
  forgeStore.dropSnapshot("r2");
  forgeChips.start();
});

afterEach(() => {
  pollScheduler.stopAll();
  forgeChips.clear("r1");
  forgeChips.clear("r2");
  tabStore.tabs = [];
  tabStore.activeId = null;
  forgeStore.dropSnapshot("r1");
  forgeStore.dropSnapshot("r2");
  resetRepoEvictors();
  vi.useRealTimers();
});

/** Deep microtask flush (the scheduler's async lane work spans many turns). */
async function flushAsync(): Promise<void> {
  for (let i = 0; i < 30; i += 1) await Promise.resolve();
}

describe("snapshot lanes (syncLanes)", () => {
  it("registers the active repo at the configured cadence and inactive repos at 5m", () => {
    tabStore.tabs = [tab({ id: "r1" }), tab({ id: "r2" })];
    tabStore.activeId = "r1";
    forgeChips.syncLanes();

    expect(pollScheduler.intervalOf(snapshotLaneKey("r1"))).toBe(60_000);
    expect(pollScheduler.intervalOf(snapshotLaneKey("r2"))).toBe(5 * 60_000);
  });

  it("re-registers only on cadence changes and drops closed repos", () => {
    tabStore.tabs = [tab({ id: "r1" }), tab({ id: "r2" })];
    tabStore.activeId = "r1";
    forgeChips.syncLanes();

    // Same shape again: no churn (the lanes keep their timers — observable
    // through an unchanged next-tick schedule after time passes).
    forgeChips.syncLanes();
    forgeChips.syncLanes();

    appSettings.update({ forgeSnapshotSeconds: 30 });
    forgeChips.syncLanes();
    expect(pollScheduler.intervalOf(snapshotLaneKey("r1"))).toBe(30_000);
    expect(pollScheduler.intervalOf(snapshotLaneKey("r2"))).toBe(5 * 60_000);

    // Tab switch promotes r2 to the active cadence.
    tabStore.activeId = "r2";
    forgeChips.syncLanes();
    expect(pollScheduler.intervalOf(snapshotLaneKey("r2"))).toBe(30_000);
    expect(pollScheduler.intervalOf(snapshotLaneKey("r1"))).toBe(5 * 60_000);

    // Closing r1 removes its lane.
    tabStore.tabs = [tab({ id: "r2" })];
    forgeChips.syncLanes();
    expect(pollScheduler.lanes[snapshotLaneKey("r1")]).toBeUndefined();
  });

  it("0 seconds turns the active lane off (inactive lanes stay)", () => {
    appSettings.update({ forgeSnapshotSeconds: 0 });
    tabStore.tabs = [tab({ id: "r1" }), tab({ id: "r2" })];
    tabStore.activeId = "r1";
    forgeChips.syncLanes();

    expect(pollScheduler.lanes[snapshotLaneKey("r1")]).toBeUndefined();
    expect(pollScheduler.intervalOf(snapshotLaneKey("r2"))).toBe(5 * 60_000);
  });

  it("remote tabs poll the same lanes (gh runs on the device)", () => {
    tabStore.tabs = [tab({ id: "r1", deviceId: "dev-1" })];
    tabStore.activeId = "r1";
    forgeChips.syncLanes();
    expect(pollScheduler.lanes[snapshotLaneKey("r1")]).toBeDefined();
  });

  it("the lane work fetches the snapshot and a failure backs the lane off", async () => {
    mockForgeSnapshot.mockRejectedValueOnce({
      kind: "RateLimited",
      message: "rate limit exceeded",
      url: null,
    } as never);
    tabStore.tabs = [tab({ id: "r1" })];
    tabStore.activeId = "r1";
    forgeChips.syncLanes();

    await vi.advanceTimersByTimeAsync(60_000);
    await tick();
    expect(mockForgeSnapshot).toHaveBeenCalledWith("r1", null);

    const lane = pollScheduler.lanes[snapshotLaneKey("r1")];
    expect(lane?.erroring).toBe(true);
    expect(lane?.backoffMs).toBe(120_000);
  });
});

describe("snapshot cache (forge store)", () => {
  it("caches the snapshot, skips re-render on equal hashes and bumps freshness", async () => {
    const first = snapshot(1);
    mockForgeSnapshot.mockResolvedValueOnce(first);
    await forgeStore.refreshSnapshot("r1", { silent: false });
    // Reads come back through $state proxies — identity holds per target.
    const firstProxy = forgeStore.snapshotFor("r1");
    const fetchedAt = forgeStore.snapshotFetchedAtFor("r1");
    expect(firstProxy).not.toBeNull();
    expect(fetchedAt).not.toBeNull();

    // Equal hashes: the previous object is restored (identity stable).
    vi.advanceTimersByTime(1_000);
    const unchanged = snapshot(1);
    mockForgeSnapshot.mockResolvedValueOnce(unchanged);
    await forgeStore.refreshSnapshot("r1");
    expect(forgeStore.snapshotFor("r1")).toBe(firstProxy);
    expect(forgeStore.snapshotFetchedAtFor("r1")).toBeGreaterThanOrEqual(fetchedAt!);

    // Changed hashes: the new object lands.
    const changed = snapshot(2);
    mockForgeSnapshot.mockResolvedValueOnce(changed);
    await forgeStore.refreshSnapshot("r1");
    expect(forgeStore.snapshotFor("r1")).not.toBe(firstProxy);
    expect(forgeStore.snapshotFor("r1")).toEqual(changed);
  });

  it("records structured failure kinds and stale-on-error keeps prior data", async () => {
    const good = snapshot(1);
    mockForgeSnapshot.mockResolvedValueOnce(good);
    await forgeStore.refreshSnapshot("r1", { silent: false });
    const goodProxy = forgeStore.snapshotFor("r1");

    mockForgeSnapshot.mockRejectedValueOnce({
      kind: "NotRepo",
      message: "`origin` is not a GitHub remote: git@example.com:a/b.git",
      url: null,
    } as never);
    await forgeStore.refreshSnapshot("r1");
    expect(forgeStore.snapshotStateFor("r1")).toBe("error");
    expect(forgeStore.snapshotFailureKindFor("r1")).toBe("NotRepo");
    // Stale-on-error: the failed refresh left the previous snapshot visible.
    expect(forgeStore.snapshotStaleFor("r1")).toBe(goodProxy);
    expect(forgeStore.snapshotFor("r1")).toBe(null);

    // A later success clears the kind.
    mockForgeSnapshot.mockResolvedValueOnce(snapshot(3));
    await forgeStore.refreshSnapshot("r1");
    expect(forgeStore.snapshotFailureKindFor("r1")).toBe(null);
  });

  it("refreshSnapshotStrict rejects on failure (scheduler backoff input)", async () => {
    mockForgeSnapshot.mockRejectedValueOnce({
      kind: "RateLimited",
      message: "rate limit exceeded",
      url: null,
    } as never);
    await expect(forgeStore.refreshSnapshotStrict("r1")).rejects.toThrow();
    mockForgeSnapshot.mockResolvedValueOnce(snapshot(1));
    await expect(forgeStore.refreshSnapshotStrict("r1")).resolves.toBeUndefined();
  });
});

describe("chip derivation + tab attention", () => {
  it("degrades on a non-authed gh and renders the off state for remote tabs", async () => {
    tabStore.tabs = [tab({ id: "r1" })];
    tabStore.activeId = "r1";

    mockForgeStatus.mockResolvedValue({ available: true, version: "2.63.3", authed: false });
    await forgeStore.refreshStatus();
    let chips = forgeChips.chipsFor("r1");
    expect(chips.pr.kind).toBe("degraded");
    expect(chips.worst?.state.kind).toBe("degraded");

    mockForgeStatus.mockResolvedValue({ available: true, version: "2.63.3", authed: true });
    await forgeStore.refreshStatus();
    chips = forgeChips.chipsFor("r1");
    // No lane has fetched yet (idle) and gh is healthy: loading, not degraded.
    expect(chips.pr.kind).toBe("loading");

    // A device tab derives from ITS device's gh status (probed through the
    // repo id), not the local machine's: no probe yet → no local-gh
    // degradation, the chip waits on the snapshot like any repo.
    const remote = tab({ id: "r2", deviceId: "dev-1" });
    tabStore.tabs = [remote];
    tabStore.activeId = "r2";
    chips = forgeChips.chipsFor("r2");
    expect(chips.pr.kind).toBe("loading");

    mockForgeStatus.mockResolvedValue({ available: true, version: "2.63.3", authed: false });
    await forgeStore.refreshStatus("r2");
    chips = forgeChips.chipsFor("r2");
    expect(chips.pr).toMatchObject({ kind: "degraded", degraded: "not-authed" });
  });

  it("attention badges surface failing background repos with staleness", async () => {
    const failing = snapshot(1, {
      head_checks: [
        { name: "ci", state: "fail" },
        { name: "lint", state: "pass" },
      ],
    });
    mockForgeSnapshot.mockResolvedValue(failing);
    await forgeStore.refreshSnapshot("r2", { silent: false });

    // Background repo (not active) with a failing checks chip.
    tabStore.tabs = [tab({ id: "r1" }), tab({ id: "r2" })];
    tabStore.activeId = "r1";
    forgeChips.syncLanes();

    const attention = forgeChips.attentionFor("r2");
    expect(attention?.attention).toBe(true);
    expect(attention?.count).toBe(1);
    expect(attention?.text).toContain("failing");
    expect(attention?.fetchedAtMs).not.toBeNull();
    // The active repo carries its own state in the footer — never attention.
    const activeAttention = forgeChips.attentionFor("r1");
    expect(activeAttention?.attention).toBe(false);
  });
});

describe("failure toasts (§9.2: failures only, one per episode)", () => {
  it("toasts once when a background repo turns failing and resets after", async () => {
    const failing = snapshot(1, {
      run: {
        database_id: 9,
        number: 4,
        name: "CI",
        display_title: "CI",
        event: "push",
        head_branch: "feat",
        head_sha: "abc",
        status: "completed",
        conclusion: "failure",
        url: "",
        created_at: null,
        updated_at: null,
        started_at: "2026-11-21T10:00:00Z",
        attempt: 1,
        workflow_database_id: 1,
      },
    });
    mockForgeSnapshot.mockResolvedValue(failing);
    tabStore.tabs = [tab({ id: "r1" })];
    tabStore.activeId = "r1";
    forgeChips.syncLanes();

    await vi.advanceTimersByTimeAsync(60_000);
    await tick();
    const failures = getToasts().filter(
      (t) => t.kind === "error" && t.message.includes("run #4"),
    );
    expect(failures).toHaveLength(1);

    // Recovery resets the episode; the next failure can toast again. (Toast
    // ids before the re-failure — earlier toasts auto-dismissed during the
    // fake-time advances.)
    mockForgeSnapshot.mockResolvedValue(snapshot(2));
    await vi.advanceTimersByTimeAsync(60_000);
    await tick();
    const idsBefore = new Set(getToasts().map((t) => t.id));
    mockForgeSnapshot.mockResolvedValue(failing);
    await vi.advanceTimersByTimeAsync(60_000);
    await tick();
    const fresh = getToasts().filter(
      (t) => t.kind === "error" && !idsBefore.has(t.id) && t.message.includes("run #4"),
    );
    expect(fresh).toHaveLength(1);
  });

  it("respects the failure-toast opt-out", async () => {
    // Dismiss whatever earlier tests left in the shared toast store.
    for (const item of getToasts()) dismissToast(item.id);
    appSettings.update({ forgeFailureToasts: false });
    const failing = snapshot(1, {
      head_checks: [{ name: "ci", state: "fail" }],
    });
    mockForgeSnapshot.mockResolvedValue(failing);
    tabStore.tabs = [tab({ id: "r1" })];
    tabStore.activeId = "r1";
    forgeChips.syncLanes();
    await vi.advanceTimersByTimeAsync(60_000);
    await tick();
    expect(getToasts().some((t) => t.kind === "error")).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Run-view log lane migration (fake clock): the WorkflowRunView lane contract
// — 3.5 s cadence, signature-gated log refetch, untracked job-scope switch,
// pause stops everything.
// ---------------------------------------------------------------------------

describe("run-view log lane (fake clock)", () => {
  interface FakeTimer {
    id: number;
    at: number;
    handler: () => void;
  }

  function fakeClock() {
    let nowMs = 0;
    let nextId = 1;
    let timers: FakeTimer[] = [];
    const clock = {
      now: () => nowMs,
      setTimeout(handler: () => void, ms: number): unknown {
        const id = nextId++;
        timers.push({ id, at: nowMs + ms, handler });
        return id;
      },
      clearTimeout(handle: unknown): void {
        timers = timers.filter((timer) => timer.id !== handle);
      },
      advance(ms: number): void {
        const deadline = nowMs + ms;
        for (;;) {
          const due = timers
            .filter((timer) => timer.at <= deadline)
            .sort((a, b) => a.at - b.at)[0];
          if (!due) break;
          timers = timers.filter((timer) => timer.id !== due.id);
          nowMs = Math.max(nowMs, due.at);
          due.handler();
        }
        nowMs = deadline;
      },
    };
    return clock;
  }

  const DETAIL_BASE: WorkflowRunDetail = {
    run: {
      database_id: 101,
      number: 7,
      name: "CI",
      display_title: "CI",
      event: "push",
      head_branch: "feat",
      head_sha: "abc",
      status: "in_progress",
      conclusion: "",
      url: "",
      created_at: null,
      updated_at: null,
      started_at: "2026-11-21T10:00:00Z",
      attempt: 1,
      workflow_database_id: 1,
    },
    jobs: [
      {
        database_id: 1,
        name: "build",
        status: "in_progress",
        conclusion: "",
        started_at: "2026-11-21T10:00:00Z",
        completed_at: null,
        url: "",
        steps: [
          { number: 1, name: "checkout", status: "completed", conclusion: "success", started_at: null, completed_at: null },
        ],
      },
    ],
  };

  /**
   * Registers the lane exactly like WorkflowRunView does (the component's
   * $effect: capture the signature, register 3.5 s work that refetches the
   * detail and — only when the signature moved — the log with the CURRENT
   * scope read from a plain holder).
   */
  function registerRunViewLane(
    scheduler: PollScheduler,
    repoId: string,
    runId: number,
    scope: { jobId: number | null; failedOnly: boolean },
  ): () => void {
    let lastLogSig = runDetailSignature(forgeStore.detailFor(repoId, runId));
    return scheduler.register(`forge:runview:${repoId}:${runId}`, {
      intervalMs: 3_500,
      work: async () => {
        const detail = await forgeStore.loadRunDetail(repoId, runId);
        if (detail === null) return;
        const sig = runDetailSignature(detail);
        if (sig === lastLogSig) return;
        lastLogSig = sig;
        await forgeStore.loadRunLogs(repoId, runId, scope.jobId, scope.failedOnly);
      },
    });
  }

  it("polls the detail at 3.5 s and refetches logs only when the signature moved", async () => {
    const clock = fakeClock();
    const scheduler = new PollScheduler({
      clock,
      visibility: { isHidden: () => false },
    });
    const detailA: WorkflowRunDetail = structuredClone(DETAIL_BASE);
    const detailB: WorkflowRunDetail = structuredClone(DETAIL_BASE);
    detailB.jobs[0]!.status = "completed";

    mockRunDetail.mockResolvedValue(detailA);
    mockRunLogs.mockResolvedValue("log");

    const repoId = "r1";
    const scope = { jobId: null as number | null, failedOnly: false };
    // On-open fetch (the component's initial-load effect), then registration.
    await forgeStore.loadRunDetail(repoId, 101);
    const stop = registerRunViewLane(scheduler, repoId, 101, scope);
    expect(mockRunDetail).toHaveBeenCalledTimes(1);

    // Tick 1: the run has not moved → no log refetch (the multi-MB payload
    // stays put).
    clock.advance(3_500);
    await flushAsync();
    expect(mockRunDetail).toHaveBeenCalledTimes(2);
    expect(mockRunLogs).not.toHaveBeenCalled();

    // Tick 2: the detail's progress signature changed → the log refetches.
    mockRunDetail.mockResolvedValue(detailB);
    clock.advance(3_500);
    await flushAsync();
    expect(mockRunLogs).toHaveBeenCalledWith(repoId, 101, null, false);

    // A job switch mid-run updates the scope holder WITHOUT re-registering:
    // the next signature move refetches the NEW scope.
    const detailC: WorkflowRunDetail = structuredClone(detailB);
    detailC.jobs[0]!.status = "in_progress";
    mockRunDetail.mockResolvedValue(detailC);
    scope.jobId = 1;
    clock.advance(3_500);
    await flushAsync();
    expect(mockRunLogs).toHaveBeenLastCalledWith(repoId, 101, 1, false);

    // Pause: the component's toggle unregisters — no more ticks at all.
    stop();
    const detailCalls = mockRunDetail.mock.calls.length;
    clock.advance(30_000);
    await flushAsync();
    expect(mockRunDetail).toHaveBeenCalledTimes(detailCalls);
  });

  it("hidden windows spawn nothing; the next visible tick catches up", async () => {
    const clock = fakeClock();
    let hidden = true;
    const scheduler = new PollScheduler({
      clock,
      visibility: { isHidden: () => hidden },
    });
    const detailA: WorkflowRunDetail = structuredClone(DETAIL_BASE);
    const detailB: WorkflowRunDetail = structuredClone(DETAIL_BASE);
    detailB.jobs[0]!.status = "completed";
    mockRunDetail.mockResolvedValue(detailA);
    mockRunLogs.mockResolvedValue("log");

    const scope = { jobId: null as number | null, failedOnly: false };
    await forgeStore.loadRunDetail("r1", 101);
    const stop = registerRunViewLane(scheduler, "r1", 101, scope);

    clock.advance(60_000);
    await flushAsync();
    const callsWhileHidden = mockRunDetail.mock.calls.length;
    expect(callsWhileHidden).toBe(1); // only the on-open fetch ran

    hidden = false;
    mockRunDetail.mockResolvedValue(detailB);
    clock.advance(3_500);
    await flushAsync();
    expect(mockRunDetail.mock.calls.length).toBeGreaterThan(callsWhileHidden);
    stop();
  });
});
