/**
 * AgentChat component tests (chunk 3): the composer's agentic chip —
 * renders with the per-feature default, toggles as a per-run override,
 * survives composer interactions, and rides `sendChat`; the turn's mode
 * marker renders honestly in the serving meta. The `ai` singleton is
 * mocked (the chat seam is what's under test, not the store internals).
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatMessage } from "$lib/ai/chat.svelte";

vi.mock("$lib/components/ai/AiMarkdown.svelte", () => ({
  default: ({ text }: { text: string }) => `<div data-testid="markdown">${text}</div>`,
}));

const sendChat = vi.hoisted(() => vi.fn(async () => undefined));
const conversation = vi.hoisted(() => vi.fn<() => ChatMessage[]>(() => []));
const isFeatureEnabled = vi.hoisted(() => vi.fn(() => true));

vi.mock("$lib/ai/ai.svelte", () => ({
  ai: {
    isEnabled: vi.fn(() => true),
    isOptedIn: vi.fn(() => true),
    isFeatureEnabled,
    setOptIn: vi.fn(),
    chat: {
      conversationFor: () => conversation(),
      busy: {} as Record<string, boolean>,
      clear: vi.fn(),
    },
    sendChat,
    jobs: { activeJob: vi.fn(() => null) },
  },
}));

import AgentChat from "./AgentChat.svelte";

afterEach(() => {
  cleanup();
});

beforeEach(() => {
  sendChat.mockClear();
  conversation.mockReturnValue([]);
  isFeatureEnabled.mockReturnValue(true);
});

const PROPS = { repoId: "r1", root: "/repo", repoName: "repo" };

function chip(): HTMLButtonElement {
  return screen.getByRole("button", {
    name: "Let the assistant use repo tools for this message",
  }) as HTMLButtonElement;
}

describe("AgentChat composer chip (chunk 3)", () => {
  it("defaults to the per-feature chat switch and toggles", async () => {
    render(AgentChat, { props: PROPS });
    await tick();
    expect(chip().getAttribute("aria-pressed")).toBe("true");
    await fireEvent.click(chip());
    expect(chip().getAttribute("aria-pressed")).toBe("false");
    await fireEvent.click(chip());
    expect(chip().getAttribute("aria-pressed")).toBe("true");
  });

  it("derives the default from the per-feature switch (off when chat is off)", async () => {
    isFeatureEnabled.mockReturnValue(false);
    render(AgentChat, { props: PROPS });
    await tick();
    expect(chip().getAttribute("aria-pressed")).toBe("false");
  });

  it("chip state survives composer interactions and rides send", async () => {
    render(AgentChat, { props: PROPS });
    await tick();
    const input = screen.getByLabelText("Ask the agent") as HTMLInputElement;
    await fireEvent.click(chip());
    expect(chip().getAttribute("aria-pressed")).toBe("false");
    await fireEvent.input(input, { target: { value: "what changed?" } });
    // Typing does not touch the chip.
    expect(chip().getAttribute("aria-pressed")).toBe("false");
    await fireEvent.click(screen.getByRole("button", { name: "Send question" }));
    await waitFor(() =>
      expect(sendChat).toHaveBeenCalledWith(
        expect.objectContaining({ agentic: false, question: "what changed?" }),
      ),
    );
    // The chip is a per-run override: it stays where the user left it.
    expect(chip().getAttribute("aria-pressed")).toBe("false");
    await fireEvent.click(chip());
    await fireEvent.input(input, { target: { value: "and now?" } });
    await fireEvent.click(screen.getByRole("button", { name: "Send question" }));
    await waitFor(() =>
      expect(sendChat).toHaveBeenLastCalledWith(expect.objectContaining({ agentic: true })),
    );
  });
});

describe("AgentChat turn meta — mode marker (chunk 3)", () => {
  it("renders the mode that actually ran", async () => {
    conversation.mockReturnValue([
      { id: "u1", role: "user", text: "q", at: 1 },
      {
        id: "a1",
        role: "assistant",
        text: "answer",
        at: 2,
        mode: "agentic",
        model: "m-1",
        backend: "opencode",
        elapsedMs: 5,
      } as ChatMessage,
    ]);
    render(AgentChat, { props: PROPS });
    await tick();
    expect(screen.getByText(/agentic · via m-1 · opencode/)).toBeTruthy();
  });

  it("single-shot turns say so; legacy turns carry no marker", async () => {
    conversation.mockReturnValue([
      {
        id: "a2",
        role: "assistant",
        text: "old",
        at: 1,
        model: "m-1",
        backend: "opencode",
        elapsedMs: 5,
      } as ChatMessage,
      {
        id: "a3",
        role: "assistant",
        text: "new",
        at: 2,
        mode: "single-shot",
        model: "m-1",
        backend: "opencode",
        elapsedMs: 5,
      } as ChatMessage,
    ]);
    render(AgentChat, { props: PROPS });
    await tick();
    expect(screen.getByText(/single-shot · via m-1/)).toBeTruthy();
    // The legacy bubble's meta has the serving parts but no mode label of
    // its own — two bubbles, two serving footers, one with the marker.
    expect(screen.getAllByText(/via m-1/)).toHaveLength(2);
  });
});
