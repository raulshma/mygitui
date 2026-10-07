/**
 * Row model for the diff viewer: flattens `FileDiff[]` into the exact list of
 * renderable rows for the current mode, independent of the DOM. Pure and unit
 * tested; DiffViewer.svelte only consumes it.
 *
 * Row kinds (all with fixed, kind-derived heights so rows form uniform-height
 * buckets per hunk region for the virtualizer):
 *
 *   file-header  one per file: path, +/- counts, collapse toggle, states
 *   hunk-header  "@@ -a,b +c,d @@" — also an a11y focus stop
 *   line         one row per DiffLine (unified mode; any origin)
 *   context      split mode: unchanged line rendered once, spanning both halves
 *   pair         split mode: aligned (-line | +line) pair; shorter side padded
 *                with null (rendered as an empty filler cell)
 *   binary       "binary file" placeholder
 *   image        old/new image compare block
 */

import type { DiffHunk, DiffLine, FileDiff } from "$lib/ipc/types";

export type DiffMode = "split" | "unified";

/**
 * Fixed row heights (px). The Svelte side renders rows with these exact
 * inline heights — CSS never disagrees with the virtualizer.
 */
export const ROW_HEIGHTS = {
  fileHeader: 40,
  hunkHeader: 26,
  line: 20,
  binary: 56,
  image: 384,
} as const;

/**
 * Image URL resolver for image diffs (asset/convert protocol etc. is the
 * host's business). `old` selects the pre-image (old_path when renamed).
 * Resolve to `null` (or reject) to show the placeholder instead.
 */
export type LoadImageFn = (path: string, old: boolean) => Promise<string | null>;

export interface RowFileHeader {
  kind: "file-header";
  file: FileDiff;
  fileIndex: number;
  collapsed: boolean;
}
export interface RowHunkHeader {
  kind: "hunk-header";
  fileIndex: number;
  hunkIndex: number;
  oldStart: number;
  oldCount: number;
  newStart: number;
  newCount: number;
}
/** Unified mode, or a context line rendered on its own row. */
export interface RowLine {
  kind: "line";
  line: DiffLine;
}
/** Split mode: unchanged line spanning both halves. */
export interface RowContext {
  kind: "context";
  line: DiffLine;
}
/** Split mode: aligned deletion/addition; `null` pads the shorter run. */
export interface RowPair {
  kind: "pair";
  left: DiffLine | null;
  right: DiffLine | null;
}
export interface RowBinary {
  kind: "binary";
}
export interface RowImage {
  kind: "image";
  file: FileDiff;
}

export type DiffRow =
  | RowFileHeader
  | RowHunkHeader
  | RowLine
  | RowContext
  | RowPair
  | RowBinary
  | RowImage;

export interface RowModel {
  rows: DiffRow[];
  /** Longest line text in JS chars — drives the horizontal scroll width. */
  maxTextChars: number;
  /** Σ additions across all files (toolbar stat). */
  totalAdditions: number;
  /** Σ deletions across all files (toolbar stat). */
  totalDeletions: number;
}

/** Context origins: ' ' (plain context) and '=' (marker/context variant). */
export function isContextLine(line: DiffLine): boolean {
  return line.origin === " " || line.origin === "=";
}

/** Line counts for the "@@ -oldStart,oldCount +newStart,newCount @@" header. */
export function hunkCounts(hunk: DiffHunk): { oldCount: number; newCount: number } {
  let oldCount = 0;
  let newCount = 0;
  for (const line of hunk.lines) {
    if (line.old_no !== null) oldCount++;
    if (line.new_no !== null) newCount++;
  }
  return { oldCount, newCount };
}

/** Uniform bucket height of a row (px). */
export function rowHeight(row: DiffRow): number {
  switch (row.kind) {
    case "file-header":
      return ROW_HEIGHTS.fileHeader;
    case "hunk-header":
      return ROW_HEIGHTS.hunkHeader;
    case "line":
    case "context":
    case "pair":
      return ROW_HEIGHTS.line;
    case "binary":
      return ROW_HEIGHTS.binary;
    case "image":
      return ROW_HEIGHTS.image;
  }
}

/** Per-row heights array for `buildLayout`. */
export function rowHeights(rows: readonly DiffRow[]): number[] {
  const out = new Array<number>(rows.length);
  for (let i = 0; i < rows.length; i++) out[i] = rowHeight(rows[i]);
  return out;
}

/**
 * Split-mode alignment within one change block: consecutive non-context lines
 * form a block; k-th '-' pairs with k-th '+' of the block; the longer run
 * pads with null. (libgit2 emits '-'-runs before '+'-runs per block, but the
 * walker is order-agnostic so interleaved orders still pair sensibly.)
 */
function appendHunkBodySplit(lines: readonly DiffLine[], rows: DiffRow[]): void {
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (isContextLine(line)) {
      rows.push({ kind: "context", line });
      i++;
      continue;
    }
    // Gather one change block: consecutive '+'/'-' lines.
    const blockStart = i;
    while (i < lines.length && !isContextLine(lines[i])) i++;
    let dels = 0;
    let adds = 0;
    for (let k = blockStart; k < i; k++) {
      if (lines[k].origin === "-") dels++;
      else adds++;
    }
    const pairs = Math.max(dels, adds);
    let d = blockStart;
    let a = blockStart;
    for (let k = 0; k < pairs; k++) {
      while (d < i && lines[d].origin !== "-") d++;
      while (a < i && lines[a].origin !== "+") a++;
      rows.push({
        kind: "pair",
        left: d < i ? lines[d] : null,
        right: a < i ? lines[a] : null,
      });
      if (d < i) d++;
      if (a < i) a++;
    }
  }
}

/**
 * Build the row list for the current mode. `collapsedPaths` (keyed by
 * FileDiff.path) replaces a file's body with just its header.
 */
export function buildRowModel(
  files: readonly FileDiff[],
  mode: DiffMode,
  collapsedPaths: ReadonlySet<string>,
): RowModel {
  const rows: DiffRow[] = [];
  let maxTextChars = 0;
  let totalAdditions = 0;
  let totalDeletions = 0;

  for (let fileIndex = 0; fileIndex < files.length; fileIndex++) {
    const file = files[fileIndex];
    totalAdditions += file.additions;
    totalDeletions += file.deletions;
    for (const hunk of file.hunks) {
      for (const line of hunk.lines) {
        if (line.text.length > maxTextChars) maxTextChars = line.text.length;
      }
    }

    const collapsed = collapsedPaths.has(file.path);
    rows.push({ kind: "file-header", file, fileIndex, collapsed });
    if (collapsed) continue;

    if (file.is_image) {
      rows.push({ kind: "image", file });
      continue;
    }
    if (file.binary) {
      rows.push({ kind: "binary" });
      continue;
    }
    for (let hunkIndex = 0; hunkIndex < file.hunks.length; hunkIndex++) {
      const hunk = file.hunks[hunkIndex];
      const { oldCount, newCount } = hunkCounts(hunk);
      rows.push({
        kind: "hunk-header",
        fileIndex,
        hunkIndex,
        oldStart: hunk.old_start,
        oldCount,
        newStart: hunk.new_start,
        newCount,
      });
      if (mode === "unified") {
        for (const line of hunk.lines) rows.push({ kind: "line", line });
      } else {
        appendHunkBodySplit(hunk.lines, rows);
      }
    }
  }

  return { rows, maxTextChars, totalAdditions, totalDeletions };
}
