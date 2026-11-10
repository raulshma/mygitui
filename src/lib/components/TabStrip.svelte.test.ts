/**
 * TabStrip overflow behavior: the active tab is scrolled back into view,
 * vertical wheel moves the horizontal strip, and the edge fades track the
 * scroll position. jsdom has no layout, so metrics are stubbed per test.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetTabStore, setActive, tabStore, type Tab } from "$lib/stores/tabs.svelte";
import { resetTransport, setTransport } from "$lib/ipc/client";
import TabStrip from "./TabStrip.svelte";

function tab(index: number): Tab {
  return {
    id: `repo-${index}`,
    name: `repo ${index}`,
    root: `/repos/${index}`,
    status: null,
    generation: 0,
    deviceId: null,
    openError: null,
  } as Tab;
}

/** Gives the strip a 500px viewport over 200px-wide tabs. */
function layout(strip: HTMLElement, scrollLeft = 0): { scrollTo: ReturnType<typeof vi.fn> } {
  let left = scrollLeft;
  Object.defineProperty(strip, "scrollWidth", { configurable: true, value: 2000 });
  Object.defineProperty(strip, "clientWidth", { configurable: true, value: 500 });
  Object.defineProperty(strip, "scrollLeft", {
    configurable: true,
    get: () => left,
    set: (value: number) => {
      left = value;
    },
  });
  strip.getBoundingClientRect = () => ({ left: 0, width: 500 }) as DOMRect;
  const scrollTo = vi.fn((options: ScrollToOptions) => {
    left = options.left ?? 0;
  });
  strip.scrollTo = scrollTo as unknown as HTMLElement["scrollTo"];
  for (const [index, button] of tabButtons(strip).entries()) {
    // Viewport-relative, so it must track the strip's current scroll.
    button.getBoundingClientRect = () =>
      ({ left: index * 200 - left, width: 200 }) as DOMRect;
  }
  return { scrollTo };
}

function tabButtons(strip: HTMLElement): HTMLButtonElement[] {
  return [...strip.querySelectorAll<HTMLButtonElement>("button[role='tab']")];
}

function stripEl(): HTMLElement {
  const node = document.querySelector<HTMLElement>(".tabstrip");
  if (node === null) throw new Error("tabstrip not rendered");
  return node;
}

beforeEach(() => {
  setTransport(async () => null);
  resetTabStore();
  tabStore.tabs = [tab(1), tab(2), tab(3), tab(4)];
  tabStore.activeId = "repo-1";
});

afterEach(() => {
  cleanup();
  resetTabStore();
  resetTransport();
});

describe("TabStrip overflow", () => {
  it("leaves the strip alone when the active tab is already in view", async () => {
    render(TabStrip);
    await tick();
    const { scrollTo } = layout(stripEl());
    setActive("repo-2");
    await tick();
    expect(scrollTo).not.toHaveBeenCalled();
  });

  it("scrolls a newly activated off-screen tab into view", async () => {
    render(TabStrip);
    await tick();
    const { scrollTo } = layout(stripEl());
    setActive("repo-4");
    await tick();
    // Tab 4 spans 600–800px; the 500px viewport must reach 800 - 500 + 8.
    expect(scrollTo).toHaveBeenCalledWith({ left: 308, behavior: "smooth" });
  });

  it("scrolls back to a tab left of the viewport", async () => {
    tabStore.activeId = "repo-4";
    render(TabStrip);
    await tick();
    const { scrollTo } = layout(stripEl(), 600);
    setActive("repo-1");
    await tick();
    expect(scrollTo).toHaveBeenCalledWith({ left: 0, behavior: "smooth" });
  });

  it("translates vertical wheel into horizontal scroll and cancels the page scroll", async () => {
    render(TabStrip);
    await tick();
    const strip = stripEl();
    layout(strip);
    const event = new WheelEvent("wheel", { deltaY: 120, cancelable: true });
    strip.dispatchEvent(event);
    expect(strip.scrollLeft).toBe(120);
    expect(event.defaultPrevented).toBe(true);
  });

  it("leaves horizontal trackpad gestures and non-overflowing strips alone", async () => {
    render(TabStrip);
    await tick();
    const strip = stripEl();
    layout(strip);
    const horizontal = new WheelEvent("wheel", { deltaX: 90, deltaY: 10, cancelable: true });
    strip.dispatchEvent(horizontal);
    expect(strip.scrollLeft).toBe(0);
    expect(horizontal.defaultPrevented).toBe(false);

    Object.defineProperty(strip, "scrollWidth", { configurable: true, value: 500 });
    const vertical = new WheelEvent("wheel", { deltaY: 120, cancelable: true });
    strip.dispatchEvent(vertical);
    expect(strip.scrollLeft).toBe(0);
    expect(vertical.defaultPrevented).toBe(false);
  });

  it("fades only the edge with more content", async () => {
    render(TabStrip);
    await tick();
    const strip = stripEl();
    layout(strip);
    strip.dispatchEvent(new Event("scroll"));
    await tick();
    // At rest only the right edge hides content; the fade points back left.
    expect(strip.style.maskImage).toBe("linear-gradient(to left, transparent, black 1.25rem)");

    strip.scrollLeft = 1500;
    strip.dispatchEvent(new Event("scroll"));
    await tick();
    expect(strip.style.maskImage).toBe("linear-gradient(to right, transparent, black 1.25rem)");
  });

  it("keeps roving tabindex on the active tab and moves focus with arrows", async () => {
    render(TabStrip);
    await tick();
    const buttons = tabButtons(stripEl());
    expect(buttons.map((button) => button.tabIndex)).toEqual([0, -1, -1, -1]);
    buttons[0]!.focus();
    await fireEvent.keyDown(stripEl(), { key: "ArrowRight" });
    expect(document.activeElement).toBe(buttons[1]);
    await fireEvent.keyDown(stripEl(), { key: "End" });
    expect(document.activeElement).toBe(buttons[3]);
  });

  it("closes a tab from its close button", async () => {
    render(TabStrip);
    await tick();
    await fireEvent.click(screen.getByLabelText("Close repo 2"));
    expect(tabStore.tabs.map((entry) => entry.id)).toEqual(["repo-1", "repo-3", "repo-4"]);
  });
});