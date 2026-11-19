/**
 * AgenticRunTranscript component tests: the live agentic block bound to a
 * job's runtime — the tool-call rolling window (label + args + outcome),
 * stacked approval cards (oldest first, one decision per card), the budget
 * indicator (calls vs cap, elapsed vs wall clock, cap-reached badge), the
 * optional run cancel, and rendering nothing for a non-agentic job. A real
 * AiJobStore + AgenticRunController drive the reactive state.
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgenticRunController } from "$lib/ai/agenticRun.svelte";
import { AiJobStore } from "$lib/ai/jobs.svelte";
import type { AiJob } from "$lib/ai/jobs.svelte";
import AgenticRunTranscript from "./AgenticRunTranscript.svelte";

afterEach(() => {
  cleanup();
});

/** Controller with recording reply/interrupt doubles (default timers). */
function makeController(budget?: { maxWallClockMs?: number; maxToolCalls?: number }) {
  const replies: Array<{ requestID: string; decision: string }> = [];
  const controller = new AgenticRunController({
    sessionID: "sess-1",
    ...(budget ? { budget } : {}),
    reply: async (requestID, decision) => {
      replies.push({ requestID, decision });
    },
    interrupt: async () => undefined,
  });
  return { controller, replies };
}

/** A live job with an agentic runtime bound (the way runAgentic leaves it). */
function boundJob(budget?: { maxWallClockMs?: number; maxToolCalls?: number }): {
  job: AiJob;
  controller: AgenticRunController;
  replies: Array<{ requestID: string; decision: string }>;
  cancel: () => void;
} {
  const store = new AiJobStore({ storage: null });
  const ticket = store.begin({ repoId: "r1", repoName: "repo", feature: "chat" });
  const { controller, replies } = makeController(budget);
  store.bindAgentic(ticket.id, controller);
  controller.start(); // the run glue arms the wall clock
  const job = store.active[ticket.id]!;
  const cancel = vi.fn((): void => undefined);
  return { job, controller, replies, cancel };
}

describe("AgenticRunTranscript", () => {
  it("renders nothing for a job without an agentic runtime", () => {
    const store = new AiJobStore({ storage: null });
    const ticket = store.begin({ repoId: "r1", feature: "commit-message" });
    render(AgenticRunTranscript, {
      props: { job: store.active[ticket.id]!, now: 0 },
    });
    expect(screen.queryByRole("log", { name: "Agent tool transcript" })).toBeNull();
  });

  it("renders the tool-call transcript with outcomes", async () => {
    const { job, controller } = boundJob();
    controller.handleToolCalled({ id: "t1", input: { command: "git status" }, executed: true });
    controller.handleToolFinished("t1", true);
    controller.handleToolCalled({ id: "t2", input: { path: ".env" }, executed: true });
    render(AgenticRunTranscript, { props: { job, now: 0 } });
    await tick();
    const log = screen.getByRole("log", { name: "Agent tool transcript" });
    expect(log.textContent).toContain("shell");
    expect(log.textContent).toContain("git status");
    expect(log.textContent).toContain("read");
    // Outcomes: finished ok vs still running.
    expect(screen.getAllByText("ok")).toHaveLength(1);
    expect(screen.getAllByText("running")).toHaveLength(1);
  });

  it("stacks pending approval cards oldest-first with per-card buttons", async () => {
    const { job, controller } = boundJob();
    controller.handlePermissionAsk({ id: "r1", action: "shell", resources: ["npm test"] });
    controller.handlePermissionAsk({
      id: "r2",
      action: "shell",
      resources: ["git checkout -- ."],
    });
    render(AgenticRunTranscript, { props: { job, now: 0 } });
    await tick();
    const cards = screen.getAllByRole("group", { name: "Approval needed: Run command" });
    expect(cards).toHaveLength(2);
    // Oldest first: the first card carries "npm test".
    expect(cards[0]!.textContent).toContain("npm test");
    expect(cards[1]!.textContent).toContain("git checkout -- .");
    // Decisions stay per-card: answering the FIRST ask only touches r1.
    // (The card's buttons are named by their text content; the first
    // button of a shell card is Allow once.)
    await fireEvent.click(cards[0]!.querySelector("button")!);
    await tick();
    expect(screen.getAllByRole("group", { name: "Approval needed: Run command" })).toHaveLength(1);
  });

  it("shows the budget indicator and the cap-reached badge", async () => {
    const { job, controller } = boundJob({ maxToolCalls: 2, maxWallClockMs: 60_000 });
    controller.handleToolCalled({ id: "t1", input: {}, executed: true });
    render(AgenticRunTranscript, {
      props: { job, now: controller.startedAt + 5_000 },
    });
    await tick();
    const log = screen.getByRole("log", { name: "Agent tool transcript" });
    expect(log.textContent).toContain("1/2 tool calls");
    expect(log.textContent).toContain("5s of 1m 0s budget");

    controller.handleToolCalled({ id: "t2", input: {}, executed: true });
    await tick();
    expect(screen.getByText("tool-call cap reached")).toBeTruthy();
  });

  it("the run cancel delegates to the parent", async () => {
    const { job, cancel } = boundJob();
    render(AgenticRunTranscript, { props: { job, now: 0, onCancel: () => cancel() } });
    await tick();
    await fireEvent.click(screen.getByRole("button", { name: "Cancel run" }));
    expect(cancel).toHaveBeenCalledTimes(1);
  });

  it("omits the cancel affordance when the parent owns one", () => {
    const { job } = boundJob();
    render(AgenticRunTranscript, { props: { job, now: 0 } });
    expect(screen.queryByRole("button", { name: "Cancel run" })).toBeNull();
  });
});
