/**
 * Hit-testing for the commit-graph canvas.
 *
 * Coordinates are CANVAS-LOCAL CSS pixels (pointer event minus the canvas
 * bounding rect). Row resolution uses the live scrollTop: the canvas is a
 * sticky viewport over the virtual list, so the data row under a canvas Y is
 * `floor((scrollTop + y) / rowHeight)`. A lane hit means the X is within
 * `tolerance` px of a lane column that exists on that row (node or edge
 * channel) — the B2 contract treats that as a commit click.
 */

export interface HitParams {
  /** Canvas-local pointer X (CSS px). */
  x: number;
  /** Canvas-local pointer Y (CSS px). */
  y: number;
  /** Current scroll offset of the virtual list. */
  scrollTop: number;
  /** Full flattened row list (only `lane_count` is read). */
  rows: readonly { lane_count: number }[];
  rowHeight: number;
  laneWidth: number;
  padding: number;
  /** Max distance from a lane column center that still counts (default 6). */
  tolerance?: number;
}

export interface HitResult {
  /** Absolute row index, or -1 when the pointer is outside the row range. */
  row: number;
  /** Lane whose column was hit, or null when X missed every lane. */
  lane: number | null;
}

/** Resolve the absolute row under canvas-local `y`; -1 when out of range. */
export function rowAt(y: number, scrollTop: number, rowHeight: number, totalRows: number): number {
  if (rowHeight <= 0 || totalRows <= 0) return -1;
  const absolute = scrollTop + y;
  if (absolute < 0) return -1;
  const row = Math.floor(absolute / rowHeight);
  return row < totalRows ? row : -1;
}

/**
 * Lane column under X, or null. `laneCount` is the row's lane_count; the
 * tolerance zone applies to the first and last lane columns too (so clicks
 * just off the outer edge of the graph still register).
 */
export function laneAt(
  x: number,
  laneCount: number,
  laneWidth: number,
  padding: number,
  tolerance = 6,
): number | null {
  if (laneCount <= 0 || laneWidth <= 0) return null;
  const rel = x - padding;
  if (rel < -tolerance) return null;
  const lane = Math.round(rel / laneWidth);
  if (lane <= 0) return 0; // rel >= -tolerance guarantees we're within tol of lane 0
  if (lane >= laneCount) {
    const lastX = padding + (laneCount - 1) * laneWidth;
    return x <= lastX + tolerance ? laneCount - 1 : null;
  }
  const laneCenter = padding + lane * laneWidth;
  return Math.abs(x - laneCenter) <= tolerance ? lane : null;
}

/** Combined row + lane resolution for a pointer position. */
export function hitTest(params: HitParams): HitResult {
  const total = params.rows.length;
  const row = rowAt(params.y, params.scrollTop, params.rowHeight, total);
  if (row < 0) return { row: -1, lane: null };
  const laneCount = params.rows[row]?.lane_count ?? 0;
  const lane = laneAt(
    params.x,
    laneCount,
    params.laneWidth,
    params.padding,
    params.tolerance ?? 6,
  );
  return { row, lane };
}
