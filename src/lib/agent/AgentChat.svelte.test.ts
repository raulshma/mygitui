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

// A REAL AiStore backs the mock: config is $state, so the chip's
// reactive default sync can be driven by setFeatureMode in tests.
vi.mock("$lib/ai/ai.svelte", async () => {
  const { AiStore } = await import("../ai/ai.svelte");
  const store = new AiStore({
    storage: null,
    providers: {},
    events: false,
    isTauri: () => false,
  });
  return { ai: store };
});

import { ai } from "$lib/ai/ai.svelte";
import type { AiStore } from "$lib/ai/ai.svelte";
import { AGENTIC_FEATURES } from "$lib/ai/registry";
import AgentChat from "./AgentChat.svelte";

const store = ai as AiStore;
/** Spied per test via vi.spyOn(store, "sendChat"). */
let sendChatSpy: ReturnType<typeof vi.fn>;

afterEach(() => {
  cleanup();
});

beforeEach(() => {
  vi.restoreAllMocks();
  store.chat.clear("/repo");
  store.setOptIn("r1", true);
  store.setFeatureMode("chat", "auto");
  sendChatSpy = vi.spyOn(store, "sendChat").mockResolvedValue({
    id: "a-x",
    role: "assistant",
    text: "answer",
    at: 1,
  } as ChatMessage);
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

  it("derives the default from the per-feature MODE, reactively (chunk 5)", async () => {
    render(AgentChat, { props: PROPS });
    await tick();
    // "auto" → chip on:
    expect(chip().getAttribute("aria-pressed")).toBe("true");
    // A settings flip auto → single-shot re-syncs the chip default while
    // the composer is idle (the reactive sync reads the $state config):
    store.setFeatureMode("chat", "single-shot");
    await tick();
    expect(chip().getAttribute("aria-pressed")).toBe("false");
    // The feature itself stays enabled — the composer is still there.
    expect(screen.getByLabelText("Ask the agent")).toBeTruthy();
    cleanup();
    // "off" → the whole surface hides (mode-off empty state):
    store.setFeatureMode("chat", "off");
    render(AgentChat, { props: PROPS });
    await tick();
    expect(screen.queryByRole("button", { name: "Tools" })).toBeNull();
    expect(screen.getByText(/The agent chat is turned off/)).toBeTruthy();
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
      expect(sendChatSpy).toHaveBeenCalledWith(
        expect.objectContaining({ agentic: false, question: "what changed?" }),
      ),
    );
    // The chip is a per-run override: it stays where the user left it.
    expect(chip().getAttribute("aria-pressed")).toBe("false");
    await fireEvent.click(chip());
    await fireEvent.input(input, { target: { value: "and now?" } });
    await fireEvent.click(screen.getByRole("button", { name: "Send question" }));
    await waitFor(() =>
      expect(sendChatSpy).toHaveBeenLastCalledWith(expect.objectContaining({ agentic: true })),
    );
  });
});

describe("AgentChat turn meta — mode marker (chunk 3)", () => {
  it("renders the mode that actually ran", async () => {
    store.chat.threads["/repo"] = {
      opencodeSessionId: null,
      messages: [
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
      ],
    };
    render(AgentChat, { props: PROPS });
    await tick();
    expect(screen.getByText(/agentic · via m-1 · opencode/)).toBeTruthy();
  });

  it("single-shot turns say so; legacy turns carry no marker", async () => {
    store.chat.threads["/repo"] = {
      opencodeSessionId: null,
      messages: [
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
      ],
    };
    render(AgentChat, { props: PROPS });
    await tick();
    expect(screen.getByText(/single-shot · via m-1/)).toBeTruthy();
    // The legacy bubble's meta has the serving parts but no mode label of
    // its own — two bubbles, two serving footers, one with the marker.
    expect(screen.getAllByText(/via m-1/)).toHaveLength(2);
  });
});
