/**
 * AgenticApprovalCard component tests: the user-visible contract of one
 * pending tool-approval ask — action + resources rendered readably (long
 * commands truncate with expand), the decision buttons per action
 * (three-way for execution asks, allow-once/deny ONLY for read/edit,
 * answer+Skip for `question`), the 120 s auto-reject countdown (expired
 * disables the buttons), and the quiet "already resolved" state for stale
 * decisions. A real AgenticRunController drives respond; its reply double
 * records the decisions.
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AGENTIC_ASK_TIMEOUT_MS,
  AgenticRunController,
} from "$lib/ai/agenticRun.svelte";
import type { AgenticApproval } from "$lib/ai/agenticRun.svelte";
import AgenticApprovalCard from "./AgenticApprovalCard.svelte";

afterEach(() => {
  cleanup();
});

/** Controller with a recording reply double (respond resolves through it). */
function makeController() {
  const replies: Array<{ requestID: string; decision: string; message: string | null }> = [];
  const controller = new AgenticRunController({
    sessionID: "sess-1",
    reply: async (requestID, decision, message) => {
      replies.push({ requestID, decision, message: message ?? null });
    },
    interrupt: async () => undefined,
  });
  return { controller, replies };
}

/** Enqueues one ask and returns the approval (as the parent binds it). */
function ask(
  controller: AgenticRunController,
  overrides?: Partial<Pick<AgenticApproval, "action" | "resources" | "message">>,
): AgenticApproval {
  const data = {
    id: "r1",
    action: "shell",
    resources: ["npm test"],
    ...overrides,
    // The event's message is optional string (absent != null).
    message: overrides?.message ?? undefined,
  };
  controller.handlePermissionAsk(data);
  return controller.pending.find((approval) => approval.requestID === "r1")!;
}

const T0 = 1_000_000;

function renderCard(
  approval: AgenticApproval,
  controller: AgenticRunController,
  now: number = T0,
): void {
  render(AgenticApprovalCard, { props: { approval, controller, now } });
}

describe("AgenticApprovalCard — shell/three-way asks", () => {
  it("renders the action label and the resource", () => {
    const { controller } = makeController();
    renderCard(ask(controller), controller);
    expect(screen.getByRole("group", { name: "Approval needed: Run command" })).toBeTruthy();
    expect(screen.getByText("npm test")).toBeTruthy();
  });

  it("prefers the ask's own message as the title", () => {
    const { controller } = makeController();
    renderCard(ask(controller, { message: "Install dependencies?" }), controller);
    expect(screen.getByRole("group", { name: "Approval needed: Install dependencies?" })).toBeTruthy();
  });

  it("Allow once responds once", async () => {
    const { controller, replies } = makeController();
    renderCard(ask(controller), controller);
    await fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    await waitFor(() =>
      expect(replies).toEqual([{ requestID: "r1", decision: "once", message: null }]),
    );
  });

  it("Always in this repo responds always", async () => {
    const { controller, replies } = makeController();
    renderCard(ask(controller), controller);
    await fireEvent.click(screen.getByRole("button", { name: "Always in this repo" }));
    await waitFor(() =>
      expect(replies).toEqual([{ requestID: "r1", decision: "always", message: null }]),
    );
  });

  it("Deny responds reject", async () => {
    const { controller, replies } = makeController();
    renderCard(ask(controller), controller);
    await fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    await waitFor(() =>
      expect(replies).toEqual([{ requestID: "r1", decision: "reject", message: null }]),
    );
  });
});

describe("AgenticApprovalCard — read/edit asks never offer Always", () => {
  it("a read ask renders allow-once/deny only, with the hint", async () => {
    const { controller, replies } = makeController();
    renderCard(ask(controller, { action: "read", resources: [".env"] }), controller);
    expect(screen.queryByRole("button", { name: "Always in this repo" })).toBeNull();
    expect(
      screen.getByText("Always-allow isn't offered for file reads/edits — each one needs your approval."),
    ).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    await waitFor(() =>
      expect(replies).toEqual([{ requestID: "r1", decision: "once", message: null }]),
    );
  });

  it("an edit ask renders allow-once/deny only", () => {
    const { controller } = makeController();
    renderCard(ask(controller, { action: "edit", resources: ["src/a.ts"] }), controller);
    expect(screen.queryByRole("button", { name: "Always in this repo" })).toBeNull();
    expect(screen.getByRole("button", { name: "Allow once" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Deny" })).toBeTruthy();
  });
});

describe("AgenticApprovalCard — question asks", () => {
  it("renders the ask message as the title with an answer field and Skip", () => {
    const { controller } = makeController();
    renderCard(
      ask(controller, { action: "question", resources: [], message: "Which branch should I pick?" }),
      controller,
    );
    expect(
      screen.getByRole("group", { name: "Approval needed: Which branch should I pick?" }),
    ).toBeTruthy();
    expect(screen.getByLabelText("Answer the agent's question")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Skip" })).toBeTruthy();
  });

  it("Skip sends the typed answer as the message", async () => {
    const { controller, replies } = makeController();
    renderCard(ask(controller, { action: "question", resources: [] }), controller);
    await fireEvent.input(screen.getByLabelText("Answer the agent's question"), {
      target: { value: "the feature branch" },
    });
    await fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    await waitFor(() =>
      expect(replies).toEqual([{ requestID: "r1", decision: "once", message: "the feature branch" }]),
    );
  });

  it("an empty answer proceeds with a null message", async () => {
    const { controller, replies } = makeController();
    renderCard(ask(controller, { action: "question", resources: [] }), controller);
    await fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    await waitFor(() =>
      expect(replies).toEqual([{ requestID: "r1", decision: "once", message: null }]),
    );
  });
});

describe("AgenticApprovalCard — countdown and expiry", () => {
  it("shows the remaining auto-reject time", () => {
    const { controller } = makeController();
    const approval = ask(controller);
    // `now` is the parent's ticker; anchor it to the ask's arrival time.
    renderCard(approval, controller, approval.askedAt + 60_000);
    expect(screen.getByText("auto-rejects in 1m 0s")).toBeTruthy();
  });

  it("an expired ask disables every button", async () => {
    const { controller, replies } = makeController();
    const approval = ask(controller);
    renderCard(approval, controller, approval.askedAt + AGENTIC_ASK_TIMEOUT_MS + 1);
    for (const name of ["Allow once", "Always in this repo", "Deny"]) {
      const button = screen.getByRole("button", { name }) as HTMLButtonElement;
      expect(button.disabled).toBe(true);
    }
    expect(screen.getByText("timed out")).toBeTruthy();
    await fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    await tick();
    expect(replies).toEqual([]); // expired clicks are no-ops
  });
});

describe("AgenticApprovalCard — reply failure and long resources", () => {
  it("a failed reply keeps the card mounted with a retry affordance", async () => {
    let attempts = 0;
    const replies: Array<{ decision: string }> = [];
    const controller = new AgenticRunController({
      sessionID: "sess-1",
      reply: async (_requestID, decision) => {
        attempts += 1;
        if (attempts === 1) throw new Error("server restart");
        replies.push({ decision });
      },
      interrupt: async () => undefined,
    });
    const approval = ask(controller);
    renderCard(approval, controller);

    await fireEvent.click(screen.getByRole("button", { name: "Allow once" }));
    await waitFor(() =>
      expect(screen.getByText("Reply failed — the ask is still open; try again.")).toBeTruthy(),
    );
    // The ask is still pending and the buttons are live again for retry.
    expect(controller.pending).toHaveLength(1);
    expect(controller.summary.approved).toBe(0);
    const allow = screen.getByRole("button", { name: "Allow once" }) as HTMLButtonElement;
    expect(allow.disabled).toBe(false);

    // The retry succeeds: the decision lands, the tally counts.
    await fireEvent.click(allow);
    await waitFor(() => expect(replies).toEqual([{ decision: "once" }]));
    expect(controller.summary.approved).toBe(1);
  });

  it("long resources truncate with an expand toggle", async () => {
    const { controller } = makeController();
    const longCommand = `git log --format="%H %s" ${"x".repeat(120)}`;
    renderCard(ask(controller, { resources: [longCommand] }), controller);
    const toggle = screen.getByRole("button", { name: longCommand });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await fireEvent.click(toggle);
    // The toggle re-renders (truncated -> full) — re-query it.
    const expandedToggle = screen.getByRole("button", { name: longCommand });
    expect(expandedToggle.getAttribute("aria-expanded")).toBe("true");
    await fireEvent.click(expandedToggle);
    expect(screen.getByRole("button", { name: longCommand }).getAttribute("aria-expanded")).toBe(
      "false",
    );
  });
});
