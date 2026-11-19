/**
 * AiFeatureButton component tests (chunk 5): the first-use confirm
 * dialog's agentic disclosure — shown when the run MAY go agentic
 * (opencode backend + an agentic-eligible feature in "auto" mode),
 * phrased to stay true when the run silently degrades to single-shot;
 * absent for OpenRouter-only backends, pinned single-shot modes,
 * non-eligible features and plain buttons without a feature kind. The
 * `ai` singleton is mocked; the confirm copy assertions cover the
 * disclosure contract (the run flow itself is the call sites').
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("$lib/components/ai/AiSettings.svelte", () => ({ default: () => "" }));
vi.mock("$lib/components/ai/AiModelPicker.svelte", () => ({ default: () => "" }));
vi.mock("$lib/components/ui/spinner/index.js", () => ({ Spinner: () => "" }));
vi.mock("$lib/toast", () => ({ toast: vi.fn() }));

const DISCLOSURE =
  "If connected to opencode, the agent can read files in this repo. Edits, commands, and anything outside the repo will ask you first. Push and web access are disabled.";

const featureMode = vi.hoisted(() => vi.fn((): "auto" | "single-shot" | "off" => "auto"));
const config = vi.hoisted(() => ({ backend: "opencode" as string }));

vi.mock("$lib/ai/ai.svelte", () => ({
  ai: {
    isEnabled: vi.fn(() => true),
    isFeatureEnabled: vi.fn(() => true),
    featureMode,
    isOptedIn: vi.fn(() => false),
    runOptsFor: vi.fn(() => ({ oneShot: true })),
    peekState: vi.fn(() => ({ busy: false, error: null, last: null })),
    jobs: { activeJob: vi.fn(() => null), cancel: vi.fn() },
    setOptIn: vi.fn(),
    config,
  },
}));

import AiFeatureButton from "./AiFeatureButton.svelte";

afterEach(() => {
  cleanup();
});

beforeEach(() => {
  featureMode.mockReturnValue("auto");
  config.backend = "opencode";
});

function renderButton(kind: "diagnose-workflow" | "explain-hunk" | undefined): void {
  render(AiFeatureButton, {
    props: {
      repoId: "r1",
      ...(kind !== undefined ? { kind } : {}),
      label: "AI diagnose",
      confirmText: "Send this run's log tail to {backend} for a diagnosis?",
      errorPrefix: "AI diagnose failed",
      gateVisibility: false,
      compact: true,
      run: async () => ({
        kind: "diagnose-workflow",
        result: { text: "out", model: "m", backend: "opencode", elapsedMs: 1 },
      }),
    },
  });
}

async function openConfirm(): Promise<void> {
  await fireEvent.click(screen.getByRole("button", { name: "AI diagnose" }));
  await tick();
}

describe("AiFeatureButton — agentic opt-in disclosure (chunk 5)", () => {
  it("shows the disclosure for an agentic-eligible feature in auto mode", async () => {
    renderButton("diagnose-workflow");
    await openConfirm();
    const dialog = screen.getByRole("dialog", { name: "Allow AI for this repository" });
    expect(dialog.textContent).toContain(DISCLOSURE);
    // The confirm question itself is unchanged.
    expect(dialog.textContent).toContain("Send this run's log tail to");
  });

  it("stays hidden for OpenRouter-only backends", async () => {
    config.backend = "openrouter";
    renderButton("diagnose-workflow");
    await openConfirm();
    const dialog = screen.getByRole("dialog", { name: "Allow AI for this repository" });
    expect(dialog.textContent).not.toContain("the agent can read files in this repo");
  });

  it("stays hidden when the feature mode pins single-shot", async () => {
    featureMode.mockReturnValue("single-shot");
    renderButton("diagnose-workflow");
    await openConfirm();
    const dialog = screen.getByRole("dialog", { name: "Allow AI for this repository" });
    expect(dialog.textContent).not.toContain("the agent can read files in this repo");
  });

  it("stays hidden for non-agentic-eligible features", async () => {
    renderButton("explain-hunk");
    await openConfirm();
    const dialog = screen.getByRole("dialog", { name: "Allow AI for this repository" });
    expect(dialog.textContent).not.toContain("the agent can read files in this repo");
  });

  it("stays hidden on buttons without a feature kind", async () => {
    renderButton(undefined);
    await openConfirm();
    const dialog = screen.getByRole("dialog", { name: "Allow AI for this repository" });
    expect(dialog.textContent).not.toContain("the agent can read files in this repo");
  });
});
