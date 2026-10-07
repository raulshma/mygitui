/**
 * Canvas painter for the commit graph — pure function of a DrawParams
 * snapshot, so it is unit-testable with a recording 2D context mock and the
 * component stays a thin shell.
 *
 * Coordinate system: CSS pixels (the caller applies the DPR transform), Y is
 * ABSOLUTE over the virtual list; the caller pre-translates by -scrollTop so
 * rows are drawn at `row * rowHeight` regardless of scroll offset.
 */

import type { GraphRow } from "$lib/ipc/types";
import { crisp, laneX, rowCenterY } from "./layout";
import { laneColor, withAlpha } from "./palette";

export interface DrawParams {
  /** Full flattened row list (backend pre-computed lanes + edges). */
  rows: readonly GraphRow[];
  /** Row window to draw, `[first, last)` — already includes overscan. */
  first: number;
  last: number;
  scrollTop: number;
  /** Viewport size in CSS px. */
  width: number;
  height: number;
  rowHeight: number;
  laneWidth: number;
  padding: number;
  /** Lane colors (see palette.ts). */
  palette: readonly string[];
  surface: string;
  outline: string;
  /** Highlight bands / rings; -1 or undefined disables. */
  hoverRow?: number;
  headRow?: number;
  focusRow?: number;
  /** Draw the focus ring (canvas has keyboard focus). */
  focusRing?: boolean;
  nodeRadius?: number;
  edgeWidth?: number;
}

const TAU = Math.PI * 2;

export function drawGraph(ctx: CanvasRenderingContext2D, p: DrawParams): void {
  ctx.clearRect(0, 0, p.width, p.height);
  const first = Math.max(0, Math.floor(p.first));
  const last = Math.min(p.last, p.rows.length);
  if (last <= first || p.width <= 0 || p.height <= 0) return;

  const nodeRadius = p.nodeRadius ?? 4;
  const edgeWidth = p.edgeWidth ?? 1.5;
  const color = (lane: number): string => laneColor(lane, p.palette);

  ctx.save();
  ctx.translate(0, -p.scrollTop);
  ctx.lineJoin = "round";
  ctx.lineCap = "round";

  // 1) Row bands (hover + HEAD) under the topology.
  const band = (row: number, fill: string): void => {
    if (row >= first && row < last) {
      ctx.fillStyle = fill;
      ctx.fillRect(0, row * p.rowHeight, p.width, p.rowHeight);
    }
  };
  band(p.hoverRow ?? -1, withAlpha(color(0), 0.08));
  band(p.headRow ?? -1, withAlpha(color(0), 0.14));

  // 2) Edges. An edge on row i connects (from, i) to (to, i+1). Same-lane
  //    edges are straight verticals (the common case — fast path); lane
  //    changes get a horizontal-midpoint bezier. Edge color follows the
  //    `from` lane so each branch keeps its lane color along its run.
  for (let i = first; i < last; i += 1) {
    const row = p.rows[i];
    if (!row || row.edges.length === 0) continue;
    const y1 = rowCenterY(i, p.rowHeight);
    const y2 = rowCenterY(i + 1, p.rowHeight);
    for (const edge of row.edges) {
      const x1 = crisp(laneX(edge.from, p.laneWidth, p.padding), edgeWidth);
      const x2 = crisp(laneX(edge.to, p.laneWidth, p.padding), edgeWidth);
      ctx.strokeStyle = color(edge.from);
      ctx.lineWidth = edgeWidth;
      ctx.beginPath();
      ctx.moveTo(x1, y1);
      if (x1 === x2) {
        ctx.lineTo(x2, y2);
      } else {
        const midY = (y1 + y2) / 2;
        ctx.bezierCurveTo(x1, midY, x2, midY, x2, y2);
      }
      ctx.stroke();
    }
  }

  // 3) Commit nodes on top of the edges.
  for (let i = first; i < last; i += 1) {
    const row = p.rows[i];
    if (!row) continue;
    ctx.beginPath();
    ctx.arc(laneX(row.lane, p.laneWidth, p.padding), rowCenterY(i, p.rowHeight), nodeRadius, 0, TAU);
    ctx.fillStyle = color(row.lane);
    ctx.fill();
  }

  // 4) HEAD node ring (the band was drawn in step 1).
  const headRow = p.headRow ?? -1;
  if (headRow >= first && headRow < last) {
    const row = p.rows[headRow];
    if (row) {
      ctx.beginPath();
      ctx.arc(
        laneX(row.lane, p.laneWidth, p.padding),
        rowCenterY(headRow, p.rowHeight),
        nodeRadius + 2.5,
        0,
        TAU,
      );
      ctx.strokeStyle = p.outline;
      ctx.lineWidth = 1.5;
      ctx.stroke();
    }
  }

  // 5) Keyboard focus ring on the focused row.
  const focusRow = p.focusRow ?? -1;
  if (p.focusRing !== false && focusRow >= first && focusRow < last) {
    ctx.strokeStyle = p.outline;
    ctx.lineWidth = 2;
    ctx.beginPath();
    const x = 1;
    const y = focusRow * p.rowHeight + 1;
    const w = Math.max(0, p.width - 2);
    const h = Math.max(0, p.rowHeight - 2);
    if (typeof ctx.roundRect === "function") {
      ctx.roundRect(x, y, w, h, 6);
    } else {
      ctx.rect(x, y, w, h);
    }
    ctx.stroke();
  }

  ctx.restore();
}
