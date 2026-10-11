/**
 * Unit tests for the list↔canvas scroll-sync module
 * (`$lib/stores/historyScrollSync`). All elements are fake plain objects —
 * no real DOM — and the view-owned window state is mirrored in the harness,
 * exactly like HistoryView's `$state` ports. Covers the three strategies:
 * fast path (exact scrollTop mirroring), fallback (`scrollToRow` without a
 * canvas scroll element), degraded local-window mode, plus the ≥ 1px
 * equality guards that keep the two-way pushes from feeding back.
 */

import { describe, expect, it, vi } from "vitest";
import {
  createHistoryScrollSync,
  type CanvasWindow,
  type GraphScrollPort,
  type ScrollEndpoint,
  type ScrollWindow,
} from "./historyScrollSync";

const ROW_HEIGHT = 24;
const OVERSCAN = 10;

/** Stand-in for a real scroll container. */
interface FakeElement {
  scrollTop: number;
  clientHeight: number;
}

function fakeElement(top = 0, height = 240): FakeElement {
  return { scrollTop: top, clientHeight: height };
}

type ElementBox = { el: FakeElement | null };

/** Port over a nullable fake element — `null` models "unmounted" (null scrollTop). */
function endpoint(box: ElementBox): ScrollEndpoint {
  return {
    getScrollTop: () => box.el?.scrollTop ?? null,
    setScrollTop: (top) => {
      if (box.el) box.el.scrollTop = top;
    },
    getClientHeight: () => box.el?.clientHeight ?? 0,
  };
}

interface Harness {
  listEl: FakeElement;
  list: ElementBox;
  graph: ElementBox;
  scrollToRow: ReturnType<typeof vi.fn>;
  ownRange: ScrollWindow;
  canvasWindow: CanvasWindow | null;
  ownRangeWrites: number;
  sync: ReturnType<typeof createHistoryScrollSync>;
}

function harness(
  opts: { total?: number; listTop?: number; graphTop?: number | null } = {},
): Harness {
  const list: ElementBox = { el: fakeElement(opts.listTop ?? 0) };
  const graph: ElementBox = {
    el: opts.graphTop === null ? null : fakeElement(opts.graphTop ?? 0),
  };
  const scrollToRow = vi.fn();
  const h: Harness = {
    listEl: list.el as FakeElement,
    list,
    graph,
    scrollToRow,
    ownRange: { start: 0, end: 0 },
    canvasWindow: null,
    ownRangeWrites: 0,
    sync: createHistoryScrollSync({
      list: endpoint(list),
      graph: { ...endpoint(graph), scrollToRow } as GraphScrollPort,
      rowHeight: ROW_HEIGHT,
      overscan: OVERSCAN,
      totalRows: () => opts.total ?? 1_000,
      ownRange: () => h.ownRange,
      setOwnRange: (r) => {
        h.ownRange = r;
        h.ownRangeWrites++;
      },
      canvasWindow: () => h.canvasWindow,
      setCanvasWindow: (w) => (h.canvasWindow = w),
    }),
  };
  return h;
}

// ---------------------------------------------------------------------------
// Render window (degraded local-window mode vs live canvas window)
// ---------------------------------------------------------------------------

describe("renderWindow", () => {
  it("adopts the canvas's reported window while it is live", () => {
    const { sync } = harness();
    sync.onCanvasVisible(5, 30);
    expect(sync.renderWindow()).toEqual({ start: 5, end: 30 });
  });

  it("clamps a negative canvas first row (overscan) to 0", () => {
    const { sync } = harness();
    sync.onCanvasVisible(-20, 15);
    expect(sync.renderWindow()).toEqual({ start: 0, end: 15 });
  });

  it("falls back to the local window when no canvas window was reported", () => {
    const h = harness({ total: 100 });
    h.sync.updateOwnRange();
    expect(h.sync.renderWindow()).toEqual(h.ownRange);
  });

  it("ignores an empty or inverted canvas window (not live)", () => {
    const h = harness({ total: 100, listTop: 48 });
    h.sync.updateOwnRange();
    h.sync.onCanvasVisible(10, 10); // last <= first
    expect(h.sync.renderWindow()).toEqual(h.ownRange);
    h.sync.onCanvasVisible(15, 12); // inverted
    expect(h.sync.renderWindow()).toEqual(h.ownRange);
  });
});

// ---------------------------------------------------------------------------
// Local window computation
// ---------------------------------------------------------------------------

describe("updateOwnRange", () => {
  it("computes the visible window ± overscan from scrollTop + height", () => {
    const h = harness({ total: 100, listTop: 24 * 20 }); // row 20 on top, 10 rows tall
    h.sync.updateOwnRange();
    expect(h.ownRange).toEqual({ start: 10, end: 40 }); // 20-10 .. 20+10+10
  });

  it("clamps to [0, totalRows]", () => {
    const h = harness({ total: 15 }); // 0+10+10 would overrun 15
    h.sync.updateOwnRange();
    expect(h.ownRange).toEqual({ start: 0, end: 15 });

    const h2 = harness({ total: 100, listTop: 24 * 95 }); // row 95 on top
    h2.sync.updateOwnRange();
    expect(h2.ownRange).toEqual({ start: 85, end: 100 }); // end clamps to total
  });

  it("is a no-op while the scroller is unmounted or zero-height", () => {
    const h = harness();
    h.list.el = null; // unmounted
    h.sync.updateOwnRange();
    expect(h.ownRangeWrites).toBe(0);

    h.list.el = fakeElement(0, 0); // hidden (0px)
    h.sync.updateOwnRange();
    expect(h.ownRangeWrites).toBe(0);
  });

  it("reports only on actual change", () => {
    const h = harness({ total: 100 });
    h.sync.updateOwnRange();
    expect(h.ownRangeWrites).toBe(1);
    h.sync.updateOwnRange(); // identical window → no write
    expect(h.ownRangeWrites).toBe(1);

    h.listEl.scrollTop = 24;
    h.sync.updateOwnRange();
    expect(h.ownRangeWrites).toBe(2);
  });
});

// ---------------------------------------------------------------------------
// List → canvas (fast path + fallback)
// ---------------------------------------------------------------------------

describe("onListScroll (list → canvas)", () => {
  it("mirrors the list scrollTop into the canvas (fast path)", () => {
    const h = harness({ listTop: 480 });
    const w = h.sync.onListScroll();
    expect(h.graph.el?.scrollTop).toBe(480);
    expect(w).toEqual({ start: 10, end: 40 }); // updated render window out
  });

  it("equality-guards the write: < 1px deltas are left alone", () => {
    const h = harness({ listTop: 500, graphTop: 500.5 });
    h.sync.onListScroll();
    expect(h.graph.el?.scrollTop).toBe(500.5); // untouched — no feedback loop
  });

  it("falls back to scrollToRow(center row) without a canvas scroll element", () => {
    const h = harness({ listTop: 480, graphTop: null }); // 480 + 240/2 = 600 → row 25
    h.sync.onListScroll();
    expect(h.scrollToRow).toHaveBeenCalledWith(25);
  });
});

// ---------------------------------------------------------------------------
// Canvas → list follow
// ---------------------------------------------------------------------------

describe("onCanvasVisible (canvas → list)", () => {
  it("adopts the reported window and follows the canvas scrollTop exactly", () => {
    const h = harness({ graphTop: 480 });
    h.sync.onCanvasVisible(20, 40);
    expect(h.canvasWindow).toEqual({ first: 20, last: 40 });
    expect(h.listEl.scrollTop).toBe(480);
  });

  it("equality-guards the follow: an aligned canvas causes no list write", () => {
    const h = harness({ listTop: 480, graphTop: 480 });
    h.listEl.scrollTop = 480.5; // sub-pixel apart → within the guard
    h.sync.onCanvasVisible(20, 40);
    expect(h.listEl.scrollTop).toBe(480.5); // untouched
  });
});

// ---------------------------------------------------------------------------
// scrollToIndex (keyboard nav / restored selection)
// ---------------------------------------------------------------------------

describe("scrollToIndex", () => {
  it("scrolls up to reveal a row above the viewport and mirrors to canvas", () => {
    const h = harness({ listTop: 480, graphTop: 480 });
    h.sync.scrollToIndex(2); // top = 48 < 480
    expect(h.listEl.scrollTop).toBe(48);
    expect(h.graph.el?.scrollTop).toBe(48);
  });

  it("bottom-aligns a row below the viewport", () => {
    const h = harness({ listTop: 0, graphTop: 0 });
    h.sync.scrollToIndex(20); // 480 + 24 > 240 → 480 + 24 - 240
    expect(h.listEl.scrollTop).toBe(264);
    expect(h.graph.el?.scrollTop).toBe(264);
  });

  it("leaves the scroll alone when the row is already fully visible", () => {
    const h = harness({ listTop: 0, graphTop: 0 });
    h.sync.scrollToIndex(5); // 120..144 inside 0..240
    expect(h.listEl.scrollTop).toBe(0);
    expect(h.graph.el?.scrollTop).toBe(0); // equality guard: no push
  });

  it("routes to the canvas fallback while the list is unmounted", () => {
    const h = harness({ graphTop: null });
    h.list.el = null;
    h.sync.scrollToIndex(7);
    expect(h.scrollToRow).toHaveBeenCalledWith(7);
  });
});

// ---------------------------------------------------------------------------
// Reset (repo re-target)
// ---------------------------------------------------------------------------

describe("reset", () => {
  it("forgets both windows and zeros both scrollers", () => {
    const h = harness({ listTop: 480, graphTop: 480 });
    h.sync.onCanvasVisible(20, 40);
    h.sync.reset();
    expect(h.listEl.scrollTop).toBe(0);
    expect(h.graph.el?.scrollTop).toBe(0);
    expect(h.canvasWindow).toBeNull();
    expect(h.ownRange).toEqual({ start: 0, end: 0 });
    expect(h.sync.renderWindow()).toEqual({ start: 0, end: 0 });
  });

  it("tolerates unmounted elements", () => {
    const h = harness({ graphTop: null });
    h.list.el = null;
    expect(() => h.sync.reset()).not.toThrow();
    expect(h.canvasWindow).toBeNull();
  });
});
