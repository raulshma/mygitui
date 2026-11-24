import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("$lib/toast", () => ({
  toast: vi.fn(),
  dismissToast: vi.fn(),
  pauseToast: vi.fn(),
  resumeToast: vi.fn(),
  getToasts: vi.fn(() => []),
}));

vi.mock("$lib/ipc/client", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("$lib/ipc/client");
  return {
    ...actual,
    opencodeDetect: vi.fn(async () => ({
      installed: true,
      version: "2.1.0",
      major: 2,
      path: "/usr/local/bin/opencode",
    })),
    opencodeServeStatus: vi.fn(async () => ({
      running: true,
      url: "http://127.0.0.1:4096",
      pid: 12345,
    })),
  };
});

import { ai } from "$lib/ai/ai.svelte";
import { appSettings } from "$lib/state/appSettings.svelte";
import { customPrompts } from "$lib/ai/customPrompts.svelte";
import AiSettingsSection from "./AiSettingsSection.svelte";
import AiSettings from "./AiSettings.svelte";

describe("AiSettingsSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ai.setEnabled(true);
    ai.setStatusBarEnabled(true);
    ai.setBackend("opencode");
    ai.setAllowFallback(true);
    ai.setOpencodeEnabled(true);
    ai.setOpencodeMode("managed");
  });

  afterEach(() => {
    cleanup();
  });

  it("renders all core cards and controls", () => {
    render(AiSettingsSection);

    // Section headings & cards
    expect(screen.getByText("AI Features & Activity")).toBeTruthy();
    expect(screen.getByText("Preferred Provider")).toBeTruthy();
    expect(screen.getByText("OpenCode Configuration")).toBeTruthy();
    expect(screen.getByText("OpenRouter Setup")).toBeTruthy();
    expect(screen.getByText("AI Capabilities & Modes")).toBeTruthy();
    expect(screen.getByText("Monthly Usage Budget")).toBeTruthy();
    expect(screen.getByText("Custom Prompts")).toBeTruthy();
    expect(screen.getByText("Saved Tool Permissions")).toBeTruthy();
  });

  it("toggles master AI enable switch reactively", async () => {
    render(AiSettingsSection);

    expect(ai.isEnabled()).toBe(true);
    const masterToggle = screen.getByLabelText("Enable AI features");
    expect(masterToggle.getAttribute("aria-checked")).toBe("true");

    await fireEvent.click(masterToggle);
    expect(ai.isEnabled()).toBe(false);
  });

  it("switches preferred provider reactively", async () => {
    render(AiSettingsSection);

    expect(ai.config.backend).toBe("opencode");
    const openrouterButton = screen.getByRole("radio", { name: /OpenRouter/i });
    await fireEvent.click(openrouterButton);

    expect(ai.config.backend).toBe("openrouter");
  });

  it("renders AiSettings modal wrapper backwards-compatibly", () => {
    render(AiSettings, { open: true });

    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(screen.getByText("AI Features & Activity")).toBeTruthy();
  });
});
