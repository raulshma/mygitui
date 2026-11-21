/**
 * OverflowMenu — the standard "⋯" trigger for secondary commands (plan
 * §4.3). Thin wrapper over the shadcn dropdown-menu, so these tests cover
 * the wrapper's own contract: the ≥28px hit-target trigger, its accessible
 * name, and that the content snippet renders as the menu body (opening the
 * bits-ui menu is exercised end-to-end by UnifiedTabStrip.svelte.test.ts).
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createRawSnippet } from "svelte";
import OverflowMenu from "./OverflowMenu.svelte";

afterEach(cleanup);

const content = createRawSnippet(() => ({
  render: () => `
    <div role="menuitem" class="overflow-test-item">Copy sha</div>
  `,
}));

describe("OverflowMenu", () => {
  it("renders a named ⋯ trigger on the 28px hit target", async () => {
    render(OverflowMenu, { label: "More actions", content });
    await tick();

    const trigger = screen.getByRole("button", { name: "More actions" });
    expect(trigger).toBeTruthy();
    expect(trigger.classList.contains("size-ui-hit")).toBe(true);
    // bits-ui marks it as a menu trigger.
    expect(trigger.getAttribute("aria-haspopup")).toBe("menu");
  });

  it("opens the menu and renders the content snippet", async () => {
    render(OverflowMenu, { label: "More actions", content });
    await tick();

    await fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    await tick();

    const item = document.querySelector(".overflow-test-item");
    expect(item).not.toBeNull();
    expect(item!.textContent).toBe("Copy sha");
  });

  it("renders a disabled trigger", async () => {
    render(OverflowMenu, { label: "More actions", content, disabled: true });
    await tick();

    const trigger = screen.getByRole("button", { name: "More actions" });
    expect((trigger as HTMLButtonElement).disabled).toBe(true);
  });
});
