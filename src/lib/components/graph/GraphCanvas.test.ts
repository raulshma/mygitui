/**
 * Component smoke tests for GraphCanvas.svelte (jsdom — no real canvas
 * backend: getContext is mocked, ResizeObserver stubbed). The heavy logic
 * lives in the pure modules under ./; these tests cover the wiring.
 */

import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import GraphCanvas from "$lib/components/graph/GraphCanvas.svelte";
import type { LogPage } from "$lib/ipc/types";

// The locked-down vite.config.js has no `resolve.conditions: ["browser"]` for
// vitest, so the `svelte` bare import resolves to index-server.js and `mount`
// throws. Redirect it to the client build (same file the `browser` condition
// picks). Applies to the whole module graph of this test file.
vi.mock("svelte", () =>
  // @ts-expect-error runtime-only redirect into svelte's client build (no d.ts).
  import("../../../../node_modules/svelte/src/index-client.js"),
);

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function pageOf(count: number, generation = 1, offset = 0): LogPage {
  const commits = Array.from({ length: count }, (_, i) => ({
    sha: `sha-${generation}-${offset + i}`,
    parents: [],
    author: { name: "Ada", email: "ada@example.com", time: 1, offset_minutes: 0 },
    committer: { name: "Ada", email: "ada@example.com", time: 1, offset_minutes: 0 },
    message: `commit ${offset + i}\n`,
    summary: `commit ${offset + i}`,
    refs: [],
  }));
  const rows = commits.map((c, i) => ({
    sha: c.sha,
    lane: i % 3,
    edges: [{ from: i % 3, to: (i + 1) % 3 }],
    lane_count: 3,
  }));
  return { commits, rows, next_cursor: null, generation };
}

// ---------------------------------------------------------------------------
// Environment stubs
// ---------------------------------------------------------------------------

class ResizeObserverStub implements ResizeObserver {
  static instances: ResizeObserverStub[] = [];
  constructor(readonly callback: ResizeObserverCallback) {
    ResizeObserverStub.instances.push(this);
  }
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
  fire(): void {
    this.callback([], this);
  }
}

/** jsdom layout is all zeros — give the scroll container a size. */
function giveViewport(width: number, height: number): void {
  const el = document.querySelector<HTMLElement>(".gc-scroll");
  expect(el).toBeTruthy();
  Object.defineProperty(el, "clientWidth", { value: width, configurable: true });
  Object.defineProperty(el, "clientHeight", { value: height, configurable: true });
  ResizeObserverStub.instances.forEach((ro) => ro.fire());
}

function makeMockContext(): CanvasRenderingContext2D {
  return new Proxy({} as Record<string, unknown>, {
    get(target, prop) {
      if (typeof prop !== "string") return undefined;
      if (prop in target) return target[prop];
      return () => undefined;
    },
    set(target, prop, value) {
      if (typeof prop === "string") target[prop] = value;
      return true;
    },
  }) as unknown as CanvasRenderingContext2D;
}

let getContextSpy: MockInstance;

beforeEach(() => {
  ResizeObserverStub.instances = [];
  vi.stubGlobal("ResizeObserver", ResizeObserverStub);
  getContextSpy = vi
    .spyOn(HTMLCanvasElement.prototype, "getContext")
    .mockImplementation(() => makeMockContext());
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  getContextSpy.mockRestore();
});

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("GraphCanvas", () => {
  it("renders a canvas role=img labelled with the commit count", () => {
    const { getByRole } = render(GraphCanvas, {
      props: {
        pages: [pageOf(2)],
        onCommitClick: () => {},
        onReachEnd: () => {},
      },
    });
    const canvas = getByRole("img");
    expect(canvas).toBeInstanceOf(HTMLCanvasElement);
    expect(canvas.getAttribute("aria-label")).toBe("Commit graph, 2 commits");
    expect(canvas.getAttribute("tabindex")).toBe("0");
  });

  it("sizes the spacer to rows × rowHeight and adapts to more pages", async () => {
    const rendered = render(GraphCanvas, {
      props: {
        pages: [pageOf(2)],
        onCommitClick: () => {},
        onReachEnd: () => {},
      },
    });
    const spacer = () => rendered.container.querySelector<HTMLElement>(".gc-spacer");
    expect(spacer()?.getAttribute("style")).toContain("height: 48px");
    expect(rendered.getByRole("img").getAttribute("aria-label")).toBe("Commit graph, 2 commits");

    // Parent appends a streamed page (cumulative pages array grows).
    await rendered.rerender({ pages: [pageOf(2), pageOf(3, 1, 2)] });
    expect(spacer()?.getAttribute("style")).toContain("height: 120px");
    expect(rendered.getByRole("img").getAttribute("aria-label")).toBe("Commit graph, 5 commits");
  });

  it("reports the visible window and fires onReachEnd while the buffer is thin", async () => {
    const onVisibleRowsChange = vi.fn();
    const onReachEnd = vi.fn();
    render(GraphCanvas, {
      props: {
        pages: [pageOf(2)],
        onCommitClick: () => {},
        onReachEnd,
        onVisibleRowsChange,
      },
    });
    giveViewport(320, 240);
    await vi.waitFor(() => {
      expect(onVisibleRowsChange).toHaveBeenCalledWith(0, 2);
    });
    // 2 rows loaded < 500-row threshold → parent is asked for more data.
    expect(onReachEnd).toHaveBeenCalled();
  });

  it("hit-tests clicks: lane column ± tolerance fires onCommitClick with the row sha", () => {
    const onCommitClick = vi.fn();
    const { getByRole } = render(GraphCanvas, {
      props: {
        pages: [pageOf(2)],
        onCommitClick,
        onReachEnd: () => {},
      },
    });
    const canvas = getByRole("img");
    // Defaults: padding 10, laneWidth 14 → lane 0 column at x=10; row 0 spans y 0..24.
    fireEvent.click(canvas, { clientX: 10, clientY: 12 });
    expect(onCommitClick).toHaveBeenCalledWith("sha-1-0");
    // 7px right of the lane 0 column: outside the ±6px tolerance.
    fireEvent.click(canvas, { clientX: 17, clientY: 12 });
    expect(onCommitClick).toHaveBeenCalledTimes(1);
    // Lane 1 column (x=24) on row 1 (y 24..48).
    fireEvent.click(canvas, { clientX: 24, clientY: 36 });
    expect(onCommitClick).toHaveBeenCalledWith("sha-1-1");
  });

  it("keyboard: ArrowDown moves the focused row and Enter clicks it", () => {
    const onCommitClick = vi.fn();
    const { getByRole } = render(GraphCanvas, {
      props: {
        pages: [pageOf(3)],
        onCommitClick,
        onReachEnd: () => {},
      },
    });
    const canvas = getByRole("img");
    fireEvent.keyDown(canvas, { key: "ArrowDown" });
    fireEvent.keyDown(canvas, { key: "Enter" });
    expect(onCommitClick).toHaveBeenCalledTimes(1);
    expect(onCommitClick).toHaveBeenCalledWith("sha-1-1");
    fireEvent.keyDown(canvas, { key: "ArrowUp" });
    fireEvent.keyDown(canvas, { key: "Enter" });
    expect(onCommitClick).toHaveBeenCalledWith("sha-1-0");
  });

  it("exposes the imperative scrollToRow via the component instance", () => {
    const rendered = render(GraphCanvas, {
      props: {
        pages: [pageOf(10)],
        onCommitClick: () => {},
        onReachEnd: () => {},
      },
    });
    const component = rendered.component as unknown as {
      scrollToRow?: (index: number) => void;
    } | null;
    expect(typeof component?.scrollToRow).toBe("function");
    expect(() => component?.scrollToRow?.(5)).not.toThrow();
  });

  it("mounts without a 2D context (graceful no-draw, no crash)", () => {
    // jsdom without node-canvas: getContext returns null.
    getContextSpy.mockImplementation(() => null);
    const { getByRole } = render(GraphCanvas, {
      props: {
        pages: [pageOf(1)],
        onCommitClick: () => {},
        onReachEnd: () => {},
      },
    });
    expect(getByRole("img").getAttribute("aria-label")).toBe("Commit graph, 1 commit");
  });
});
