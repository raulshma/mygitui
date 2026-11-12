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

describe("TabStrip drag-to-reorder", () => {
  /**
   * jsdom has no DragEvent/DataTransfer, and @testing-library's fireEvent
   * re-creates events from init objects (dropping non-enumerable assigned
   * fields like a defineProperty dataTransfer). So every drag event here is
   * hand-rolled and dispatched directly.
   */

  /** jsdom has no DataTransfer — hand-roll the payload. */
  function dataTransfer(): DataTransfer {
    const data = new Map<string, string>();
    return {
      setData(type: string, value: string): void {
        data.set(type, value);
      },
      getData(type: string): string {
        return data.get(type) ?? "";
      },
      get types(): string[] {
        return [...data.keys()];
      },
      effectAllowed: "none",
      dropEffect: "none",
    } as unknown as DataTransfer;
  }

  function dragEvent(type: string, dt: DataTransfer, clientX = 0): DragEvent {
    const event = new Event(type, { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", { value: dt });
    Object.defineProperty(event, "clientX", { value: clientX });
    return event as DragEvent;
  }

  /** Dispatches a synthetic drag event; returns !defaultPrevented. */
  function fire(node: Element, type: string, dt: DataTransfer, clientX = 0): boolean {
    return node.dispatchEvent(dragEvent(type, dt, clientX));
  }

  /** The `.tab` cell divs (dragover/drop targets), in strip order. */
  function tabCells(strip: HTMLElement): HTMLElement[] {
    return [...strip.querySelectorAll<HTMLElement>("div.tab")];
  }

  /** The draggable tab button inside a cell — where dragstart originates. */
  function tabButton(cell: HTMLElement): HTMLElement {
    const button = cell.querySelector<HTMLElement>("button[role='tab']");
    if (button === null) throw new Error("tab button not rendered");
    return button;
  }

  function indicator(strip: HTMLElement): HTMLElement | null {
    // The insertion bar is the only aria-hidden span inside a tab cell.
    return strip.querySelector<HTMLElement>("div.tab > span[aria-hidden='true']");
  }

  /**
   * Cells are laid out left to right, 200px each; the insertion side is
   * measured against the tab LABEL's rect (160px — the rest of the cell is
   * the close button), so stub both.
   */
  function grid(cells: HTMLElement[]): void {
    cells.forEach((cell, index) => {
      cell.getBoundingClientRect = () => ({ left: index * 200, width: 200 }) as DOMRect;
      const label = cell.querySelector<HTMLElement>("button[role='tab']");
      if (label !== null) {
        label.getBoundingClientRect = () =>
          ({ left: index * 200, width: 160 }) as DOMRect;
      }
    });
  }

  it("reorders on drop, before or after the hovered tab label's midpoint", async () => {
    render(TabStrip);
    await tick();
    const strip = stripEl();
    const cells = tabCells(strip);
    grid(cells);
    const dt = dataTransfer();

    // Drag the last tab over the first: pointer on the left half of the
    // label (midpoint 80) → before.
    fire(tabButton(cells[3]!), "dragstart", dt);
    await tick();
    fire(cells[0]!, "dragover", dt, 50);
    // Accepted drops are preventDefaulted → dispatchEvent reports false.
    expect(fire(cells[0]!, "drop", dt, 50)).toBe(false);
    await tick();
    expect(tabStore.tabs.map((entry) => entry.id)).toEqual(["repo-4", "repo-1", "repo-2", "repo-3"]);

    // Again: drag repo-4 (now first) over repo-1, pointer on the right
    // half of its label (midpoint 280) → after.
    const cells2 = tabCells(strip);
    grid(cells2);
    const dt2 = dataTransfer();
    fire(tabButton(cells2[0]!), "dragstart", dt2);
    fire(cells2[1]!, "dragover", dt2, 350);
    fire(cells2[1]!, "drop", dt2, 350);
    await tick();
    expect(tabStore.tabs.map((entry) => entry.id)).toEqual(["repo-1", "repo-4", "repo-2", "repo-3"]);
  });

  it("shows the insertion indicator while dragging and cleans up on dragend", async () => {
    render(TabStrip);
    await tick();
    const strip = stripEl();
    const cells = tabCells(strip);
    grid(cells);
    const dt = dataTransfer();

    expect(indicator(strip)).toBeNull();

    fire(tabButton(cells[1]!), "dragstart", dt);
    await tick();
    // The dragged tab itself dims; no indicator until it hovers another.
    expect(cells[1]!.className).toContain("opacity-45");
    expect(indicator(strip)).toBeNull();

    fire(cells[2]!, "dragover", dt, 0); // well left of the label's midpoint
    await tick();
    const bar = indicator(strip);
    expect(bar).not.toBeNull();
    expect(bar!.className).toContain("-left-px"); // before the hovered tab

    fire(cells[0]!, "dragover", dt, 300); // right half of repo-1's label
    await tick();
    expect(indicator(strip)!.className).toContain("-right-px"); // after repo-1

    fire(tabButton(cells[1]!), "dragend", dt);
    await tick();
    expect(indicator(strip)).toBeNull();
    expect(cells[1]!.className).not.toContain("opacity-45");
    expect(tabStore.tabs.map((entry) => entry.id)).toEqual(["repo-1", "repo-2", "repo-3", "repo-4"]);
  });

  it("ignores drags the strip did not start (external file drops untouched)", async () => {
    render(TabStrip);
    await tick();
    const strip = stripEl();
    const cells = tabCells(strip);
    // No dragstart on the strip: an incoming dragover/drop (e.g. a file
    // from the OS) must not be preventDefaulted nor reorder anything.
    expect(fire(cells[0]!, "dragover", dataTransfer(), 0)).toBe(true);
    expect(indicator(strip)).toBeNull();

    fire(cells[0]!, "drop", dataTransfer(), 0);
    await tick();
    expect(tabStore.tabs.map((entry) => entry.id)).toEqual(["repo-1", "repo-2", "repo-3", "repo-4"]);
  });

  it("dropping a tab onto itself is a no-op", async () => {
    render(TabStrip);
    await tick();
    const strip = stripEl();
    const cells = tabCells(strip);
    grid(cells);
    const dt = dataTransfer();

    fire(tabButton(cells[0]!), "dragstart", dt);
    fire(cells[0]!, "dragover", dt, 0);
    await tick();
    // Hovering the dragged tab shows no indicator and accepts no drop.
    expect(indicator(strip)).toBeNull();
    fire(cells[0]!, "drop", dt, 0);
    await tick();
    expect(tabStore.tabs.map((entry) => entry.id)).toEqual(["repo-1", "repo-2", "repo-3", "repo-4"]);
  });
});
