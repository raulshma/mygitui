/**
 * Line-granular selection model for the diff viewer (M12, lane C).
 *
 * The unit of selection is the NEW-side line number — exactly what the
 * backend's `{lines: …}` StageTarget consumes (inclusive ranges over the
 * new-side numbers; see src-tauri diffcore `mark_kept`):
 *
 *   - context lines are always kept by the backend, so selecting them is
 *     harmless (they only ever ride along inside a range);
 *   - '+' lines are staged iff their new-side number is selected;
 *   - '-' runs are staged iff any paired '+' run line is selected (pure
 *     deletion runs: the nearest new-side number around them).
 *
 * The UI mirrors those semantics so what the user sees highlighted is what
 * the backend will stage. All functions here are PURE (no runes, no DOM) —
 * DiffViewer.svelte owns the single active selection as `$state` outside
 * the per-row render path; rows only receive derived booleans.
 */

import type { DiffLine, LineRange } from "$lib/ipc/types";

/** The one active selection in a viewer (keyed by file + hunk). */
export interface LineSelection {
  /** Index into `FileDiff[]` of the owning file. */
  fileIndex: number;
  /** Hunk index within that file's diff (same index hunk staging uses). */
  hunkIndex: number;
  /** Sorted unique NEW-side line numbers currently selected. */
  lines: number[];
  /** New-side number shift-click extends from. */
  anchor: number;
}

/** Stable key for a selection target (file index + hunk index). */
export function selectionKey(fileIndex: number, hunkIndex: number): string {
  return `${fileIndex}:${hunkIndex}`;
}

/**
 * Starts a fresh one-line selection anchored at new-side number `anchor`.
 * Returns `null` for anything that cannot be a new-side number
 * (non-integer / < 1) — the caller treats that as "not selectable".
 */
export function beginSelection(
  fileIndex: number,
  hunkIndex: number,
  anchor: number,
): LineSelection | null {
  if (!Number.isInteger(anchor) || anchor < 1) return null;
  return { fileIndex, hunkIndex, lines: [anchor], anchor };
}

/** Every NEW-side number present in a hunk, ascending (context included). */
export function hunkNewSides(lines: readonly DiffLine[]): number[] {
  const out: number[] = [];
  for (const line of lines) {
    if (line.new_no !== null) out.push(line.new_no);
  }
  return out.sort((a, b) => a - b);
}

/**
 * Shift-click extension: selects the whole span between the anchor and
 * `target`, CLAMPED to the hunk (only numbers that exist as new-side
 * numbers of `hunkLines` are kept). Always returns a fresh object; the
 * anchor stays put so repeated shift-clicks re-extend from the origin.
 */
export function extendSelection(
  sel: LineSelection,
  target: number,
  hunkLines: readonly DiffLine[],
): LineSelection {
  const allowed = new Set(hunkNewSides(hunkLines));
  const lo = Math.min(sel.anchor, target);
  const hi = Math.max(sel.anchor, target);
  const lines: number[] = [];
  for (let n = lo; n <= hi; n++) {
    if (allowed.has(n)) lines.push(n);
  }
  // Degenerate span (target outside the hunk's numbers): keep the anchor.
  if (lines.length === 0) return { ...sel };
  return { ...sel, lines };
}

/** Clears a selection (identity helper; the viewer stores `null`). */
export function clearSelection(): null {
  return null;
}

/**
 * Merges sorted numbers into inclusive `LineRange`s: consecutive numbers
 * collapse into one range; gaps start new ones.
 */
export function mergeRanges(sorted: readonly number[]): LineRange[] {
  const ranges: LineRange[] = [];
  for (const n of [...sorted].sort((a, b) => a - b)) {
    const last = ranges.at(-1);
    if (last && n === last.end + 1) last.end = n;
    else ranges.push({ start: n, end: n });
  }
  return ranges;
}

/** The `{lines: …}` StageTarget ranges for a selection. */
export function selectionRanges(sel: LineSelection): LineRange[] {
  return mergeRanges(sel.lines);
}

/** Membership set for O(1) per-row checks. */
export function selectedSet(sel: LineSelection | null): Set<number> {
  return new Set(sel?.lines ?? []);
}

/**
 * The new-side number a gutter click on `hunkLines[index]` selects:
 * the line's own new-side number, or — for old-side-only ('-') lines —
 * the nearest one FORWARD, else BACKWARD (mirroring the backend's
 * pure-deletion-run anchoring). `null` when the hunk has no new-side
 * number to anchor to at all.
 */
export function selectableAnchor(
  hunkLines: readonly DiffLine[],
  index: number,
): number | null {
  const line = hunkLines[index];
  if (!line) return null;
  if (line.new_no !== null) return line.new_no;
  for (let i = index + 1; i < hunkLines.length; i++) {
    const no = hunkLines[i].new_no;
    if (no !== null) return no;
  }
  for (let i = index - 1; i >= 0; i--) {
    const no = hunkLines[i].new_no;
    if (no !== null) return no;
  }
  return null;
}

/**
 * Whether the row at `index` renders as selected — exactly the lines the
 * backend will keep for the selected ranges (`mark_kept` semantics):
 * '+' / context by their own new-side number; '-' via its run's paired
 * '+' lines, else the nearest new-side number (forward first, then back).
 */
export function isLineSelected(
  hunkLines: readonly DiffLine[],
  index: number,
  selected: ReadonlySet<number>,
): boolean {
  const line = hunkLines[index];
  if (!line) return false;
  if (line.origin !== "-") {
    return line.new_no !== null && selected.has(line.new_no);
  }
  // Locate the '-' run containing `index`.
  const n = hunkLines.length;
  let start = index;
  while (start > 0 && hunkLines[start - 1].origin === "-") start--;
  let end = index;
  while (end < n && hunkLines[end].origin === "-") end++;
  // A '+' run right after: kept iff ANY of its lines is selected.
  if (end < n && hunkLines[end].origin === "+") {
    for (let i = end; i < n && hunkLines[i].origin === "+"; i++) {
      const no = hunkLines[i].new_no;
      if (no !== null && selected.has(no)) return true;
    }
    return false;
  }
  // Pure deletion run: nearest new-side number, forward first.
  for (let i = end; i < n; i++) {
    const no = hunkLines[i].new_no;
    if (no !== null) return selected.has(no);
  }
  for (let i = start - 1; i >= 0; i--) {
    const no = hunkLines[i].new_no;
    if (no !== null) return selected.has(no);
  }
  return false;
}
