/**
 * Split-view text selection model (pure; no DOM, no runes).
 *
 * Native selection cannot express "the left halves of rows 1–3": a selection
 * is one contiguous DOM range, and split rows interleave their halves
 * (row1-left, row1-right, row2-left, …), so a multi-row drag on one side
 * always spans the other side's copies in between. DiffViewer therefore
 * tracks split-mode text selection itself (anchor/focus points in row/char
 * coordinates) and paints the segments with the CSS Custom Highlight API;
 * this module owns the pure geometry:
 *
 *   - `selectionSegments` — the per-row character ranges between two points
 *     on the same side (rows whose half is absent — e.g. pair fillers — are
 *     skipped, and contribute nothing to the copied text);
 *   - `segmentText` — what Ctrl+C should put on the clipboard;
 *   - `wordSpanAt` — double-click word selection.
 *
 * A point past the end of its line (the drag can leave the text) clamps to
 * the line length, so selecting "beyond" the last character grabs the rest
 * of the row.
 */

/** A selection endpoint: global model row index + char offset in its half. */
export interface SelPoint {
  row: number;
  char: number;
}

/** Character range of one selected row half. */
export interface Segment {
  row: number;
  start: number;
  end: number;
}

/** Length of a row's half text on the selection's side (null = no half). */
export type HalfLength = (row: number) => number | null;

/**
 * Per-row segments between `a` and `b` (order-agnostic). Rows without a
 * half on the side are skipped; both endpoints clamp to their line lengths.
 */
export function selectionSegments(
  a: SelPoint,
  b: SelPoint,
  length: HalfLength,
): Segment[] {
  const clamp = (p: SelPoint): SelPoint => {
    const len = length(p.row);
    return { row: p.row, char: Math.max(0, Math.min(p.char, len ?? 0)) };
  };
  const from = clamp(a);
  const to = clamp(b);
  const lo =
    from.row < to.row || (from.row === to.row && from.char <= to.char)
      ? from
      : to;
  const hi = lo === from ? to : from;
  const segs: Segment[] = [];
  for (let row = lo.row; row <= hi.row; row++) {
    const len = length(row);
    if (len === null) continue; // filler half — nothing to select here
    const start = row === lo.row ? lo.char : 0;
    const end = row === hi.row ? hi.char : len;
    if (end > start) segs.push({ row, start, end });
  }
  return segs;
}

/**
 * The text a copy action should emit for `segs`: rows joined with "\n",
 * each sliced to its segment.
 */
export function segmentText(
  segs: readonly Segment[],
  text: (row: number) => string,
): string {
  return segs.map((s) => text(s.row).slice(s.start, s.end)).join("\n");
}

/** Word chars for double-click selection (letters, digits, underscore). */
const WORD = /[\p{L}\p{N}_]/u;

/**
 * Expands `char` within a line of `len` to the full word containing it
 * ([start, end), empty when the offset sits on whitespace/out of range).
 */
export function wordSpanAt(line: string, char: number): [number, number] {
  if (char < 0 || char >= line.length || !WORD.test(line[char]!)) return [0, 0];
  let start = char;
  while (start > 0 && WORD.test(line[start - 1]!)) start--;
  let end = char + 1;
  while (end < line.length && WORD.test(line[end]!)) end++;
  return [start, end];
}
