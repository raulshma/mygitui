/**
 * Lightweight list virtualizer for the diff viewer.
 *
 * Design: rows come as a per-row height array; consecutive rows of EQUAL
 * height are merged into uniform "buckets". A layout stores one prefix entry
 * per bucket (not per row), so:
 *
 *   - totalHeight is O(1),
 *   - "which rows are visible" is a binary search over buckets + pure
 *     arithmetic inside each bucket (rows never need per-row prefix sums),
 *   - rendering maps each visible bucket to ONE absolutely-positioned block
 *     whose rows flow naturally inside it.
 *
 * This is what keeps 10k-hunk (200k-row) diffs at 60fps: a scroll event costs
 * a binary search over ~40k buckets (≈16 comparisons) plus building the
 * handful of visible slices. No npm dependencies.
 */

/** A run of consecutive rows sharing one uniform row height. */
export interface HeightBucket {
  /** Index of this bucket's first row in the global row array. */
  firstRow: number;
  /** Number of rows in the bucket. */
  count: number;
  /** Uniform per-row height (px). */
  rowHeight: number;
  /** Absolute Y offset of the bucket's first row (px). */
  top: number;
}

export interface VLayout {
  readonly buckets: readonly HeightBucket[];
  /** Total row count across buckets. */
  readonly totalRows: number;
  /** Total content height in px. */
  readonly totalHeight: number;
}

/** A contiguous run of visible rows from one bucket, positioned as a block. */
export interface VisibleSlice {
  bucketIndex: number;
  /** Global index of the first visible row in the slice. */
  firstRow: number;
  /** Visible row count. */
  count: number;
  /** Absolute Y of the slice block (px). */
  top: number;
  /** Height of the slice block = count * bucket row height (px). */
  height: number;
}

const NO_BUCKETS: readonly HeightBucket[] = [];
const EMPTY_LAYOUT: VLayout = { buckets: NO_BUCKETS, totalRows: 0, totalHeight: 0 };

/**
 * Build a layout from per-row heights. Consecutive equal heights merge into
 * buckets; `rowHeights` may be any array-like (typed arrays included).
 */
export function buildLayout(rowHeights: ArrayLike<number>): VLayout {
  const n = rowHeights.length;
  if (n === 0) return EMPTY_LAYOUT;

  const buckets: HeightBucket[] = [];
  let firstRow = 0;
  let top = 0;
  let totalHeight = 0;
  let i = 0;
  while (i < n) {
    const h = rowHeights[i];
    let count = 1;
    // Fast scan for the uniform run. Diff row streams are dominated by long
    // line-runs (one bucket per hunk body), so this amortizes to ~one entry
    // per run, not per row.
    while (i + count < n && rowHeights[i + count] === h) count++;
    buckets.push({ firstRow, count, rowHeight: h, top });
    firstRow += count;
    top += count * h;
    totalHeight = top;
    i += count;
  }
  return { buckets, totalRows: n, totalHeight };
}

/** First bucket whose `top` is <= y; -1 when y precedes everything. */
function findBucketAtOrBefore(buckets: readonly HeightBucket[], y: number): number {
  let lo = 0;
  let hi = buckets.length - 1;
  let result = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (buckets[mid].top <= y) {
      result = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return result;
}

/**
 * Compute the visible slices for a scroll window. Pure: no DOM, no state.
 * `overscanPx` extends the window above and below so fast scrolls don't flash
 * blanks. Returns one slice per intersecting bucket, in document order.
 */
export function visibleSlices(
  layout: VLayout,
  scrollTop: number,
  viewportHeight: number,
  overscanPx: number,
): VisibleSlice[] {
  const buckets = layout.buckets;
  if (buckets.length === 0 || viewportHeight + overscanPx <= 0) return [];

  const yTop = Math.max(0, scrollTop - overscanPx);
  const yBottom = Math.max(0, scrollTop) + viewportHeight + overscanPx;

  let bi = findBucketAtOrBefore(buckets, yTop);
  // Only the very top of the first bucket can be scrolled past: when the
  // window starts inside bucket k, the run from k onward is what intersects.
  if (bi < 0) bi = 0;

  const slices: VisibleSlice[] = [];
  for (; bi < buckets.length; bi++) {
    const bucket = buckets[bi];
    if (bucket.top >= yBottom) break;
    const bucketBottom = bucket.top + bucket.count * bucket.rowHeight;
    if (bucketBottom <= yTop) continue;

    const startIdx = Math.max(0, Math.floor((yTop - bucket.top) / bucket.rowHeight));
    const endIdx = Math.min(bucket.count, Math.ceil((yBottom - bucket.top) / bucket.rowHeight));
    const count = endIdx - startIdx;
    if (count <= 0) continue;

    slices.push({
      bucketIndex: bi,
      firstRow: bucket.firstRow + startIdx,
      count,
      top: bucket.top + startIdx * bucket.rowHeight,
      height: count * bucket.rowHeight,
    });
  }
  return slices;
}

/** Absolute Y offset of a row (clamped to the content for out-of-range indices). */
export function rowTop(layout: VLayout, rowIndex: number): number {
  const buckets = layout.buckets;
  if (buckets.length === 0) return 0;
  const clamped = Math.max(0, Math.min(rowIndex, layout.totalRows - 1));

  let lo = 0;
  let hi = buckets.length - 1;
  while (lo < hi) {
    // Round UP: with `lo = mid` on match, a plain (lo+hi)>>1 mid can equal lo
    // (hi === lo+1) and stall the search forever.
    const mid = (lo + hi + 1) >> 1;
    if (buckets[mid].firstRow <= clamped) lo = mid;
    else hi = mid - 1;
  }
  const bucket = buckets[lo];
  return bucket.top + (clamped - bucket.firstRow) * bucket.rowHeight;
}
