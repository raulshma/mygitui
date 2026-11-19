/**
 * Unit tests for the agentic-run controller: the pending-approval queue
 * with its per-ask 120 s auto-reject timer (fake timers), the app-side
 * deny bypass that never surfaces a `git push` ask, and the budget caps
 * (tool calls + wall clock) that fire the interrupt and settle the run.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  AGENTIC_ASK_TIMEOUT_MS,
  AGENTIC_ENDED_MESSAGE,
  AGENTIC_TIMEOUT_MESSAGE,
  AgenticRunController,
  agenticStepViews,
  offersAlways,
  safeDecision,
} from "./agenticRun.svelte";
import type { AgenticRunDeps, AgenticTimers } from "./agenticRun.svelte";
import { AGENTIC_DENY_MESSAGE } from "./toolPolicy";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

/** Controller with recording reply/interrupt doubles (default timers). */
function makeController(overrides?: {
  budget?: { maxWallClockMs?: number; maxToolCalls?: number };
  now?: () => number;
}) {
  const replies: Array<{ requestID: string; decision: string; message: string | null }> = [];
  const deps: AgenticRunDeps = {
    sessionID: "sess-1",
    ...(overrides?.budget ? { budget: overrides.budget } : {}),
    ...(overrides?.now ? { now: overrides.now } : {}),
    reply: async (requestID, decision, message) => {
      replies.push({ requestID, decision, message: message ?? null });
    },
    interrupt: vi.fn(async () => undefined),
    onBudgetStop: vi.fn(),
  };
  const controller = new AgenticRunController(deps);
  return {
    controller,
    replies,
    interrupt: deps.interrupt,
    onBudgetStop: deps.onBudgetStop as ReturnType<typeof vi.fn>,
  };
}

describe("approvals — the pending queue", () => {
  it("queues a pending approval and resolves it via respond", async () => {
    const { controller, replies } = makeController();
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["npm test"] });
    expect(controller.pending).toEqual([
      expect.objectContaining({ requestID: "r1", action: "shell", resources: ["npm test"] }),
    ]);
    expect(controller.summary.asked).toBe(1);

    const handled = await controller.respond("r1", "once", "sure");
    expect(handled).toBe("handled");
    expect(replies).toEqual([{ requestID: "r1", decision: "once", message: "sure" }]);
    expect(controller.pending).toEqual([]);
    expect(controller.summary.approved).toBe(1);
    expect(controller.summary.rejected).toBe(0);

    // The ask's timer was cleared: time passing fires nothing.
    await vi.advanceTimersByTimeAsync(AGENTIC_ASK_TIMEOUT_MS + 1_000);
    expect(replies).toHaveLength(1);
  });

  it("auto-rejects an ask whose 120 s timer fires", async () => {
    const { controller, replies } = makeController();
    controller.handlePermissionAsk({ id: "r1", action: "edit", resources: ["src/a.ts"] });
    expect(controller.pending).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(AGENTIC_ASK_TIMEOUT_MS);
    expect(replies).toEqual([
      { requestID: "r1", decision: "reject", message: AGENTIC_TIMEOUT_MESSAGE },
    ]);
    expect(controller.pending).toEqual([]);
    expect(controller.summary.expired).toBe(1);
  });

  it("reject decisions are counted separately from approvals", async () => {
    const { controller, replies } = makeController();
    controller.handlePermissionAsk({ id: "r1", action: "edit", resources: ["a"] });
    await controller.respond("r1", "reject");
    expect(replies).toEqual([{ requestID: "r1", decision: "reject", message: null }]);
    expect(controller.summary.rejected).toBe(1);
    expect(controller.summary.approved).toBe(0);
  });

  it("respond for an unknown/expired request is a no-op", async () => {
    const { controller, replies } = makeController();
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["ls"] });
    await vi.advanceTimersByTimeAsync(AGENTIC_ASK_TIMEOUT_MS);

    expect(await controller.respond("r1", "once")).toBe("stale");
    expect(await controller.respond("never-existed", "once")).toBe("stale");
    expect(replies).toEqual([
      { requestID: "r1", decision: "reject", message: AGENTIC_TIMEOUT_MESSAGE },
    ]);
  });

  it("uses the injected clock for askedAt", () => {
    let now = 1_000;
    const { controller } = makeController({ now: () => now });
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["ls"] });
    expect(controller.pending[0]!.askedAt).toBe(1_000);
    now = 5_000;
    controller.handlePermissionAsk({ id: "r2", action: "shell", resources: ["ls"] });
    expect(controller.pending[1]!.askedAt).toBe(5_000);
  });

  it("timers are injectable (the seam tests use for fake timers)", async () => {
    const set = vi.fn((handler: () => void, ms: number) => `handle-${ms}`);
    const clear = vi.fn();
    const timers: AgenticTimers = { set, clear };
    const replies: Array<{ requestID: string; decision: string }> = [];
    const controller = new AgenticRunController({
      sessionID: "sess-1",
      timers,
      reply: async (requestID, decision) => {
        replies.push({ requestID, decision });
      },
      interrupt: async () => undefined,
    });
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["ls"] });
    expect(set).toHaveBeenCalledTimes(1);
    expect(set.mock.calls[0]![1]).toBe(AGENTIC_ASK_TIMEOUT_MS);

    await controller.respond("r1", "always");
    expect(clear).toHaveBeenCalledWith("handle-120000");
    // The cleared timer never fires its expiry reply.
    const expiry = set.mock.calls[0]![0];
    // (already cleared — invoking it anyway must be a no-op on a gone ask)
    expiry();
    await Promise.resolve();
    expect(replies).toEqual([{ requestID: "r1", decision: "always" }]);
  });

  it("a failed reply keeps the ask pending with a re-armed timer for retry", async () => {
    let attempts = 0;
    const replies: Array<{ requestID: string; decision: string; message: string | null }> = [];
    const controller = new AgenticRunController({
      sessionID: "sess-1",
      reply: async (requestID, decision, message) => {
        attempts += 1;
        if (attempts === 1) throw new Error("server restart");
        replies.push({ requestID, decision, message: message ?? null });
      },
      interrupt: async () => undefined,
    });
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["npm test"] });

    const outcome = await controller.respond("r1", "once");
    expect(outcome).toBe("reply-failed");
    // The server is still awaiting a decision: the ask stays queued, the
    // tally is untouched and the auto-reject timer is live again.
    expect(controller.pending).toHaveLength(1);
    expect(controller.summary.approved).toBe(0);
    expect(vi.getTimerCount()).toBe(1);

    // The retry succeeds: tally counts only now, timer cleared.
    const retried = await controller.respond("r1", "once");
    expect(retried).toBe("handled");
    expect(replies).toEqual([{ requestID: "r1", decision: "once", message: null }]);
    expect(controller.summary.approved).toBe(1);
    expect(controller.pending).toHaveLength(0);
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("approvals — the deny bypass", () => {
  it("auto-rejects a git push ask without queueing it", async () => {
    const { controller, replies } = makeController();
    controller.handlePermissionAsk({
      id: "r1",
      action: "shell",
      resources: ["git push origin main"],
    });
    expect(replies).toEqual([
      { requestID: "r1", decision: "reject", message: AGENTIC_DENY_MESSAGE },
    ]);
    expect(controller.pending).toEqual([]);
    expect(controller.summary.autoRejected).toBe(1);
    expect(controller.summary.asked).toBe(1);

    // No ask timer was armed for the bypassed ask.
    await vi.advanceTimersByTimeAsync(AGENTIC_ASK_TIMEOUT_MS + 1_000);
    expect(replies).toHaveLength(1);
  });

  it("still queues non-denied asks alongside a denied one", () => {
    const { controller, replies } = makeController();
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["git push"] });
    controller.handlePermissionAsk({ id: "r2", action: "shell", resources: ["npm install"] });
    expect(replies).toHaveLength(1);
    expect(controller.pending.map((approval) => approval.requestID)).toEqual(["r2"]);
  });

  it("ignores duplicate deliveries of an already-seen ask id", async () => {
    const { controller, replies } = makeController();
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["npm test"] });
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["npm test"] });
    expect(controller.pending).toHaveLength(1);
    expect(controller.summary.asked).toBe(1);
    // One expiry reply, not two (the duplicate must not overwrite the
    // id's ask timer).
    await vi.advanceTimersByTimeAsync(AGENTIC_ASK_TIMEOUT_MS);
    expect(replies).toEqual([
      { requestID: "r1", decision: "reject", message: AGENTIC_TIMEOUT_MESSAGE },
    ]);

    // The deny bypass dedupes too.
    controller.handlePermissionAsk({ id: "push", action: "shell", resources: ["git push"] });
    controller.handlePermissionAsk({ id: "push", action: "shell", resources: ["git push"] });
    expect(replies.filter((reply) => reply.requestID === "push")).toHaveLength(1);
  });
});

describe("budget — tool-call cap", () => {
  it("fires the interrupt when the tool-call cap is reached", async () => {
    const { controller, interrupt, onBudgetStop, replies } = makeController({
      budget: { maxToolCalls: 2 },
    });
    controller.handleToolCalled({ id: "t1", input: { command: "ls" }, executed: true });
    expect(controller.budgetStopped).toBe(false);
    expect(interrupt).not.toHaveBeenCalled();

    controller.handleToolCalled({ id: "t2", input: { command: "git status" }, executed: true });
    expect(controller.budgetStopped).toBe(true);
    expect(controller.budgetReason).toBe("tool-calls");
    expect(interrupt).toHaveBeenCalledTimes(1);
    expect(onBudgetStop).toHaveBeenCalledWith("tool-calls");

    // The cap fires once; further calls and wall-clock time change nothing.
    controller.handleToolCalled({ id: "t3", input: {}, executed: true });
    await vi.advanceTimersByTimeAsync(AGENTIC_ASK_TIMEOUT_MS + 1_000);
    expect(interrupt).toHaveBeenCalledTimes(1);
    expect(replies).toEqual([]);
  });

  it("rejects asks that arrive after the budget stop without queueing", async () => {
    const { controller, replies } = makeController({ budget: { maxToolCalls: 1 } });
    controller.handleToolCalled({ id: "t1", input: {}, executed: true });
    expect(controller.budgetStopped).toBe(true);

    controller.handlePermissionAsk({ id: "late", action: "shell", resources: ["npm test"] });
    expect(replies).toEqual([
      { requestID: "late", decision: "reject", message: AGENTIC_ENDED_MESSAGE },
    ]);
    expect(controller.pending).toEqual([]);
  });

  it("rejects asks pending at the budget stop (nothing outlives the run)", async () => {
    const { controller, replies } = makeController({ budget: { maxToolCalls: 1 } });
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["npm test"] });
    expect(controller.pending).toHaveLength(1);

    controller.handleToolCalled({ id: "t1", input: {}, executed: true });
    expect(controller.budgetStopped).toBe(true);
    expect(controller.pending).toEqual([]);
    expect(replies).toEqual([
      { requestID: "r1", decision: "reject", message: AGENTIC_ENDED_MESSAGE },
    ]);
  });

  it("clears the wall-clock timer when the budget stop fires", () => {
    const { controller } = makeController({ budget: { maxToolCalls: 1, maxWallClockMs: 60_000 } });
    controller.start();
    expect(vi.getTimerCount()).toBe(1);

    controller.handleToolCalled({ id: "t1", input: {}, executed: true });
    expect(controller.budgetStopped).toBe(true);
    // The cap timer is gone: nothing keeps ticking for a settled run.
    expect(vi.getTimerCount()).toBe(0);
  });

  it("clamps non-positive budgets to the defaults", () => {
    const { controller } = makeController({ budget: { maxWallClockMs: 0, maxToolCalls: -3 } });
    expect(controller.budget).toEqual({ maxWallClockMs: 5 * 60_000, maxToolCalls: 40 });
  });
});
describe("budget — wall-clock cap", () => {
  it("fires the interrupt when the wall clock expires", async () => {
    const { controller, interrupt, onBudgetStop } = makeController({
      budget: { maxWallClockMs: 5_000 },
    });
    controller.start();
    expect(controller.startedAt).toBeGreaterThan(0);
    await vi.advanceTimersByTimeAsync(4_999);
    expect(interrupt).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(controller.budgetStopped).toBe(true);
    expect(controller.budgetReason).toBe("wall-clock");
    expect(interrupt).toHaveBeenCalledTimes(1);
    expect(onBudgetStop).toHaveBeenCalledWith("wall-clock");
  });

  it("arms only one wall-clock timer across repeated starts", async () => {
    const { controller, interrupt } = makeController({ budget: { maxWallClockMs: 1_000 } });
    controller.start();
    controller.start();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(interrupt).toHaveBeenCalledTimes(1);
  });

  it("does not fire the wall clock when the run finishes before it", async () => {
    const { controller, interrupt } = makeController({ budget: { maxWallClockMs: 60_000 } });
    controller.start();
    controller.dispose();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(interrupt).not.toHaveBeenCalled();
    expect(controller.budgetStopped).toBe(false);
  });
});

describe("steps — tool-call log", () => {
  it("records called tools and dedupes by id", () => {
    const { controller } = makeController();
    controller.handleToolCalled({ id: "t1", input: { command: "ls" }, executed: true });
    controller.handleToolCalled({ id: "t1", input: { command: "ls" }, executed: true });
    controller.handleToolCalled({ id: "t2", input: { path: "README.md" }, executed: false });
    expect(controller.steps).toEqual([
      { id: "t1", input: { command: "ls" }, executed: true, status: null, at: expect.any(Number) },
      { id: "t2", input: { path: "README.md" }, executed: false, status: null, at: expect.any(Number) },
    ]);
  });

  it("marks outcomes from the success/failed events", () => {
    const { controller } = makeController();
    controller.handleToolCalled({ id: "t1", input: {}, executed: true });
    controller.handleToolCalled({ id: "t2", input: {}, executed: true });
    controller.handleToolFinished("t1", true);
    controller.handleToolFinished("t2", false);
    controller.handleToolFinished("unknown", true);
    expect(controller.steps.map((step) => step.status)).toEqual(["ok", "failed"]);
  });

  it("the default tool-call cap is the types.ts constant (40)", () => {
    const { controller, interrupt } = makeController();
    for (let i = 0; i < 40; i++) {
      controller.handleToolCalled({ id: `t${i}`, input: {}, executed: true });
    }
    expect(controller.budgetStopped).toBe(true);
    expect(interrupt).toHaveBeenCalledTimes(1);
  });
});

describe("dispose", () => {
  it("rejects pending asks, clears every timer and ignores late asks", async () => {
    const { controller, replies, interrupt } = makeController();
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["npm test"] });
    controller.handlePermissionAsk({ id: "r2", action: "edit", resources: ["a.ts"] });

    controller.dispose();
    expect(controller.pending).toEqual([]);
    expect(replies).toEqual([
      { requestID: "r1", decision: "reject", message: AGENTIC_ENDED_MESSAGE },
      { requestID: "r2", decision: "reject", message: AGENTIC_ENDED_MESSAGE },
    ]);
    // Run-ended rejections land in the cancelled bucket, not expired.
    expect(controller.summary.cancelled).toBe(2);
    expect(controller.summary.expired).toBe(0);

    // Cleared timers never fire; a late ask is rejected immediately (the
    // run is over — nothing new may block on the user).
    await vi.advanceTimersByTimeAsync(AGENTIC_ASK_TIMEOUT_MS + 1_000);
    controller.handlePermissionAsk({ id: "r3", action: "shell", resources: ["ls"] });
    expect(controller.pending).toEqual([]);
    expect(replies.filter((reply) => reply.requestID === "r3")).toEqual([
      { requestID: "r3", decision: "reject", message: AGENTIC_ENDED_MESSAGE },
    ]);
    expect(replies).toHaveLength(3);
    expect(controller.summary.cancelled).toBe(3); // r3 counts here too
    expect(interrupt).not.toHaveBeenCalled();
  });

  it("dispose is idempotent", async () => {
    const { controller, replies } = makeController();
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["ls"] });
    controller.dispose();
    controller.dispose();
    expect(replies).toEqual([
      { requestID: "r1", decision: "reject", message: AGENTIC_ENDED_MESSAGE },
    ]);
  });
});

describe("budgetView", () => {
  it("snapshots counts vs caps and elapsed vs wall clock", () => {
    let now = 10_000;
    const { controller } = makeController({ budget: { maxWallClockMs: 60_000, maxToolCalls: 5 }, now: () => now });
    // Not started: elapsed is 0 even as time passes.
    now = 20_000;
    expect(controller.budgetView(now)).toEqual({
      toolCalls: 0,
      maxToolCalls: 5,
      elapsedMs: 0,
      maxWallClockMs: 60_000,
      budgetStopped: false,
      budgetReason: null,
    });
    now = 20_000;
    controller.start(); // startedAt = 20_000
    now = 45_000;
    controller.handleToolCalled({ id: "t1", input: {}, executed: true });
    controller.handleToolCalled({ id: "t2", input: {}, executed: true });
    expect(controller.budgetView(now)).toEqual({
      toolCalls: 2,
      maxToolCalls: 5,
      elapsedMs: 25_000,
      maxWallClockMs: 60_000,
      budgetStopped: false,
      budgetReason: null,
    });
  });

  it("reflects the budget-stopped settlement", () => {
    const { controller } = makeController({ budget: { maxToolCalls: 1 } });
    controller.handleToolCalled({ id: "t1", input: {}, executed: true });
    expect(controller.budgetView(0)).toMatchObject({
      budgetStopped: true,
      budgetReason: "tool-calls",
    });
  });
});

describe("agenticStepViews — transcript rows", () => {
  it("derives display labels from the input shape", () => {
    const { controller } = makeController();
    controller.handleToolCalled({ id: "t1", input: { command: "git status" }, executed: true });
    controller.handleToolCalled({ id: "t2", input: { path: "src/a.ts" }, executed: true });
    controller.handleToolCalled({ id: "t3", input: { path: "src/b.ts", new_string: "x" }, executed: true });
    controller.handleToolCalled({ id: "t4", input: { url: "https://x" }, executed: true });
    controller.handleToolCalled({ id: "t5", input: { pattern: "TODO", path: "src" }, executed: true });
    controller.handleToolFinished("t1", false);
    const views = agenticStepViews(controller.steps);
    expect(views).toEqual([
      { id: "t1", tool: "shell", args: "git status", status: "failed", at: expect.any(Number) },
      { id: "t2", tool: "read", args: "src/a.ts", status: "called", at: expect.any(Number) },
      { id: "t3", tool: "edit", args: "src/b.ts", status: "called", at: expect.any(Number) },
      { id: "t4", tool: "webfetch", args: "https://x", status: "called", at: expect.any(Number) },
      { id: "t5", tool: "search", args: "TODO", status: "called", at: expect.any(Number) },
    ]);
  });

  it("recognizes camelCase path/content keys", () => {
    const { controller } = makeController();
    controller.handleToolCalled({ id: "t1", input: { filePath: "src/a.ts" }, executed: true });
    controller.handleToolCalled({
      id: "t2",
      input: { filePath: "src/b.ts", oldString: "a", newString: "b" },
      executed: true,
    });
    const views = agenticStepViews(controller.steps);
    expect(views).toEqual([
      { id: "t1", tool: "read", args: "src/a.ts", status: "called", at: expect.any(Number) },
      { id: "t2", tool: "edit", args: "src/b.ts", status: "called", at: expect.any(Number) },
    ]);
  });

  it("falls back to the first input key for unknown tool shapes", () => {
    const { controller } = makeController();
    controller.handleToolCalled({ id: "t1", input: { skill_name: "release" }, executed: true });
    expect(agenticStepViews(controller.steps)).toEqual([
      { id: "t1", tool: "skill_name", args: "release", status: "called", at: expect.any(Number) },
    ]);
    expect(agenticStepViews([{ id: "t", input: {}, executed: true, status: null, at: 1 }])).toEqual([
      { id: "t", tool: "tool", args: "", status: "called", at: 1 },
    ]);
  });

  it("keeps the newest cap entries, oldest first", () => {
    const { controller } = makeController();
    for (let i = 0; i < 7; i++) {
      controller.handleToolCalled({ id: `t${i}`, input: { command: `cmd ${i}` }, executed: true });
    }
    const views = agenticStepViews(controller.steps, 3);
    expect(views.map((view) => view.args)).toEqual(["cmd 4", "cmd 5", "cmd 6"]);
  });
});

describe("approval decision helpers", () => {
  it("offers Always for mutation/execution asks, never for read/edit", () => {
    expect(offersAlways("shell")).toBe(true);
    expect(offersAlways("external_directory")).toBe(true);
    expect(offersAlways("subagent")).toBe(true);
    expect(offersAlways("skill")).toBe(true);
    expect(offersAlways("execute")).toBe(true);
    expect(offersAlways("mcp_fs_write_file")).toBe(true);
    expect(offersAlways("question")).toBe(true);
    expect(offersAlways("read")).toBe(false);
    expect(offersAlways("edit")).toBe(false);
  });

  it("collapses an Always decision to once for read/edit asks", () => {
    expect(safeDecision("read", "always")).toBe("once");
    expect(safeDecision("edit", "always")).toBe("once");
    expect(safeDecision("read", "once")).toBe("once");
    expect(safeDecision("read", "reject")).toBe("reject");
    expect(safeDecision("shell", "always")).toBe("always");
    expect(safeDecision("shell", "once")).toBe("once");
  });
});
