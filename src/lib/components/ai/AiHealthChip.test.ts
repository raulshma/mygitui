/**
 * Component test for AiHealthChip (M12, lane C) — the chip starts gray
 * ("not configured", nothing probed in a fresh jsdom run) and a click
 * dispatches the typed "open-ai-settings" UI event on `document`.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { onUiEvent } from "$lib/palette/events";
import AiHealthChip from "$lib/components/ai/AiHealthChip.svelte";

// The locked-down vite.config.js has no `resolve.conditions: ["browser"]`
// for vitest — redirect `svelte` to the client build (see GraphCanvas.test.ts).
vi.mock("svelte", () =>
  // @ts-expect-error runtime-only redirect into svelte's client build (no d.ts).
  import("../../../../node_modules/svelte/src/index-client.js"),
);

afterEach(() => cleanup());

describe("AiHealthChip", () => {
  it("renders the gray 'not configured' state before any probe", () => {
    const { getByRole, container } = render(AiHealthChip);
    const chip = getByRole("button");
    expect(chip.getAttribute("aria-label")).toContain("not configured");
    expect(chip.textContent).toContain("AI not configured");
    expect(container.querySelector(".ai-health.off")).toBeTruthy();
  });

  it("click dispatches the typed open-ai-settings event", () => {
    const seen: unknown[] = [];
    const off = onUiEvent("open-ai-settings", (detail) => seen.push(detail));
    try {
      const { getByRole } = render(AiHealthChip);
      fireEvent.click(getByRole("button"));
      expect(seen).toHaveLength(1);
      // jsdom normalizes an undefined CustomEvent detail to null.
      expect(seen[0] ?? undefined).toBeUndefined(); // payload-less event
    } finally {
      off();
    }
  });

  it("tooltip lists both backends", async () => {
    // The chip uses the canonical Hint tooltip (no native title=): it
    // opens on hover after the ~300ms delay and lists every backend.
    vi.useFakeTimers();
    try {
      const { getByRole } = render(AiHealthChip);
      const chip = getByRole("button");
      expect(chip.getAttribute("title")).toBeNull();

      fireEvent.pointerEnter(chip);
      await vi.advanceTimersByTimeAsync(400);

      const tooltip = screen.getByText((_, el) =>
        el?.getAttribute("data-slot") === "tooltip-content",
      );
      expect(tooltip.textContent).toContain("opencode:");
      expect(tooltip.textContent).toContain("openrouter:");
    } finally {
      vi.useRealTimers();
      cleanup();
    }
  });
});
