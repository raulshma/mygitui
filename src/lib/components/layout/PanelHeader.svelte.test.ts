/**
 * PanelHeader — the shared one-row header pattern (plan §4.3). Covers the
 * presentational contract: title (12px meta ramp), count badge (11px micro
 * ramp), the optional filter toggle (28px hit target, pressed state,
 * onFilterToggle) and the leading/trailing snippet slots.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createRawSnippet } from "svelte";
import PanelHeader from "./PanelHeader.svelte";

afterEach(cleanup);

const leading = createRawSnippet(() => ({
  render: () => `<span class="test-leading">L</span>`,
}));
const trailing = createRawSnippet(() => ({
  render: () => `<span class="test-trailing">T</span>`,
}));

describe("PanelHeader", () => {
  it("renders the title on the 12px meta ramp and no optional parts", async () => {
    render(PanelHeader, { title: "Branches" });
    await tick();

    const title = screen.getByText("Branches");
    expect(title.classList.contains("text-ui-meta")).toBe(true);
    // No badge / filter / slots when their props are absent.
    expect(document.querySelector("[data-slot='badge']")).toBeNull();
    expect(screen.queryByRole("button", { name: "Show filter" })).toBeNull();
    expect(document.querySelector(".test-leading")).toBeNull();
    expect(document.querySelector(".test-trailing")).toBeNull();
  });

  it("shows the count badge on the 11px micro ramp", async () => {
    render(PanelHeader, { title: "Stashes", count: 12 });
    await tick();

    const badge = screen.getByText("12").closest("[data-slot='badge']");
    expect(badge).not.toBeNull();
    expect(badge!.classList.contains("text-ui-micro")).toBe(true);
  });

  it("toggles the filter through the pressed-state icon button", async () => {
    const onFilterToggle = vi.fn();
    const { rerender } = render(PanelHeader, {
      title: "Status",
      filterOn: false,
      onFilterToggle,
    });
    await tick();

    const toggle = screen.getByRole("button", { name: "Show filter" });
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    // The filter glyph sits inside a 28px hit target (plan §4.1).
    expect(toggle.classList.contains("size-ui-hit")).toBe(true);

    await fireEvent.click(toggle);
    expect(onFilterToggle).toHaveBeenCalledWith(true);

    await rerender({ title: "Status", filterOn: true, onFilterToggle });
    const active = screen.getByRole("button", { name: "Hide filter" });
    expect(active.getAttribute("aria-pressed")).toBe("true");
    expect(active.classList.contains("bg-primary/10")).toBe(true);
  });

  it("renders the leading and trailing snippet slots", async () => {
    render(PanelHeader, {
      title: "History",
      leading,
      trailing,
      filterOn: false,
    });
    await tick();

    // Leading sits before the title; trailing after the filter toggle.
    const row = document.querySelector(".panel-title")!.parentElement!;
    const children = [...row.children];
    expect(children.findIndex((el) => el.classList.contains("test-leading"))).toBeLessThan(
      children.findIndex((el) => el.classList.contains("panel-title")),
    );
    expect(
      children.findIndex((el) => el.classList.contains("test-trailing")),
    ).toBeGreaterThan(
      children.findIndex((el) => el.getAttribute("aria-pressed") !== null),
    );
  });
});
