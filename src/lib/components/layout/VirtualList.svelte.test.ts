/**
 * VirtualList component tests (jsdom, no real layout): windowing math with
 * a stubbed viewport, programmatic scrollToIndex, snippet passthrough
 * (item + flat index), aria passthrough and the empty window. A test host
 * (VirtualList.test.host.svelte) supplies the snippets and delegates the
 * instance's scroll API — snippet props are not buildable from plain JS.
 */
import { cleanup, render } from "@testing-library/svelte";
import { afterEach, describe, expect, it } from "vitest";
import { tick } from "svelte";
import VirtualListHost from "./VirtualList.test.host.svelte";
import type { TestItem } from "./VirtualList.test.host.svelte";

function items(count: number): TestItem[] {
  return Array.from({ length: count }, (_, i) => ({ id: `item-${i}` }));
}

/**
 * Stubs the scroller's layout box (jsdom has none). The scroll dispatch
 * re-runs the tracker with the stubbed metrics — the ResizeObserver stub
 * never fires, so the scroll listener is the only update path in tests.
 */
function stubScroller(scroller: HTMLElement, height: number): void {
  let scrollTop = 0;
  Object.defineProperty(scroller, "clientHeight", {
    configurable: true,
    value: height,
  });
  Object.defineProperty(scroller, "scrollTop", {
    configurable: true,
    get: () => scrollTop,
    set: (value: number) => {
      scrollTop = value;
      scroller.dispatchEvent(new Event("scroll"));
    },
  });
  // The ResizeObserver stub never fires; a scroll dispatch is the only way
  // to re-run the tracker with the stubbed metrics after mount.
  scroller.dispatchEvent(new Event("scroll"));
}

afterEach(cleanup);

describe("VirtualList", () => {
  it("renders a window of rows (plus overscan), not the whole list", async () => {
    const { component } = render(VirtualListHost, {
      items: items(1000),
      rowHeight: 24,
      overscan: 2,
    });
    stubScroller(component.scroller() as HTMLElement, 240); // 10 rows visible
    await tick();

    const scroller = component.scroller() as HTMLElement;
    const rows = scroller.querySelectorAll<HTMLElement>(".vl-row");
    expect(rows.length).toBeGreaterThan(10);
    expect(rows.length).toBeLessThan(30);
    // First row mounts at the top offset; the window carries the total height.
    expect(rows[0]!.style.top).toBe("0px");
    expect(scroller.querySelector<HTMLElement>(".vl-rows")!.style.height).toBe(
      `${1000 * 24}px`,
    );
    expect(scroller.querySelector<HTMLElement>(".test-row")!.textContent).toContain(
      "item-0",
    );
  });

  it("windows shift with the scroll offset (spacer + absolute rows)", async () => {
    const { component } = render(VirtualListHost, {
      items: items(1000),
      rowHeight: 24,
      overscan: 0,
    });
    const scroller = component.scroller() as HTMLElement;
    stubScroller(scroller, 240);
    scroller.scrollTop = 24 * 100; // scrolled 100 rows down
    await tick();

    const first = scroller.querySelector<HTMLElement>(".vl-row");
    expect(first!.style.top).toBe(`${24 * 100}px`);
    expect(first!.textContent).toContain("item-100");
  });

  it("passes the flat index to the row snippet", async () => {
    const { component } = render(VirtualListHost, { items: items(5) });
    const scroller = component.scroller() as HTMLElement;
    stubScroller(scroller, 240);
    await tick();
    const rows = [...scroller.querySelectorAll<HTMLElement>(".test-row")];
    expect(rows.map((row) => row.dataset.index)).toEqual(
      rows.map((_, index) => String(index)),
    );
    expect(rows.map((row) => row.dataset.id)).toEqual(
      rows.map((_, index) => `item-${index}`),
    );
  });

  it("applies aria role/label to the row container", async () => {
    const { component } = render(VirtualListHost, {
      items: items(5),
      ariaRole: "list",
      ariaLabel: "Test rows",
    });
    const container = component
      .scroller()!
      .querySelector<HTMLElement>(".vl-rows");
    expect(container!.getAttribute("role")).toBe("list");
    expect(container!.getAttribute("aria-label")).toBe("Test rows");
  });

  it("spreads scrollerProps onto the scrolling viewport", async () => {
    const { component } = render(VirtualListHost, {
      items: items(5),
      scrollerProps: { role: "tree", "aria-label": "Files", tabindex: 0 },
    });
    const scroller = component.scroller()!;
    expect(scroller.getAttribute("role")).toBe("tree");
    expect(scroller.getAttribute("aria-label")).toBe("Files");
    expect(scroller.getAttribute("tabindex")).toBe("0");
  });

  it("renders no rows when items are empty (total height 0)", async () => {
    const { component } = render(VirtualListHost, { items: [] });
    const scroller = component.scroller() as HTMLElement;
    await tick();
    expect(scroller.querySelectorAll(".vl-row").length).toBe(0);
    expect(scroller.querySelector<HTMLElement>(".vl-rows")!.style.height).toBe(
      "0px",
    );
  });

  it("scrollToIndex clamps to the valid range and aligns nearest/center", async () => {
    const { component } = render(VirtualListHost, { items: items(100) });
    const scroller = component.scroller() as HTMLElement;
    stubScroller(scroller, 240); // 10 rows visible

    // Row 0 already visible → no movement.
    component.scrollToIndex(0);
    expect(scroller.scrollTop).toBe(0);
    // Row 50 far below → lands on the bottom edge.
    component.scrollToIndex(50);
    expect(scroller.scrollTop).toBe(50 * 24 + 24 - 240);
    // Row 11 now above the window → lands on the top edge.
    component.scrollToIndex(11);
    expect(scroller.scrollTop).toBe(11 * 24);
    // Center alignment positions the row mid-viewport regardless.
    component.scrollToIndex(10, "center");
    expect(scroller.scrollTop).toBe(10 * 24 + 12 - 120);
    // Out-of-range indexes clamp to the ends.
    component.scrollToIndex(-5);
    expect(scroller.scrollTop).toBe(0);
    component.scrollToIndex(999, "nearest");
    expect(scroller.scrollTop).toBe(99 * 24 + 24 - 240); // clamped to last row
  });

  it("viewportSize/viewportScrollTop reflect the tracked scroller", async () => {
    const { component } = render(VirtualListHost, { items: items(10) });
    const scroller = component.scroller() as HTMLElement;
    stubScroller(scroller, 320);
    scroller.scrollTop = 96;
    expect(component.viewportSize()).toBe(320);
    expect(component.viewportScrollTop()).toBe(96);
    expect(component.scroller()).toBe(scroller);
  });
});
