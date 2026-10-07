/**
 * Unit tests for the canvas painter (render.ts) using a recording 2D-context
 * mock — jsdom has no real canvas backend.
 */

import { describe, expect, it } from "vitest";
import type { GraphEdge, GraphRow } from "$lib/ipc/types";
import { drawGraph, type DrawParams } from "$lib/components/graph/render";

interface RecordedOp {
  op: string;
  args: unknown[];
}

function createMockContext(): { ctx: CanvasRenderingContext2D; ops: RecordedOp[] } {
  const ops: RecordedOp[] = [];
  const styles = new Map<string, unknown>();
  const ctx = new Proxy({} as Record<string, unknown>, {
    get(_target, prop) {
      if (typeof prop !== "string") return undefined;
      if (styles.has(prop)) return styles.get(prop);
      return (...args: unknown[]) => {
        ops.push({ op: prop, args });
      };
    },
    set(_target, prop, value) {
      if (typeof prop === "string") {
        styles.set(prop, value);
        ops.push({ op: `set ${prop}`, args: [value] });
      }
      return true;
    },
  }) as unknown as CanvasRenderingContext2D;
  return { ctx, ops };
}

function row(sha: string, lane: number, lane_count: number, edges: GraphEdge[]): GraphRow {
  return { sha, lane, edges, lane_count };
}

const PALETTE = ["#ff0000", "#00ff00", "#0000ff"];

function baseParams(overrides: Partial<DrawParams> = {}): DrawParams {
  return {
    rows: [],
    first: 0,
    last: 0,
    scrollTop: 0,
    width: 200,
    height: 100,
    rowHeight: 24,
    laneWidth: 14,
    padding: 10,
    palette: PALETTE,
    surface: "#ffffff",
    outline: "#333333",
    ...overrides,
  };
}

function opsNamed(ops: RecordedOp[], op: string): RecordedOp[] {
  return ops.filter((entry) => entry.op === op);
}

describe("drawGraph", () => {
  it("clears the viewport and draws nothing for an empty range", () => {
    const { ctx, ops } = createMockContext();
    drawGraph(ctx, baseParams({ rows: [row("a", 0, 1, [])], first: 0, last: 0 }));
    expect(ops).toEqual([{ op: "clearRect", args: [0, 0, 200, 100] }]);
  });

  it("draws one node per row in the window, lanes colored by lane % 3", () => {
    const rows = [
      row("a", 0, 3, []),
      row("b", 1, 3, []),
      row("c", 2, 3, []),
      row("d", 4, 5, []), // lane 4 → slot 1 → green
      row("e", 0, 3, []),
    ];
    const { ctx, ops } = createMockContext();
    drawGraph(ctx, baseParams({ rows, first: 0, last: rows.length }));

    const arcs = opsNamed(ops, "arc");
    expect(arcs).toHaveLength(5);
    // Node 0: lane 0 at x=10, y center 12, radius 4.
    expect(arcs[0]?.args).toEqual([10, 12, 4, 0, Math.PI * 2]);
    // Node 1: lane 1 at x=24.
    expect(arcs[1]?.args?.[0]).toBe(24);
    // Node 3: lane 4 → palette slot 1.
    const fills = opsNamed(ops, "set fillStyle").map((entry) => entry.args[0]);
    expect(fills).toEqual(["#ff0000", "#00ff00", "#0000ff", "#00ff00", "#ff0000"]);
  });

  it("draws only rows inside [first, last)", () => {
    const rows = [
      row("a", 0, 1, []),
      row("b", 0, 1, []),
      row("c", 0, 1, []),
      row("d", 0, 1, []),
      row("e", 0, 1, []),
    ];
    const { ctx, ops } = createMockContext();
    drawGraph(ctx, baseParams({ rows, first: 1, last: 3, scrollTop: 24 }));

    expect(opsNamed(ops, "arc")).toHaveLength(2);
    // Translation by -scrollTop so absolute row y lands in the viewport.
    expect(opsNamed(ops, "translate")[0]?.args).toEqual([0, -24]);
    // Nodes drawn at absolute centers of rows 1 and 2.
    const arcYs = opsNamed(ops, "arc").map((entry) => entry.args[1]);
    expect(arcYs).toEqual([row1Center(), row2Center()]);
  });

  it("renders straight edges for same-lane connections and colors them by the from lane", () => {
    const rows = [
      row("a", 0, 1, [{ from: 0, to: 0 }]),
      row("b", 0, 1, [{ from: 0, to: 0 }]),
      row("c", 2, 3, [{ from: 2, to: 2 }]),
    ];
    const { ctx, ops } = createMockContext();
    drawGraph(ctx, baseParams({ rows, first: 0, last: rows.length }));

    const lineTos = opsNamed(ops, "lineTo");
    expect(lineTos).toHaveLength(3);
    expect(opsNamed(ops, "bezierCurveTo")).toHaveLength(0);
    // Same-lane edge: x stays on the crisp lane column, y spans both rows.
    const strokeStyles = opsNamed(ops, "set strokeStyle").map((entry) => entry.args[0]);
    expect(strokeStyles).toEqual(["#ff0000", "#ff0000", "#0000ff"]);
    const widths = opsNamed(ops, "set lineWidth").map((entry) => entry.args[0]);
    expect(widths).toEqual([1.5, 1.5, 1.5]);
  });

  it("renders lane changes as bezier curves with horizontal-midpoint controls", () => {
    const rows = [row("a", 0, 2, [{ from: 0, to: 1 }])];
    const { ctx, ops } = createMockContext();
    drawGraph(ctx, baseParams({ rows, first: 0, last: 1 }));

    const beziers = opsNamed(ops, "bezierCurveTo");
    expect(beziers).toHaveLength(1);
    // y1 = 12, y2 = 36 → midY = 24. x positions crisped for a 1.5px stroke:
    // lane 0 (x=10) → 9.75, lane 1 (x=24) → 23.75.
    expect(beziers[0]?.args).toEqual([9.75, 24, 23.75, 24, 23.75, 36]);
    // Edge color follows the `from` lane.
    expect(opsNamed(ops, "set strokeStyle")[0]?.args[0]).toBe("#ff0000");
  });

  it("draws a full-width HEAD band and node ring", () => {
    const rows = [row("a", 0, 1, []), row("head", 1, 2, []), row("c", 0, 1, [])];
    const { ctx, ops } = createMockContext();
    drawGraph(ctx, baseParams({ rows, first: 0, last: 3, headRow: 1 }));

    const rects = opsNamed(ops, "fillRect");
    // Full-width band over the HEAD row (row 1 → y 24..48).
    expect(rects).toContainEqual({ op: "fillRect", args: [0, 24, 200, 24] });
    const ring = opsNamed(ops, "arc").at(-1);
    expect(ring?.args?.slice(0, 3)).toEqual([24, 36, 6.5]); // head lane 1, radius 4 + 2.5
  });

  it("skips bands for rows outside the drawn window", () => {
    const rows = [row("a", 0, 1, []), row("b", 0, 1, [])];
    const { ctx, ops } = createMockContext();
    drawGraph(ctx, baseParams({ rows, first: 0, last: 1, headRow: 1, hoverRow: 1 }));
    expect(opsNamed(ops, "fillRect")).toHaveLength(0);
  });

  it("draws a rounded focus ring only when requested and in-window", () => {
    const rows = [row("a", 0, 1, []), row("b", 0, 1, [])];
    const withRing = createMockContext();
    drawGraph(withRing.ctx, baseParams({ rows, first: 0, last: 2, focusRow: 0, focusRing: true }));
    expect(opsNamed(withRing.ops, "roundRect")).toEqual([
      { op: "roundRect", args: [1, 1, 198, 22, 6] },
    ]);

    const unfocused = createMockContext();
    drawGraph(unfocused.ctx, baseParams({ rows, first: 0, last: 2, focusRow: 0, focusRing: false }));
    expect(opsNamed(unfocused.ops, "roundRect")).toHaveLength(0);

    const offWindow = createMockContext();
    drawGraph(offWindow.ctx, baseParams({ rows, first: 0, last: 1, focusRow: 1, focusRing: true }));
    expect(opsNamed(offWindow.ops, "roundRect")).toHaveLength(0);
  });

  it("falls back to rect when roundRect is unavailable", () => {
    const rows = [row("a", 0, 1, [])];
    const { ctx, ops } = createMockContext();
    (ctx as { roundRect?: unknown }).roundRect = undefined;
    drawGraph(ctx, baseParams({ rows, first: 0, last: 1, focusRow: 0, focusRing: true }));
    expect(opsNamed(ops, "rect")).toEqual([{ op: "rect", args: [1, 1, 198, 22] }]);
    expect(opsNamed(ops, "roundRect")).toHaveLength(0);
  });
});

function row1Center(): number {
  return 1.5 * 24;
}
function row2Center(): number {
  return 2.5 * 24;
}
