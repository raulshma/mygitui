/**
 * Pure geometry + scrolling math for the commit-graph canvas.
 *
 * Everything here is allocation-light and O(1) except `visibleSlice` (O(k) in
 * the slice length) — the 250k-commit scroll gate relies on that: computing
 * the visible window must stay far below a millisecond per frame.
 */

/** Row index range `[first, last)` (last exclusive). */
export interface VisibleRange {
  first: number;
  last: number;
}

/**
 * Rows intersecting the viewport plus `overscan` rows on each side, clamped to
 * `[0, totalRows]`. Invalid dimensions (rowHeight <= 0, empty list, no
 * viewport) yield an empty range — the caller draws nothing.
 */
export function visibleRange(
  scrollTop: number,
  viewportHeight: number,
  totalRows: number,
  rowHeight: number,
  overscan = 20,
): VisibleRange {
  if (
    totalRows <= 0 ||
    rowHeight <= 0 ||
    viewportHeight <= 0 ||
    !Number.isFinite(scrollTop) ||
    !Number.isFinite(viewportHeight)
  ) {
    return { first: 0, last: 0 };
  }
  const extra = Math.max(0, overscan);
  const firstRow = Math.floor(Math.max(0, scrollTop) / rowHeight);
  const span = Math.ceil(viewportHeight / rowHeight) + 1 + extra;
  return {
    first: Math.max(0, firstRow - extra),
    last: Math.min(totalRows, firstRow + span),
  };
}

/** The slice of `rows` the caller should draw for the current scroll state. */
export function visibleSlice<T>(
  rows: readonly T[],
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  overscan = 20,
): readonly T[] {
  const { first, last } = visibleRange(scrollTop, viewportHeight, rows.length, rowHeight, overscan);
  return last <= first ? [] : rows.slice(first, last);
}

/** X center of lane `lane`'s column: `padding + lane * laneWidth`. */
export function laneX(lane: number, laneWidth: number, padding: number): number {
  return padding + lane * laneWidth;
}

/** Y center of `row`'s band: `(row + 0.5) * rowHeight`. */
export function rowCenterY(row: number, rowHeight: number): number {
  return (row + 0.5) * rowHeight;
}

/** Backing-store pixel size for a CSS-size × device pixel ratio. */
export function pixelSize(cssWidth: number, cssHeight: number, dpr: number): { w: number; h: number } {
  const ratio = Number.isFinite(dpr) && dpr > 0 ? dpr : 1;
  return {
    w: Math.max(0, Math.round(cssWidth * ratio)),
    h: Math.max(0, Math.round(cssHeight * ratio)),
  };
}

/** Set the ctx transform so drawing can use CSS-pixel coordinates. */
export function applyDprTransform(ctx: CanvasRenderingContext2D, dpr: number): void {
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}

/**
 * Align a coordinate to the physical pixel grid for a stroke of `lineWidth`
 * CSS px, so 1px/1.5px lines stay crisp under any DPR transform.
 */
export function crisp(value: number, lineWidth: number): number {
  return Math.round(value - lineWidth / 2) + lineWidth / 2;
}

/**
 * Scroll offset that centers row `index` in the viewport, clamped to the
 * scrollable range. Pure math behind GraphCanvas' imperative `scrollToRow`.
 */
export function scrollToRowTarget(
  index: number,
  totalRows: number,
  rowHeight: number,
  viewportHeight: number,
): number {
  if (totalRows <= 0 || rowHeight <= 0) return 0;
  const clamped = Math.min(Math.max(0, Math.round(index)), totalRows - 1);
  const desired = (clamped + 0.5) * rowHeight - viewportHeight / 2;
  const maxScroll = Math.max(0, totalRows * rowHeight - viewportHeight);
  return Math.min(Math.max(0, desired), maxScroll);
}
