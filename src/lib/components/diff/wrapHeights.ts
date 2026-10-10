/**
 * Wrap-mode row heights for the diff viewer (pure, unit tested).
 *
 * Wrapping breaks the fixed 20px row-height contract, so the virtualizer
 * needs a per-row height estimate BEFORE a row is ever rendered (visible-
 * window arithmetic). Text is monospace at a measured column advance, so
 * the estimate is analytic: visual columns (tabs to their stops) divided by
 * columns-per-line, rounded up, times the line height. Estimates can still
 * drift (wide glyphs, break opportunities) — DiffViewer corrects rendered
 * rows from measured DOM heights; this module only seeds the layout.
 */

import { ROW_HEIGHTS, rowHeight, type DiffRow } from "./rowModel";
import { charWidth, visualColumns } from "./textWidth";

/** Gutter + padding constants — must match DiffRow.svelte's CSS (content-box):
 *  `.no` 3.25rem + 0.5rem padding, `.sign` 1.25rem, `.txt` padding-right 1rem. */
export const GUTTER_PX = {
  /** Per split half: one line-number gutter + sign + text right padding. */
  splitSide: 96,
  /** Unified row: two line-number gutters + sign + text right padding. */
  unified: 156,
} as const;

export interface WrapMetrics {
  /** Usable text width of one split half, px (≤ 0 = cannot wrap → 1 line). */
  splitTextWidth: number;
  /** Usable text width of a unified row, px. */
  unifiedTextWidth: number;
  /** Advance of one monospace column, px. */
  charWidth: number;
}

/** Metrics for the current viewport width (client, scrollbar-excluded). */
export function wrapMetrics(
  viewportWidth: number,
  mode: "split" | "unified",
): WrapMetrics {
  const halfWidth = mode === "split" ? Math.max(0, (viewportWidth - 1) / 2) : 0;
  return {
    splitTextWidth: halfWidth - GUTTER_PX.splitSide,
    unifiedTextWidth: viewportWidth - GUTTER_PX.unified,
    charWidth: charWidth(),
  };
}

/** Visual lines `text` occupies at `width` px (tab-aware, ≥ 1). */
export function wrappedLineCount(
  text: string,
  width: number,
  charWidthPx: number,
): number {
  if (width <= 0 || charWidthPx <= 0) return 1;
  const colsPerLine = Math.max(1, Math.floor(width / charWidthPx));
  return Math.max(1, Math.ceil(visualColumns(text) / colsPerLine));
}

/** Estimated wrapped height of one row (a multiple of the line height). */
export function wrappedRowHeight(row: DiffRow, metrics: WrapMetrics): number {
  switch (row.kind) {
    case "line":
      return (
        wrappedLineCount(row.line.text, metrics.unifiedTextWidth, metrics.charWidth) *
        ROW_HEIGHTS.line
      );
    case "context":
      return (
        wrappedLineCount(row.line.text, metrics.splitTextWidth, metrics.charWidth) *
        ROW_HEIGHTS.line
      );
    case "pair": {
      const left = row.left
        ? wrappedLineCount(row.left.text, metrics.splitTextWidth, metrics.charWidth)
        : 1;
      const right = row.right
        ? wrappedLineCount(row.right.text, metrics.splitTextWidth, metrics.charWidth)
        : 1;
      return Math.max(left, right) * ROW_HEIGHTS.line;
    }
    default:
      // Headers / binary / image stay fixed-height.
      return rowHeight(row);
  }
}

/** Per-row heights for `buildLayout` under wrap mode. */
export function wrappedHeights(
  rows: readonly DiffRow[],
  metrics: WrapMetrics,
): number[] {
  const out = new Array<number>(rows.length);
  for (let i = 0; i < rows.length; i++) out[i] = wrappedRowHeight(rows[i], metrics);
  return out;
}
