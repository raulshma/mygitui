/**
 * Single source of truth for the diff's fixed pixel geometry: the row-height
 * grid the virtualizer lays out with AND the gutter widths DiffRow.svelte
 * renders. Both the DOM (inline styles) and the wrap arithmetic (wrapHeights)
 * derive from these constants, so the two can never drift again — before this
 * module the numbers lived as rem utilities in DiffRow's markup and were
 * restated by hand here.
 *
 * All values are px at the app's 16px root font size (the rem classes these
 * replaced were 16px-rooted too).
 */

/**
 * Fixed row heights (px). The Svelte side renders rows with these exact
 * inline heights — CSS never disagrees with the virtualizer.
 *
 * Line rows carry their height inline from `ROW_HEIGHTS.line` (DiffRow sets
 * `style:height` from `rowHeight()`; the 20px is also the text `leading`) —
 * the viewer's windowing math (`virtualizer.ts` via `rowHeights()`) uses the
 * same constant, so DOM and virtualizer agree exactly. The only 22px boxes in
 * DiffRow are the file-header's collapse button (inside the 40px header row,
 * unrelated to the line grid). If a future line-height tweak adds a border to
 * line rows, keep `line` as the FULL rendered row height.
 */
export const ROW_HEIGHTS = {
  fileHeader: 40,
  hunkHeader: 26,
  line: 20,
  binary: 56,
  image: 384,
} as const;

/**
 * Gutter cell geometry (px) — the non-text prefix of every diff line row.
 * DiffRow renders each cell with inline styles from these constants:
 *
 *   `.no`   line-number cell: `LINE_NUMBER_W` total, of which
 *           `LINE_NUMBER_PAD_R` is right padding (border-box — Tailwind's
 *           preflight makes min-width include padding, so the cell renders
 *           exactly `LINE_NUMBER_W` px wide: 44px content + 8px padding)
 *   `.sign` origin column: `SIGN_W` px
 *   `.txt`  code text: `TEXT_PAD_R` px right padding
 */

/** `.no` line-number cell, border-box (content + its right padding). */
export const LINE_NUMBER_W = 52;
/** Right padding inside the `.no` cell (subset of `LINE_NUMBER_W`). */
export const LINE_NUMBER_PAD_R = 8;
/** `.sign` origin column width. */
export const SIGN_W = 20;
/** `.txt` code cell right padding. */
export const TEXT_PAD_R = 16;

/**
 * Gutter + padding constants consumed by the wrap math (wrapHeights.ts) and
 * the horizontal scroll width (DiffViewer's `contentMinWidth`) — derived from
 * the same cell widths DiffRow renders, so the arithmetic is exact by
 * construction.
 *
 * History: these were hand-maintained as 96/156 with a comment claiming
 * DiffRow's gutters were content-box (`.no` = 3.25rem min-width + 0.5rem
 * padding ON TOP = 60px). Tailwind's border-box preflight actually renders
 * the `.no` cell at 52px total, so the old values over-subtracted 8px per
 * gutter; the DOM measurement feedback loop (wrapFeedback) silently absorbed
 * the difference. The derivation below is the rendered truth.
 */
export const GUTTER_PX = {
  /** Per split half: one line-number gutter + sign + text right padding. */
  splitSide: LINE_NUMBER_W + SIGN_W + TEXT_PAD_R,
  /** Unified row: two line-number gutters + sign + text right padding. */
  unified: 2 * LINE_NUMBER_W + SIGN_W + TEXT_PAD_R,
} as const;
