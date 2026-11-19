/**
 * AssistantPanel component tests (chunk 5): the per-feature chat-mode-off
 * gate — the panel's affordances hide behind an empty state with the
 * settings path back in; a normal gate state renders the chat surface.
 * A REAL AiStore backs the mock (config is $state, so the mode flip is
 * driven by setFeatureMode).
 */

import { cleanup, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("$lib/components/ai/AiMarkdown.svelte", () => ({
  default: ({ text }: { text: string }) => `<div>${text}</div>`,
}));

vi.mock("$lib/ai/ai.svelte", async () => {
  const { AiStore } = await import("../../ai/ai.svelte");
  const store = new AiStore({
    storage: null,
    providers: {},
    events: false,
    isTauri: () => false,
  });
  return { ai: store };
});

import { ai } from "../../ai/ai.svelte";
import type { AiStore } from "../../ai/ai.svelte";
import AssistantPanel from "./AssistantPanel.svelte";

const store = ai as AiStore;

afterEach(() => {
  cleanup();
});

beforeEach(() => {
  store.chat.clear("/repo");
  store.setOptIn("r1", true);
  store.setFeatureMode("chat", "auto");
});

const PROPS = { repoId: "r1", root: "/repo", repoName: "repo", diffPath: null };

describe("AssistantPanel chat-mode-off gate (chunk 5)", () => {
  it("auto mode renders the chat affordances (composer + tabs)", async () => {
    render(AssistantPanel, { props: PROPS });
    await tick();
    expect(screen.getByLabelText("Ask the assistant")).toBeTruthy();
  });

  it("chat mode off hides the affordances behind the off empty state", async () => {
    store.setFeatureMode("chat", "off");
    render(AssistantPanel, { props: PROPS });
    await tick();
    expect(screen.getByText(/The assistant chat is turned off/)).toBeTruthy();
    expect(screen.queryByLabelText("Ask the assistant")).toBeNull();
    // The path back in stays reachable.
    expect(screen.getByRole("button", { name: "Open AI settings" })).toBeTruthy();
  });
});
