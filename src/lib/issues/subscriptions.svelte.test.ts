import { afterEach, describe, expect, it, vi } from "vitest";
import { appSettings } from "$lib/state/appSettings.svelte";
import { resetRepoEvictors } from "$lib/state/repoEvictors";
import { PollScheduler, type Clock } from "$lib/stores/pollScheduler.svelte";
import {
  IssueActivityStore,
  defaultIssueSyncState,
  readIssueSyncState,
  writeIssueSyncState,
  type IssueSyncState,
} from "./activity.svelte";
import {
  ACTIVE_ISSUE_POLL_MS,
  INACTIVE_ISSUE_POLL_MS,
  ExecutorRateLimitGate,
  IssueSubscriptionCoordinator,
  issueLaneKey,
  rateLimitDelayMs,
  type SubscriptionEventTarget,
  type SubscriptionTab,
} from "./subscriptions.svelte";
import { batch, comment, identity, issue, transport, MemoryStorage } from "./testFixtures";

const ROOT = "c:/repos/demo";
const ROOT_2 = "c:/repos/other";
const CURSOR = "2026-01-01T12:00:00.000Z";
const NOW = Date.parse("2026-02-01T00:00:00.000Z");

class ManualClock implements Clock {
  now = NOW;
  private nextId = 1;
  private timers = new Map<number, { at: number; handler: () => void }>();

  setTimeout(handler: () => void, ms: number): number {
    const id = this.nextId++;
    this.timers.set(id, { at: this.now + Math.max(0, ms), handler });
    return id;
  }

  clearTimeout(handle: unknown): void {
    this.timers.delete(handle as number);
  }

  advance(ms: number): void {
    const target = this.now + ms;
    let iterations = 0;
    while (iterations++ < 1_000) {
      const due = [...this.timers.entries()]
        .filter(([, timer]) => timer.at <= target)
        .sort((a, b) => a[1].at - b[1].at)[0];
      if (due === undefined) break;
      this.timers.delete(due[0]);
      this.now = due[1].at;
      due[1].handler();
    }
    this.now = target;
    if (iterations >= 1_000) throw new Error("timer loop");
  }
}

class Events implements SubscriptionEventTarget {
  private listeners = new Map<string, Set<() => void>>();
  addEventListener(type: string, listener: () => void): void {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }
  removeEventListener(type: string, listener: () => void): void {
    this.listeners.get(type)?.delete(listener);
  }
  emit(type: string): void {
    for (const listener of [...(this.listeners.get(type) ?? [])]) listener();
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}

function enabled(root = ROOT, overrides: Partial<IssueSyncState> = {}): IssueSyncState {
  return {
    ...defaultIssueSyncState(),
    newIssues: true,
    newComments: true,
    phase: "ready",
    identity: identity(),
    issueCursor: CURSOR,
    commentCursor: CURSOR,
    ...overrides,
  };
}

const coordinators: IssueSubscriptionCoordinator[] = [];
afterEach(() => {
  for (const coordinator of coordinators.splice(0)) coordinator.stop();
  resetRepoEvictors();
  appSettings.update({
    githubEnabled: true,
    githubIssues: true,
  });
});

function setup(options: {
  storage?: MemoryStorage;
  tabs?: SubscriptionTab[];
  activeId?: string | null;
  transport?: ReturnType<typeof transport>;
  hidden?: boolean;
} = {}) {
  const storage = options.storage ?? new MemoryStorage();
  const clock = new ManualClock();
  const visibility = { hidden: options.hidden ?? false, isHidden() { return this.hidden; } };
  const events = new Events();
  let tabs = options.tabs ?? [{ id: "repo-1", root: ROOT }];
  let activeId = options.activeId === undefined ? tabs[0]?.id ?? null : options.activeId;
  const showToast = vi.fn();
  const activity = new IssueActivityStore({ storage, now: () => clock.now, showToast });
  const scheduler = new PollScheduler({ clock, visibility });
  const tabsSource = {
    list: () => tabs,
    activeId: () => activeId,
    rootFor: (repoId: string) => tabs.find((tab) => tab.id === repoId)?.root,
  };
  const coordinator = new IssueSubscriptionCoordinator({
    storage,
    clock,
    now: () => clock.now,
    visibility,
    eventTarget: events,
    scheduler,
    activity,
    transport: options.transport ?? transport(),
    tabs: tabsSource,
  });
  coordinators.push(coordinator);
  return {
    coordinator, storage, clock, visibility, events, showToast, scheduler,
    setTabs(next: SubscriptionTab[]) { tabs = next; },
    setActive(next: string | null) { activeId = next; },
  };
}

describe("Issues subscription coordinator", () => {
  it("defaults off, persists independent toggles, and establishes a silent first baseline", async () => {
    const activitySince = vi.fn()
      .mockResolvedValueOnce(batch({ issues: [issue()], comments: [comment()] }))
      .mockResolvedValueOnce(batch({
        scanned_at: "2026-01-04T00:00:00.000Z",
        next_issue_cursor: "2026-01-04T00:00:00.000Z",
        next_comment_cursor: "2026-01-04T00:00:00.000Z",
        issues: [issue({ id: "I_new", number: 2, created_at: "2026-01-03T00:00:00.000Z", updated_at: "2026-01-03T00:00:00.000Z" })],
      }));
    const context = setup({ transport: transport({ issuesActivitySince: activitySince }) });
    context.coordinator.start();
    await context.coordinator.catchUp();
    expect(activitySince).not.toHaveBeenCalled();
    expect(context.coordinator.lane("repo-1")).toBeUndefined();

    await context.coordinator.setSubscription("repo-1", { newIssues: true });
    expect(readIssueSyncState(context.storage, ROOT)).toMatchObject({ newIssues: true, newComments: false, phase: "baseline-pending" });
    await context.coordinator.catchUp();
    expect(activitySince).toHaveBeenCalledOnce();
    expect(activitySince.mock.calls[0]).toEqual(["repo-1", null, null]);
    expect(context.showToast.mock.calls.filter((call) => call[1]?.kind === "info")).toHaveLength(0);
    expect(readIssueSyncState(context.storage, ROOT).outbox).toEqual([]);

    await context.coordinator.setSubscription("repo-1", { newComments: true });
    expect(readIssueSyncState(context.storage, ROOT)).toMatchObject({ newIssues: true, newComments: true, phase: "ready" });
    await context.coordinator.refresh("repo-1");
    expect(context.showToast.mock.calls.filter((call) => call[1]?.kind === "info")).toHaveLength(1);
    expect(readIssueSyncState(context.storage, ROOT).outbox.map((event) => event.id)).toEqual(["issue:I_new"]);
    const raw = context.storage.getItem("mygitui.issues.sync." + ROOT) ?? "";
    expect(raw).not.toContain("Issue body must never enter subscription persistence");
  });

  it("continues an incomplete silent baseline from its opaque cursors", async () => {
    const issueCursor = "/repos/octo/demo/issues?page=2";
    const commentCursor = "/repos/octo/demo/issues/comments?page=3";
    const activitySince = vi.fn()
      .mockResolvedValueOnce(batch({
        next_issue_cursor: issueCursor,
        next_comment_cursor: commentCursor,
        issues_complete: false,
        comments_complete: false,
      }))
      .mockResolvedValueOnce(batch());
    const context = setup({ transport: transport({ issuesActivitySince: activitySince }) });
    context.coordinator.start();
    await context.coordinator.setSubscription("repo-1", { newIssues: true });
    await context.coordinator.catchUp();
    expect(activitySince.mock.calls[0]).toEqual(["repo-1", null, null]);
    expect(readIssueSyncState(context.storage, ROOT)).toMatchObject({
      phase: "baseline-pending",
      issueCursor,
      commentCursor,
    });

    context.clock.advance(0);
    await vi.waitFor(() => expect(activitySince).toHaveBeenCalledTimes(2));
    expect(activitySince.mock.calls[1]).toEqual(["repo-1", issueCursor, commentCursor]);
    expect(readIssueSyncState(context.storage, ROOT).phase).toBe("ready");
  });

  it("resumes durable cursors after restart without replaying old activity", async () => {
    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, enabled(), NOW);
    const activitySince = vi.fn().mockResolvedValue(batch({
      scanned_at: "2026-01-03T00:00:00.000Z",
      next_issue_cursor: "2026-01-03T00:00:00.000Z",
      next_comment_cursor: "2026-01-03T00:00:00.000Z",
      issues: [issue({ id: "I_resumed", number: 3, created_at: "2026-01-02T00:00:00.000Z", updated_at: "2026-01-02T00:00:00.000Z" })],
    }));
    const context = setup({ storage, transport: transport({ issuesActivitySince: activitySince }) });
    context.coordinator.start();
    await vi.waitFor(() => expect(readIssueSyncState(storage, ROOT).outbox).toHaveLength(1));
    expect(activitySince.mock.calls[0]).toEqual(["repo-1", "2026-01-01T11:59:58.000Z", "2026-01-01T11:59:58.000Z"]);
    expect(readIssueSyncState(storage, ROOT).outbox[0]?.id).toBe("issue:I_resumed");
  });

  it("guards startup, focus, and manual refresh while hidden", async () => {
    const issuesIdentity = vi.fn(async () => identity());
    const activitySince = vi.fn(async () => batch());
    const context = setup({ hidden: true, transport: transport({ issuesIdentity, issuesActivitySince: activitySince }) });
    context.coordinator.start();
    await context.coordinator.setSubscription("repo-1", { newIssues: true });
    await context.coordinator.catchUp();
    await context.coordinator.refresh("repo-1");
    context.events.emit("focus");
    expect(issuesIdentity).not.toHaveBeenCalled();
    expect(activitySince).not.toHaveBeenCalled();

    context.visibility.hidden = false;
    context.events.emit("visibilitychange");
    await vi.waitFor(() => expect(activitySince).toHaveBeenCalledOnce());
  });

  it("uses active/inactive cadences, promotes the active repo, and evicts only runtime lanes", async () => {
    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, enabled(), NOW);
    writeIssueSyncState(storage, ROOT_2, enabled(ROOT_2), NOW);
    const context = setup({ storage, tabs: [{ id: "repo-1", root: ROOT }, { id: "repo-2", root: ROOT_2 }] });
    context.coordinator.start();
    expect(context.scheduler.intervalOf(issueLaneKey("repo-1"))).toBe(ACTIVE_ISSUE_POLL_MS);
    expect(context.scheduler.intervalOf(issueLaneKey("repo-2"))).toBe(INACTIVE_ISSUE_POLL_MS);
    context.setActive("repo-2");
    context.coordinator.syncLanes();
    expect(context.scheduler.intervalOf(issueLaneKey("repo-1"))).toBe(INACTIVE_ISSUE_POLL_MS);
    expect(context.scheduler.intervalOf(issueLaneKey("repo-2"))).toBe(ACTIVE_ISSUE_POLL_MS);
    context.coordinator.evict("repo-2");
    expect(context.coordinator.lane("repo-2")).toBeUndefined();
    expect(readIssueSyncState(storage, ROOT_2).newIssues).toBe(true);
    context.setTabs([{ id: "repo-1", root: ROOT }]);
    context.coordinator.syncLanes();
    expect(context.coordinator.lane("repo-1")).toBeDefined();
  });

  it("drops all lanes and clears queue when GitHub or Issues are disabled", async () => {
    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, enabled(), NOW);
    const context = setup({ storage, tabs: [{ id: "repo-1", root: ROOT }] });
    context.coordinator.start();
    expect(context.coordinator.lane("repo-1")).toBeDefined();

    // Disable Issues feature
    appSettings.update({ githubIssues: false });
    context.coordinator.syncLanes();
    expect(context.coordinator.lane("repo-1")).toBeUndefined();

    // Re-enable Issues, disable master switch
    appSettings.update({ githubIssues: true });
    context.coordinator.syncLanes();
    expect(context.coordinator.lane("repo-1")).toBeDefined();

    appSettings.update({ githubEnabled: false });
    context.coordinator.syncLanes();
    expect(context.coordinator.lane("repo-1")).toBeUndefined();
  });

  it("coalesces catch-up and starts with the active repository", async () => {
    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, enabled(), NOW);
    writeIssueSyncState(storage, ROOT_2, enabled(ROOT_2), NOW);
    const first = deferred<ReturnType<typeof batch>>();
    const calls: string[] = [];
    const activitySince = vi.fn(async (repoId: string) => {
      calls.push(repoId);
      if (repoId === "repo-2") return first.promise;
      return batch();
    });
    const context = setup({
      storage,
      tabs: [{ id: "repo-1", root: ROOT }, { id: "repo-2", root: ROOT_2 }],
      activeId: "repo-2",
      transport: transport({ issuesActivitySince: activitySince }),
    });
    context.coordinator.start();
    const one = context.coordinator.catchUp();
    const two = context.coordinator.catchUp();
    expect(two).toBe(one);
    await vi.waitFor(() => expect(calls[0]).toBe("repo-2"));
    expect(calls).toEqual(["repo-2"]);
    first.resolve(batch());
    await one;
    expect(calls).toEqual(["repo-2", "repo-1"]);
  });

  it("rejects failed polls so the scheduler earns exponential backoff", async () => {
    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, enabled(), NOW);
    const activitySince = vi.fn().mockRejectedValue(new Error("offline"));
    const context = setup({ storage, transport: transport({ issuesActivitySince: activitySince }) });
    context.coordinator.start();
    await context.coordinator.catchUp();
    expect(context.coordinator.lane("repo-1")).toMatchObject({ erroring: true, consecutiveErrors: 1, backoffMs: ACTIVE_ISSUE_POLL_MS * 2 });
    expect(readIssueSyncState(storage, ROOT).lastError).toBe("request-failed");
    await context.coordinator.refresh("repo-1");
    expect(context.coordinator.lane("repo-1")).toMatchObject({ consecutiveErrors: 2, backoffMs: ACTIVE_ISSUE_POLL_MS * 4 });
  });

  it("continues partial batches immediately without waiting for the cadence", async () => {
    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, enabled(), NOW);
    const activitySince = vi.fn()
      .mockResolvedValueOnce(batch({ issues_complete: false, next_issue_cursor: "2026-01-01T12:30:00.000Z" }))
      .mockResolvedValueOnce(batch({
        scanned_at: "2026-01-03T00:00:00.000Z",
        next_issue_cursor: "2026-01-03T00:00:00.000Z",
        next_comment_cursor: "2026-01-03T00:00:00.000Z",
        issues: [issue({ id: "I_later", number: 4, created_at: "2026-01-02T00:00:00.000Z", updated_at: "2026-01-02T00:00:00.000Z" })],
      }));
    const context = setup({ storage, hidden: true, transport: transport({ issuesActivitySince: activitySince }) });
    context.coordinator.start();
    context.visibility.hidden = false;
    await context.coordinator.refresh("repo-1");
    expect(activitySince).toHaveBeenCalledOnce();
    context.clock.advance(0);
    await vi.waitFor(() => expect(activitySince).toHaveBeenCalledTimes(2));
    expect(readIssueSyncState(storage, ROOT).outbox[0]?.id).toBe("issue:I_later");
  });

  it("resets baseline and outbox when the GitHub identity changes", async () => {
    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, enabled(ROOT, {
      outbox: [{
        id: "issue:I_old", kind: "issue", issueId: "I_old", issueNumber: 8, commentId: null,
        title: "Old", actor: "alice", createdAt: CURSOR, detectedAt: CURSOR,
        url: "https://github.com/octo/demo/issues/8", readAt: null,
      }],
    }), NOW);
    const changed = identity({ user_id: "user-new", user_login: "new-user" });
    const activitySince = vi.fn(async () => batch({ issues: [issue()] }));
    const context = setup({ storage, transport: transport({ issuesIdentity: async () => changed, issuesActivitySince: activitySince }) });
    context.coordinator.start();
    await context.coordinator.catchUp();
    expect(activitySince.mock.calls[0]).toEqual(["repo-1", null, null]);
    expect(readIssueSyncState(storage, ROOT)).toMatchObject({ identity: changed, outbox: [], phase: "ready" });
    expect(context.showToast.mock.calls.filter((call) => call[1]?.kind === "info")).toHaveLength(0);
  });

  it("advances nothing on persistence failure and retries atomically on manual refresh", async () => {
    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, enabled(), NOW);
    const original = storage.getItem("mygitui.issues.sync." + ROOT);
    const activitySince = vi.fn().mockResolvedValue(batch({
      scanned_at: "2026-01-03T00:00:00.000Z",
      next_issue_cursor: "2026-01-03T00:00:00.000Z",
      next_comment_cursor: "2026-01-03T00:00:00.000Z",
      issues: [issue({ id: "I_durable", number: 5, created_at: "2026-01-02T00:00:00.000Z", updated_at: "2026-01-02T00:00:00.000Z" })],
    }));
    const context = setup({ storage, transport: transport({ issuesActivitySince: activitySince }) });
    context.coordinator.start();
    storage.failWrites = true;
    await context.coordinator.refresh("repo-1");
    expect(storage.getItem("mygitui.issues.sync." + ROOT)).toBe(original);
    expect(context.coordinator.state(ROOT).phase).toBe("storage-error");
    expect(context.showToast.mock.calls.filter((call) => call[1]?.kind === "error")).toHaveLength(1);

    storage.failWrites = false;
    await context.coordinator.refresh("repo-1");
    const saved = readIssueSyncState(storage, ROOT);
    expect(saved.issueCursor).toBe("2026-01-03T00:00:00.000Z");
    expect(saved.outbox.map((event) => event.id)).toEqual(["issue:I_durable"]);
    expect(context.showToast.mock.calls.filter((call) => call[1]?.kind === "info")).toHaveLength(1);
  });

  it("does not persist an in-flight response after stop", async () => {
    const storage = new MemoryStorage();
    writeIssueSyncState(storage, ROOT, enabled(), NOW);
    const pending = deferred<ReturnType<typeof batch>>();
    const activitySince = vi.fn(() => pending.promise);
    const context = setup({ storage, transport: transport({ issuesActivitySince: activitySince }) });
    context.coordinator.start();
    const refresh = context.coordinator.refresh("repo-1");
    await vi.waitFor(() => expect(activitySince).toHaveBeenCalledOnce());
    context.coordinator.stop();
    pending.resolve(batch({ issues: [issue({ id: "I_stopped", created_at: "2026-01-02T00:00:00.000Z" })] }));
    await refresh;
    context.clock.advance(ACTIVE_ISSUE_POLL_MS * 2);
    expect(readIssueSyncState(storage, ROOT).outbox).toEqual([]);
    expect(readIssueSyncState(storage, ROOT).issueCursor).toBe(CURSOR);
  });
});

describe("Issues shared executor rate gate", () => {
  it("serializes work on the same executor scope", async () => {
    const gate = new ExecutorRateLimitGate({ now: () => NOW, clock: new ManualClock() });
    let active = 0;
    let maximum = 0;
    const work = async () => {
      active += 1;
      maximum = Math.max(maximum, active);
      await Promise.resolve();
      active -= 1;
    };
    await Promise.all([gate.run("local", work), gate.run("local", work)]);
    expect(maximum).toBe(1);
  });

  it("honors Retry-After and reset deadlines before the next request", async () => {
    const clock = new ManualClock();
    const gate = new ExecutorRateLimitGate({ now: () => clock.now, clock });
    const limited = gate.run("local", async () => { throw { status: 403, retry_after: 2 }; });
    await expect(limited).rejects.toMatchObject({ status: 403 });
    const work = vi.fn(async () => "ok");
    const next = gate.run("local", work);
    await Promise.resolve();
    expect(work).not.toHaveBeenCalled();
    clock.advance(2_000);
    await expect(next).resolves.toBe("ok");
    expect(rateLimitDelayMs({ failure: { kind: "RateLimited", reset_at: NOW / 1_000 + 30 } }, NOW)).toBe(30_000);
    expect(rateLimitDelayMs({ kind: "RateLimited", http_status: 403, rate_limit_reset_at: NOW / 1_000 + 45 }, NOW)).toBe(45_000);
    expect(rateLimitDelayMs(new Error("other"))).toBe(0);
  });
});
