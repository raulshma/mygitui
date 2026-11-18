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
 *   single       split mode, one-sided file (added or deleted): the existing
 *                side spans the whole row (VS Code's inline layout), because
 *                the missing side would just be a blank half
 *   context      split mode: unchanged line rendered in both halves, each
 *                clipped at the divider
 *   pair         split mode: aligned (-line | +line) pair; shorter side padded
 *                with null (rendered as an empty filler cell)
 *   binary       "binary file" placeholder
 *   image        old/new image compare block
 */

import type { DiffHunk, DiffLine, FileDiff } from "$lib/ipc/types";
import { visualColumns } from "$lib/components/diff/textWidth";
import { ROW_HEIGHTS } from "$lib/components/diff/metrics";

// Back-compat re-export: the row heights moved to diff/metrics.ts (the shared
// geometry module), but rowModel remains the canonical import for the model's
// consumers (virtualizer tests, DiffViewer).
export { ROW_HEIGHTS };

export type DiffMode = "split" | "unified";

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
/**
 * Unified mode, or a context line rendered on its own row. Owner fields
 * (M12) let the row handle gutter clicks / selection without parallel
 * lookups: `lineIndex` is the position in the owning hunk's `lines`.
 */
export interface RowLine {
  kind: "line";
  line: DiffLine;
  fileIndex: number;
  hunkIndex: number;
  lineIndex: number;
}
/** Split mode: unchanged line spanning both halves. */
export interface RowContext {
  kind: "context";
  line: DiffLine;
  fileIndex: number;
  hunkIndex: number;
  lineIndex: number;
}
/**
 * Split mode, one-sided file: only one side exists, so the line spans the
 * whole row and shows that side's gutter (no empty filler half).
 */
export interface RowSingle {
  kind: "single";
  line: DiffLine;
  fileIndex: number;
  hunkIndex: number;
  lineIndex: number;
  /** Existing side — the only gutter this row renders. */
  side: "old" | "new";
}
/** Split mode: aligned deletion/addition; `null` pads the shorter run. */
export interface RowPair {
  kind: "pair";
  left: DiffLine | null;
  right: DiffLine | null;
  fileIndex: number;
  hunkIndex: number;
  /** Index of `left` in the hunk's lines (`null` when padded). */
  leftIndex: number | null;
  /** Index of `right` in the hunk's lines (`null` when padded). */
  rightIndex: number | null;
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
  | RowSingle
  | RowPair
  | RowBinary
  | RowImage;

export interface RowModel {
  rows: DiffRow[];
  /** Parallel to `rows`: owning file's index (syntax highlighting, M11). */
  fileIndexes: number[];
  /** True when any row is a two-column split row (drives scroll width). */
  hasSplitRows: boolean;
  /** Longest line text in JS chars (raw length stat). */
  maxTextChars: number;
  /**
   * Widest line's text by rendered columns (tabs expanded to their stops) —
   * drives the unified-mode horizontal scroll width (see textWidth.ts).
   */
  maxLineText: string;
  /** Σ additions across all files (toolbar stat). */
  totalAdditions: number;
  /** Σ deletions across all files (toolbar stat). */
  totalDeletions: number;
  /**
   * Files represented in the row list — smaller than the input's length
   * when the ignore-whitespace toggle dropped ws-only (zero-hunk) files,
   * so the toolbar's file count matches what is rendered.
   */
  fileCount: number;
}

/** Context origins: ' ' (plain context) and '=' (marker/context variant). */
export function isContextLine(line: DiffLine): boolean {
  return line.origin === " " || line.origin === "=";
}

/** Optional row-model filters driven by the diff toolbar toggles. */
export interface RowModelOptions {
  /**
   * "Ignore whitespace" (backend `DiffQuery.ignore_whitespace` parity,
   * FE-side): drop file entries whose change carried NO hunks — libgit2
   * strips ws-only deltas but keeps the file entry, which would otherwise
   * render as a bare header row. Binary/image entries always stay.
   */
  ignoreWhitespaceFiles?: boolean;
  /**
   * "Hide whitespace changes": pure row filter. Heuristic — within each
   * change block, the k-th '-' is paired with the k-th '+' (the split
   * view's alignment); a pair whose TRIMMED texts are equal hides both
   * lines. Context lines are never hidden, and unpaired '-'/'+' lines
   * (pure adds/deletes, even blank ones) always stay.
   */
  hideWhitespaceRows?: boolean;
}

/**
 * Marks the whitespace-only change pairs in one hunk's line list (see
 * {@link RowModelOptions.hideWhitespaceRows} for the heuristic). Returns a
 * parallel `boolean[]` — `true` hides the line at that index.
 */
export function whitespaceHiddenLines(lines: readonly DiffLine[]): boolean[] {
  const hidden = new Array<boolean>(lines.length).fill(false);
  let i = 0;
  while (i < lines.length) {
    if (isContextLine(lines[i])) {
      i++;
      continue;
    }
    const dels: number[] = [];
    const adds: number[] = [];
    let j = i;
    while (j < lines.length && !isContextLine(lines[j])) {
      (lines[j].origin === "-" ? dels : adds).push(j);
      j++;
    }
    const pairs = Math.min(dels.length, adds.length);
    for (let k = 0; k < pairs; k++) {
      const del = lines[dels[k]]!;
      const add = lines[adds[k]]!;
      if (del.text.trim() === add.text.trim()) {
        hidden[dels[k]!] = true;
        hidden[adds[k]!] = true;
      }
    }
    i = j;
  }
  return hidden;
}

/** Change kind shown on a file header badge. */
export type FileChangeKind = "added" | "deleted" | "modified" | "renamed";

/**
 * Best-effort change kind for a file header badge.
 *
 * `FileDiff` carries no status field, so infer from what the diff contains:
 * a differing `old_path` means a rename/copy; all hunks with zero old lines
 * means the file was added; all hunks with zero new lines means deleted.
 * Files without hunks (binary/empty) fall back to the +/- counters.
 *
 * Memoized by `FileDiff` identity: diff payloads are replaced wholesale when
 * a new diff arrives (never mutated in place), and the header re-renders on
 * every syntax-highlight tick — rescanning a 100k-line file there would be
 * the dominant cost.
 */
const changeKindCache = new WeakMap<FileDiff, FileChangeKind>();

export function fileChangeKind(file: FileDiff): FileChangeKind {
  const cached = changeKindCache.get(file);
  if (cached !== undefined) return cached;
  const kind = computeFileChangeKind(file);
  changeKindCache.set(file, kind);
  return kind;
}

function computeFileChangeKind(file: FileDiff): FileChangeKind {
  if (file.old_path !== null && file.old_path !== file.path) return "renamed";
  let oldLines = 0;
  let newLines = 0;
  for (const hunk of file.hunks) {
    const { oldCount, newCount } = hunkCounts(hunk);
    oldLines += oldCount;
    newLines += newCount;
  }
  if (oldLines === 0 && newLines > 0) return "added";
  if (newLines === 0 && oldLines > 0) return "deleted";
  if (file.hunks.length === 0) {
    if (file.additions > 0 && file.deletions === 0) return "added";
    if (file.deletions > 0 && file.additions === 0) return "deleted";
  }
  return "modified";
}

/** One-letter badge for a file change kind. */
export function fileKindLetter(kind: FileChangeKind): string {
  switch (kind) {
    case "added":
      return "A";
    case "deleted":
      return "D";
    case "renamed":
      return "R";
    default:
      return "M";
  }
}

/** Human-readable file change kind (file header tooltip). */
export function fileKindLabel(kind: FileChangeKind): string {
  switch (kind) {
    case "added":
      return "Added";
    case "deleted":
      return "Deleted";
    case "renamed":
      return "Renamed";
    default:
      return "Modified";
  }
}

/** Line counts for the "@@ -oldStart,oldCount +newStart,newCount @@" header. */
export function hunkCounts(hunk: { lines: readonly DiffLine[] }): {
  oldCount: number;
  newCount: number;
} {
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
    case "single":
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
 * `hidden` marks whitespace-pair lines to skip (see
 * {@link whitespaceHiddenLines}); `lineIndex`/`leftIndex`/`rightIndex` stay
 * POSITIONAL in the hunk's original line list (M12 selection targets).
 * `fileIndex`/`hunkIndex` are stamped onto every row (M12 selection owner).
 */
function appendHunkBodySplit(
  lines: readonly DiffLine[],
  hidden: readonly boolean[],
  rows: DiffRow[],
  fileIndex: number,
  hunkIndex: number,
): void {
  let i = 0;
  while (i < lines.length) {
    if (hidden[i]) {
      i++;
      continue;
    }
    const line = lines[i];
    if (isContextLine(line)) {
      rows.push({ kind: "context", line, fileIndex, hunkIndex, lineIndex: i });
      i++;
      continue;
    }
    // Gather one change block: consecutive '+'/'-' lines (hidden lines are
    // consumed silently and excluded from the pairing).
    const blockStart = i;
    let j = i;
    let dels = 0;
    let adds = 0;
    while (j < lines.length && !isContextLine(lines[j])) {
      if (!hidden[j]) {
        if (lines[j].origin === "-") dels++;
        else adds++;
      }
      j++;
    }
    i = j;
    const pairs = Math.max(dels, adds);
    let d = blockStart;
    let a = blockStart;
    for (let k = 0; k < pairs; k++) {
      while (d < j && (hidden[d] || lines[d].origin !== "-")) d++;
      while (a < j && (hidden[a] || lines[a].origin !== "+")) a++;
      rows.push({
        kind: "pair",
        left: d < j ? lines[d] : null,
        right: a < j ? lines[a] : null,
        fileIndex,
        hunkIndex,
        leftIndex: d < j ? d : null,
        rightIndex: a < j ? a : null,
      });
      if (d < j) d++;
      if (a < j) a++;
    }
  }
}

/**
 * One-sided file body: the file has no content on the other side, so every
 * line renders full-width with only its own gutter (VS Code's inline diff for
 * added/deleted files) rather than as a half plus an empty filler half.
 */
function appendHunkBodySingle(
  lines: readonly DiffLine[],
  rows: DiffRow[],
  fileIndex: number,
  hunkIndex: number,
  side: "old" | "new",
): void {
  for (let lineIndex = 0; lineIndex < lines.length; lineIndex++) {
    rows.push({
      kind: "single",
      line: lines[lineIndex],
      fileIndex,
      hunkIndex,
      lineIndex,
      side,
    });
  }
}

/**
 * Build the row list for the current mode. `collapsedPaths` (keyed by
 * FileDiff.path) replaces a file's body with just its header. `options`
 * carries the whitespace toggles (see {@link RowModelOptions}); the hidden
 * markers stay positional in each hunk's line list, so selection/line
 * targets (M12) keep addressing the original `hunk.lines`.
 */
export function buildRowModel(
  files: readonly FileDiff[],
  mode: DiffMode,
  collapsedPaths: ReadonlySet<string>,
  options: RowModelOptions = {},
): RowModel {
  const ignoreWsFiles = options.ignoreWhitespaceFiles === true;
  const hideWsRows = options.hideWhitespaceRows === true;
  const rows: DiffRow[] = [];
  /** Parallel to `rows`: owning file's index (syntax highlighting, M11). */
  const fileIndexes: number[] = [];
  let hasSplitRows = false;
  let maxTextChars = 0;
  let maxLineCols = 0;
  let maxLineText = "";
  let totalAdditions = 0;
  let totalDeletions = 0;
  let fileCount = 0;

  for (let fileIndex = 0; fileIndex < files.length; fileIndex++) {
    const file = files[fileIndex];
    // Ignore-whitespace: ws-only deltas arrive as zero-hunk file entries
    // (binary/image placeholders always render). Dropped files keep their
    // original `fileIndex` — the indexes stay aligned with the input list.
    if (
      ignoreWsFiles &&
      file.hunks.length === 0 &&
      !file.binary &&
      !file.is_image
    ) {
      continue;
    }
    fileCount++;
    // Per-hunk hidden markers (all-false unless the hide filter is on).
    const hiddenPerHunk = hideWsRows
      ? file.hunks.map((hunk) => whitespaceHiddenLines(hunk.lines))
      : null;
    totalAdditions += file.additions;
    totalDeletions += file.deletions;
    if (hiddenPerHunk !== null) {
      // Keep the +/- stats consistent with what is actually rendered:
      // subtract the hidden pair members from the backend's file counters.
      for (let h = 0; h < file.hunks.length; h++) {
        const hidden = hiddenPerHunk[h];
        const lines = file.hunks[h].lines;
        for (let i = 0; i < hidden.length; i++) {
          if (!hidden[i]) continue;
          if (lines[i].origin === "-") totalDeletions--;
          else totalAdditions--;
        }
      }
    }
    for (let hunkIndex = 0; hunkIndex < file.hunks.length; hunkIndex++) {
      const lines = file.hunks[hunkIndex].lines;
      const hidden = hiddenPerHunk?.[hunkIndex];
      for (let i = 0; i < lines.length; i++) {
        if (hidden?.[i]) continue;
        const text = lines[i].text;
        if (text.length > maxTextChars) maxTextChars = text.length;
        const cols = visualColumns(text);
        if (cols > maxLineCols) {
          maxLineCols = cols;
          maxLineText = text;
        }
      }
    }

    const collapsed = collapsedPaths.has(file.path);
    rows.push({ kind: "file-header", file, fileIndex, collapsed });
    fileIndexes.push(fileIndex);
    if (collapsed) continue;

    if (file.is_image) {
      rows.push({ kind: "image", file });
      fileIndexes.push(fileIndex);
      continue;
    }
    if (file.binary) {
      rows.push({ kind: "binary" });
      fileIndexes.push(fileIndex);
      continue;
    }
    // Added/deleted files have no content on one side: render them inline
    // (full-width gutter + text) instead of a half paired with a blank filler.
    const change = mode === "split" ? fileChangeKind(file) : null;
    const singleSide =
      change === "added" ? "new" : change === "deleted" ? "old" : null;
    for (let hunkIndex = 0; hunkIndex < file.hunks.length; hunkIndex++) {
      const hunk = file.hunks[hunkIndex];
      const hidden = hiddenPerHunk?.[hunkIndex];
      // Hunk-header counts stay consistent with the visible rows.
      const visibleLines = hidden
        ? hunk.lines.filter((_, i) => !hidden[i])
        : hunk.lines;
      const { oldCount, newCount } = hunkCounts({ lines: visibleLines });
      rows.push({
        kind: "hunk-header",
        fileIndex,
        hunkIndex,
        oldStart: hunk.old_start,
        oldCount,
        newStart: hunk.new_start,
        newCount,
      });
      fileIndexes.push(fileIndex);
      if (mode === "unified") {
        for (let lineIndex = 0; lineIndex < hunk.lines.length; lineIndex++) {
          if (hidden?.[lineIndex]) continue;
          rows.push({
            kind: "line",
            line: hunk.lines[lineIndex],
            fileIndex,
            hunkIndex,
            lineIndex,
          });
          fileIndexes.push(fileIndex);
        }
      } else if (singleSide !== null) {
        const before = rows.length;
        appendHunkBodySingle(hunk.lines, rows, fileIndex, hunkIndex, singleSide);
        for (let i = before; i < rows.length; i++) fileIndexes.push(fileIndex);
      } else {
        const before = rows.length;
        appendHunkBodySplit(
          hunk.lines,
          hidden ?? EMPTY_HIDDEN,
          rows,
          fileIndex,
          hunkIndex,
        );
        if (rows.length > before) hasSplitRows = true;
        for (let i = before; i < rows.length; i++) fileIndexes.push(fileIndex);
      }
    }
  }

  return {
    rows,
    fileIndexes,
    hasSplitRows,
    maxTextChars,
    maxLineText,
    totalAdditions,
    totalDeletions,
    fileCount,
  };
}

/** Shared all-false hidden marker for the no-filter path. */
const EMPTY_HIDDEN: readonly boolean[] = [];

// ---------------------------------------------------------------------------
// Copy actions (unified patch synthesis) — pure text, unit tested.
// ---------------------------------------------------------------------------

/** Prefixes hunk lines with their patch sign (unknown origins read as context). */
function patchSign(origin: DiffLine["origin"]): string {
  return origin === "-" || origin === "+" ? origin : " ";
}

/**
 * Unified patch text for one file: `diff --git` / `---` / `+++` headers
 * (a `/dev/null` side for pure adds/deletes, `a/<old_path>` for renames)
 * followed by every hunk's `@@` header and signed lines. Best-effort
 * synthesis from the `FileDiff` payload — good enough to paste/re-apply
 * context, not byte-identical to `git diff` output (no index lines, no
 * rename bipcharts).
 */
export function filePatchText(file: FileDiff): string {
  const parts = [`diff --git a/${file.old_path ?? file.path} b/${file.path}`];
  parts.push(
    fileChangeKind(file) === "added"
      ? "--- /dev/null"
      : `--- a/${file.old_path ?? file.path}`,
  );
  parts.push(
    fileChangeKind(file) === "deleted"
      ? "+++ /dev/null"
      : `+++ b/${file.path}`,
  );
  for (let hunkIndex = 0; hunkIndex < file.hunks.length; hunkIndex++) {
    const header = hunkHeaderText(file, hunkIndex);
    if (header === null) continue; // unreachable: the hunk exists
    parts.push(header);
    for (const line of file.hunks[hunkIndex].lines) {
      parts.push(patchSign(line.origin) + line.text);
    }
  }
  return parts.join("\n");
}

/** The "@@ -a,b +c,d @@" line for one of `file`'s hunks (null = no hunk). */
function hunkHeaderText(file: FileDiff, hunkIndex: number): string | null {
  const hunk = file.hunks[hunkIndex];
  if (!hunk) return null;
  const { oldCount, newCount } = hunkCounts(hunk);
  return `@@ -${hunk.old_start},${oldCount} +${hunk.new_start},${newCount} @@`;
}

/**
 * One hunk's patch text (`@@` header + signed lines), without the file
 * headers. Null when the file has no such hunk.
 */
export function hunkPatchText(file: FileDiff, hunkIndex: number): string | null {
  const header = hunkHeaderText(file, hunkIndex);
  if (header === null) return null;
  const parts = [header];
  for (const line of file.hunks[hunkIndex].lines) {
    parts.push(patchSign(line.origin) + line.text);
  }
  return parts.join("\n");
}

// ---------------------------------------------------------------------------
// Find-in-diff — pure search over the flattened row model.
// ---------------------------------------------------------------------------

/** One find hit: the row (and split half) plus the matched char range. */
export interface FindMatch {
  rowIndex: number;
  /** Split-row half carrying the hit; null for whole-row text rows. */
  side: "left" | "right" | null;
  /** Matched range in the row's text (JS string indices). */
  start: number;
  end: number;
}

/**
 * Render-side find highlight handed to DiffRow: the matched char range of
 * one row half plus whether it is the currently navigated match.
 */
export interface FindSpan {
  side: "left" | "right" | null;
  start: number;
  end: number;
  active: boolean;
}

/** The searchable line texts of one row (headers/binary/image have none). */
export function rowSearchTexts(row: DiffRow): Array<{
  side: "left" | "right" | null;
  text: string;
}> {
  switch (row.kind) {
    case "line":
    case "single":
    case "context":
      return [{ side: null, text: row.line.text }];
    case "pair": {
      const out: Array<{ side: "left" | "right" | null; text: string }> = [];
      if (row.left !== null) out.push({ side: "left", text: row.left.text });
      if (row.right !== null) out.push({ side: "right", text: row.right.text });
      return out;
    }
    default:
      return [];
  }
}

/**
 * Case-insensitive SUBSTRING matches of `query` over the row model (the
 * documented heuristic: plain indexOf, no regex; matching folds case with
 * toLowerCase — fast and Unicode-simple; non-overlapping per text). Empty
 * queries match nothing. Header/hunk-header/binary/image rows carry no
 * searchable line text and are skipped.
 */
export function findMatches(
  rows: readonly DiffRow[],
  query: string,
): FindMatch[] {
  const needle = query.toLowerCase();
  if (needle === "") return [];
  const out: FindMatch[] = [];
  for (let rowIndex = 0; rowIndex < rows.length; rowIndex++) {
    for (const { side, text } of rowSearchTexts(rows[rowIndex])) {
      const haystack = text.toLowerCase();
      let from = 0;
      for (;;) {
        const at = haystack.indexOf(needle, from);
        if (at === -1) break;
        out.push({ rowIndex, side, start: at, end: at + needle.length });
        from = at + needle.length;
      }
    }
  }
  return out;
}

/**
 * The rendered line text of one row (context-menu "copy line"). `side`
 * picks the half of a split pair row; null takes the row's single text
 * (or, for an unpaired pair row, whichever half exists).
 */
export function rowLineText(
  row: DiffRow,
  side: "left" | "right" | null,
): string | null {
  switch (row.kind) {
    case "line":
    case "single":
    case "context":
      return row.line.text;
    case "pair": {
      if (side === "left") return row.left?.text ?? null;
      if (side === "right") return row.right?.text ?? null;
      return row.left?.text ?? row.right?.text ?? null;
    }
    default:
      return null;
  }
}
