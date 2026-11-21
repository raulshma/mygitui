/**
 * UnifiedTabStrip — the shared tab-strip engine (plan §4.3). The repo /
 * panel / file surfaces test their own cells through their wrappers (see
 * TabStrip.svelte.test.ts, TabGroup.svelte.test.ts); these tests cover the
 * engine itself: the ⋯ overflow menu for width-hidden tabs, the
 * triggerProps contract (roving tabindex + Enter/Space activation) and the
 * two arrow-key idioms (focus-only vs activate-on-arrow).
 *
 * jsdom has no layout, so overflow metrics are stubbed per test; without
 * stubs the strip has no viewport (clientWidth 0) and shows no ⋯.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import Host from "./UnifiedTabStrip.test.host.svelte";

afterEach(cleanup);

interface TestTab {
  id: string;
  label: string;
}

function item(id: string): TestTab {
  return { id, label: `Tab ${id}` };
}

const TABS: TestTab[] = [item("a"), item("b"), item("c")];

function tabButtons(): HTMLButtonElement[] {
  return [...document.querySelectorAll<HTMLButtonElement>("button[role='tab']")];
}

function stripEl(): HTMLElement {
  const node = document.querySelector<HTMLElement>(".test-strip");
  if (node === null) throw new Error("strip not rendered");
  return node;
}

function overflowTrigger(): HTMLButtonElement | null {
  return screen.queryByRole("button", { name: "More tabs" });
}

/** Give the strip a viewport and the tabs 200px-wide rects left to right;
 * the tab rects are viewport-relative and track the strip's scrollLeft. */
function layout(strip: HTMLElement, count = 3, viewport = 500, scrollLeft = 0): void {
  let left = scrollLeft;
  Object.defineProperty(strip, "scrollWidth", {
    configurable: true,
    value: count * 200,
  });
  Object.defineProperty(strip, "clientWidth", {
    configurable: true,
    value: viewport,
  });
  Object.defineProperty(strip, "scrollLeft", {
    configurable: true,
    get: () => left,
    set: (value: number) => {
      left = value;
    },
  });
  // jsdom has no scrollTo; reveal-active only needs the call to land.
  strip.scrollTo = (() => {}) as unknown as HTMLElement["scrollTo"];
  strip.getBoundingClientRect = () =>
    ({ left: 0, width: viewport, right: viewport }) as DOMRect;
  for (const [index, button] of tabButtons().entries()) {
    button.getBoundingClientRect = () =>
      ({
        left: index * 200 - left,
        right: index * 200 - left + 200,
        width: 200,
      }) as DOMRect;
  }
}

describe("UnifiedTabStrip triggerProps contract", () => {
  it("renders a tablist with roving tabindex on the active tab", async () => {
    render(Host, { tabs: TABS, activeId: "b" });
    await tick();

    const list = screen.getByRole("tablist", { name: "Test tabs" });
    expect(list).toBeTruthy();
    const buttons = tabButtons();
    expect(buttons.map((button) => button.tabIndex)).toEqual([-1, 0, -1]);
    expect(buttons.map((button) => button.getAttribute("aria-selected"))).toEqual([
      "false",
      "true",
      "false",
    ]);
    expect(buttons.map((button) => button.dataset.tabIndex)).toEqual(["0", "1", "2"]);
  });

  it("activates from Enter and Space on a tab (div tabs have no native click)", async () => {
    const onActivate = vi.fn();
    render(Host, { tabs: TABS, activeId: "a", onActivate });
    await tick();

    const second = tabButtons()[1]!;
    await fireEvent.keyDown(second, { key: "Enter" });
    expect(onActivate).toHaveBeenCalledWith(TABS[1]);

    await fireEvent.keyDown(second, { key: " " });
    expect(onActivate).toHaveBeenCalledTimes(2);
    // Other keys flow through untouched.
    await fireEvent.keyDown(second, { key: "x" });
    expect(onActivate).toHaveBeenCalledTimes(2);
  });

  it("moves focus with arrows/Home/End without activating by default", async () => {
    const onActivate = vi.fn();
    render(Host, { tabs: TABS, activeId: "a", onActivate });
    await tick();

    const buttons = tabButtons();
    buttons[0]!.focus();
    const list = screen.getByRole("tablist", { name: "Test tabs" });

    await fireEvent.keyDown(list, { key: "ArrowRight" });
    expect(document.activeElement).toBe(buttons[1]);
    expect(onActivate).not.toHaveBeenCalled();

    await fireEvent.keyDown(list, { key: "End" });
    expect(document.activeElement).toBe(buttons[2]);
    await fireEvent.keyDown(list, { key: "Home" });
    expect(document.activeElement).toBe(buttons[0]);
    expect(onActivate).not.toHaveBeenCalled();
  });

  it("activates while arrows move focus with activateOnArrow (TabGroup idiom)", async () => {
    const onActivate = vi.fn();
    render(Host, {
      tabs: TABS,
      activeId: "a",
      onActivate,
      activateOnArrow: true,
    });
    await tick();

    const buttons = tabButtons();
    const list = screen.getByRole("tablist", { name: "Test tabs" });

    await fireEvent.keyDown(list, { key: "ArrowRight" });
    expect(document.activeElement).toBe(buttons[1]);
    expect(onActivate).toHaveBeenCalledWith(TABS[1]);

    await fireEvent.keyDown(list, { key: "ArrowLeft" });
    expect(document.activeElement).toBe(buttons[0]);
    expect(onActivate).toHaveBeenCalledWith(TABS[0]);
  });
});

describe("UnifiedTabStrip overflow menu", () => {
  it("shows no ⋯ trigger when the strip has no layout (jsdom default)", async () => {
    render(Host, { tabs: TABS, activeId: "a" });
    await tick();
    expect(overflowTrigger()).toBeNull();
  });

  it("shows no ⋯ trigger while every tab fits the strip", async () => {
    render(Host, { tabs: TABS, activeId: "a" });
    await tick();
    layout(stripEl(), 3, 1000); // 3 × 200px tabs in a 1000px viewport
    stripEl().dispatchEvent(new Event("scroll"));
    await tick();
    expect(overflowTrigger()).toBeNull();
  });

  it("lists exactly the clipped tabs and activates on pick", async () => {
    const onActivate = vi.fn();
    render(Host, { tabs: TABS, activeId: "a", onActivate });
    await tick();
    layout(stripEl(), 3, 500); // tab c (400–600) clips past 500
    stripEl().dispatchEvent(new Event("scroll"));
    await tick();

    const trigger = overflowTrigger();
    expect(trigger).not.toBeNull();
    await fireEvent.click(trigger!);
    await tick();

    const menu = await screen.findByRole("menu");
    const items = [
      ...menu.querySelectorAll<HTMLButtonElement>("[role='menuitem']"),
    ];
    // Only the clipped tab is listed — not the fully visible a and b.
    expect(items.map((entry) => entry.textContent?.trim())).toEqual(["Tab c"]);

    await fireEvent.click(items[0]!);
    expect(onActivate).toHaveBeenCalledWith(TABS[2]);
  });

  it("tracks the clip set as the strip scrolls", async () => {
    render(Host, { tabs: TABS, activeId: "a" });
    await tick();
    const strip = stripEl();

    // Mid-strip scroll (250px into 600px of tabs): a is fully out on the
    // left, b straddles the left edge, c fits the 500px viewport.
    layout(strip, 3, 500, 250);
    strip.dispatchEvent(new Event("scroll"));
    await tick();

    const trigger = overflowTrigger();
    expect(trigger).not.toBeNull();
    await fireEvent.click(trigger!);
    await tick();
    const menu = await screen.findByRole("menu");
    const items = [
      ...menu.querySelectorAll<HTMLButtonElement>("[role='menuitem']"),
    ];
    expect(items.map((entry) => entry.textContent?.trim())).toEqual([
      "Tab a",
      "Tab b",
    ]);
  });

  it("drops the trigger once the tabs fit again", async () => {
    render(Host, { tabs: TABS, activeId: "a" });
    await tick();
    const strip = stripEl();
    layout(strip, 3, 500);
    strip.dispatchEvent(new Event("scroll"));
    await tick();
    expect(overflowTrigger()).not.toBeNull();

    layout(strip, 3, 1000);
    strip.dispatchEvent(new Event("scroll"));
    await tick();
    expect(overflowTrigger()).toBeNull();
  });
});

describe("UnifiedTabStrip edge fades", () => {
  it("fades the overflowing edge and clears the mask without overflow", async () => {
    render(Host, { tabs: TABS, activeId: "a" });
    await tick();
    const strip = stripEl();
    layout(strip, 3, 500);
    strip.dispatchEvent(new Event("scroll"));
    await tick();
    expect(strip.style.maskImage).toBe(
      "linear-gradient(to left, transparent, black 1.25rem)",
    );

    layout(strip, 3, 1000);
    strip.dispatchEvent(new Event("scroll"));
    await tick();
    expect(strip.style.maskImage).toBe("none");
  });

  it("edgeFades=false keeps the mask off entirely", async () => {
    render(Host, { tabs: TABS, activeId: "a", edgeFades: false });
    await tick();
    const strip = stripEl();
    layout(strip, 3, 500);
    strip.dispatchEvent(new Event("scroll"));
    await tick();
    expect(strip.style.maskImage).toBe("none");
  });
});
