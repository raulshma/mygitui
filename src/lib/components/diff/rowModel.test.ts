/**
 * Unit tests for the diff row model (diff/rowModel.ts): unified ordering,
 * split-mode alignment pairs, context spanning, collapse, binary/image
 * placeholders and stats.
 */

import { describe, expect, it } from "vitest";
import type { DiffHunk, DiffLine, FileDiff } from "$lib/ipc/types";
import {
  buildRowModel,
  hunkCounts,
  isContextLine,
  rowHeight,
  rowHeights,
  ROW_HEIGHTS,
} from "$lib/components/diff/rowModel";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function line(
  origin: DiffLine["origin"],
  text: string,
  oldNo: number | null,
  newNo: number | null,
  highlights: [number, number][] = [],
): DiffLine {
  return { old_no: oldNo, new_no: newNo, origin, text, highlights };
}

function hunk(lines: DiffLine[], oldStart = 1, newStart = 1): DiffHunk {
  return { old_start: oldStart, new_start: newStart, lines };
}

function file(overrides: Partial<FileDiff> = {}): FileDiff {
  return {
    path: "src/a.txt",
    old_path: null,
    binary: false,
    is_image: false,
    additions: 0,
    deletions: 0,
    hunks: [],
    ...overrides,
  };
}

/** ctx, -old, -old2, +new, +new2, ctx — the canonical small hunk. */
function changeHunk(): DiffHunk {
  return hunk([
    line(" ", "keep", 1, 1),
    line("-", "old", 2, null),
    line("-", "old2", 3, null),
    line("+", "new", null, 2),
    line("+", "new2", null, 3),
    line(" ", "tail", 4, 4),
  ]);
}

const NO_COLLAPSE: ReadonlySet<string> = new Set();

// ---------------------------------------------------------------------------
// isContextLine / hunkCounts
// ---------------------------------------------------------------------------

describe("isContextLine", () => {
  it("treats ' ' and '=' as context, '+'/'-' as changes", () => {
    expect(isContextLine(line(" ", "x", 1, 1))).toBe(true);
    expect(isContextLine(line("=", "x", 1, 1))).toBe(true);
    expect(isContextLine(line("+", "x", null, 1))).toBe(false);
    expect(isContextLine(line("-", "x", 1, null))).toBe(false);
  });
});

describe("hunkCounts", () => {
  it("counts context+deletions old-side, context+additions new-side", () => {
    expect(hunkCounts(changeHunk())).toEqual({ oldCount: 4, newCount: 4 });
    expect(hunkCounts(hunk([line("+", "a", null, 1)]))).toEqual({ oldCount: 0, newCount: 1 });
    expect(hunkCounts(hunk([]))).toEqual({ oldCount: 0, newCount: 0 });
  });
});

// ---------------------------------------------------------------------------
// Unified mode
// ---------------------------------------------------------------------------

describe("buildRowModel (unified)", () => {
  it("emits header, hunk-header, then every line in order", () => {
    const model = buildRowModel([file({ hunks: [changeHunk()] })], "unified", NO_COLLAPSE);
    expect(model.rows.map((r) => r.kind)).toEqual([
      "file-header",
      "hunk-header",
      "line",
      "line",
      "line",
      "line",
      "line",
      "line",
    ]);
    const texts = model.rows
      .filter((r) => r.kind === "line")
      .map((r) => (r as { line: DiffLine }).line.text);
    expect(texts).toEqual(["keep", "old", "old2", "new", "new2", "tail"]);
  });

  it("carries old/new starts and counts on the hunk header", () => {
    const model = buildRowModel(
      [file({ hunks: [hunk(changeHunk().lines, 41, 57)] })],
      "unified",
      NO_COLLAPSE,
    );
    const header = model.rows[1];
    expect(header).toMatchObject({ kind: "hunk-header", oldStart: 41, newStart: 57, oldCount: 4, newCount: 4 });
  });
});

// ---------------------------------------------------------------------------
// Split mode
// ---------------------------------------------------------------------------

describe("buildRowModel (split)", () => {
  it("pairs deletions with additions row-by-row and spans context", () => {
    const model = buildRowModel([file({ hunks: [changeHunk()] })], "split", NO_COLLAPSE);
    expect(model.rows.map((r) => r.kind)).toEqual([
      "file-header",
      "hunk-header",
      "context", // keep
      "pair", // old  | new
      "pair", // old2 | new2
      "context", // tail
    ]);

    const pairs = model.rows.filter((r) => r.kind === "pair") as Array<{
      left: DiffLine | null;
      right: DiffLine | null;
    }>;
    expect(pairs[0]).toMatchObject({ left: { text: "old" }, right: { text: "new" } });
    expect(pairs[1]).toMatchObject({ left: { text: "old2" }, right: { text: "new2" } });
  });

  it("pads the shorter run with null (more deletions)", () => {
    const h = hunk([
      line("-", "d1", 1, null),
      line("-", "d2", 2, null),
      line("-", "d3", 3, null),
      line("+", "a1", null, 1),
    ]);
    const model = buildRowModel([file({ hunks: [h] })], "split", NO_COLLAPSE);
    const pairs = model.rows.filter((r) => r.kind === "pair") as Array<{
      left: DiffLine | null;
      right: DiffLine | null;
    }>;
    expect(pairs).toHaveLength(3);
    expect(pairs[0]).toMatchObject({ left: { text: "d1" }, right: { text: "a1" } });
    expect(pairs[1]).toMatchObject({ left: { text: "d2" }, right: null });
    expect(pairs[2]).toMatchObject({ left: { text: "d3" }, right: null });
  });

  it("pads the shorter run with null (more additions, none deleted)", () => {
    const h = hunk([
      line("+", "a1", null, 1),
      line("+", "a2", null, 2),
    ]);
    const model = buildRowModel([file({ hunks: [h] })], "split", NO_COLLAPSE);
    const pairs = model.rows.filter((r) => r.kind === "pair") as Array<{
      left: DiffLine | null;
      right: DiffLine | null;
    }>;
    expect(pairs).toHaveLength(2);
    expect(pairs[0]).toMatchObject({ left: null, right: { text: "a1" } });
    expect(pairs[1]).toMatchObject({ left: null, right: { text: "a2" } });
  });

  it("pairs interleaved change blocks independently", () => {
    const h = hunk([
      line(" ", "c1", 1, 1),
      line("-", "d1", 2, null),
      line("+", "a1", null, 2),
      line(" ", "c2", 3, 3),
      line("-", "d2", 4, null),
      line("+", "a2", null, 4),
      line("+", "a3", null, 5),
    ]);
    const model = buildRowModel([file({ hunks: [h] })], "split", NO_COLLAPSE);
    expect(model.rows.map((r) => r.kind)).toEqual([
      "file-header",
      "hunk-header",
      "context",
      "pair",
      "context",
      "pair",
      "pair",
    ]);
    const pairs = model.rows.filter((r) => r.kind === "pair") as Array<{
      left: DiffLine | null;
      right: DiffLine | null;
    }>;
    expect(pairs[1]).toMatchObject({ left: { text: "d2" }, right: { text: "a2" } });
    expect(pairs[2]).toMatchObject({ left: null, right: { text: "a3" } });
  });

  it("handles an order-agnostic interleaved block (- + -)", () => {
    const h = hunk([
      line("-", "d1", 1, null),
      line("+", "a1", null, 1),
      line("-", "d2", 2, null),
    ]);
    const model = buildRowModel([file({ hunks: [h] })], "split", NO_COLLAPSE);
    const pairs = model.rows.filter((r) => r.kind === "pair") as Array<{
      left: DiffLine | null;
      right: DiffLine | null;
    }>;
    expect(pairs).toHaveLength(2);
    expect(pairs[0]).toMatchObject({ left: { text: "d1" }, right: { text: "a1" } });
    expect(pairs[1]).toMatchObject({ left: { text: "d2" }, right: null });
  });

  it("keeps multiple hunks per file separate", () => {
    const f = file({ hunks: [hunk([line("+", "a", null, 1)], 1, 1), hunk([line("-", "d", 9, null)], 9, 9)] });
    const model = buildRowModel([f], "split", NO_COLLAPSE);
    expect(model.rows.map((r) => r.kind)).toEqual([
      "file-header",
      "hunk-header",
      "pair",
      "hunk-header",
      "pair",
    ]);
    expect(model.rows[1]).toMatchObject({ oldStart: 1, newStart: 1 });
    expect(model.rows[3]).toMatchObject({ oldStart: 9, newStart: 9 });
  });
});

// ---------------------------------------------------------------------------
// File sections, collapse, placeholders
// ---------------------------------------------------------------------------

describe("file sections and states", () => {
  it("emits one header per file, preserving order", () => {
    const files = [
      file({ path: "a.txt", hunks: [hunk([line("+", "x", null, 1)])] }),
      file({ path: "b.txt", hunks: [hunk([line("-", "y", 1, null)])] }),
    ];
    const model = buildRowModel(files, "unified", NO_COLLAPSE);
    const headers = model.rows.filter((r) => r.kind === "file-header");
    expect(headers.map((h) => (h as { file: FileDiff }).file.path)).toEqual(["a.txt", "b.txt"]);
    expect(headers[0]).toMatchObject({ fileIndex: 0, collapsed: false });
  });

  it("collapses a file to just its header", () => {
    const files = [
      file({ path: "a.txt", hunks: [hunk([line("+", "x", null, 1)])] }),
      file({ path: "b.txt", hunks: [hunk([line("+", "y", null, 1)])] }),
    ];
    const model = buildRowModel(files, "split", new Set(["a.txt"]));
    expect(model.rows.map((r) => r.kind)).toEqual([
      "file-header", // a.txt, collapsed
      "file-header", // b.txt
      "hunk-header",
      "pair",
    ]);
    expect(model.rows[0]).toMatchObject({ collapsed: true });
  });

  it("renders a binary placeholder for binary files and skips hunks", () => {
    const model = buildRowModel(
      [file({ path: "blob.bin", binary: true, hunks: [hunk([line("+", "??", null, 1)])] })],
      "split",
      NO_COLLAPSE,
    );
    expect(model.rows.map((r) => r.kind)).toEqual(["file-header", "binary"]);
  });

  it("prefers the image compare over the binary placeholder", () => {
    const model = buildRowModel(
      [file({ path: "img.png", binary: true, is_image: true })],
      "split",
      NO_COLLAPSE,
    );
    expect(model.rows.map((r) => r.kind)).toEqual(["file-header", "image"]);
  });

  it("emits just a header for a non-binary file with no hunks", () => {
    const model = buildRowModel([file({ path: "empty.txt" })], "unified", NO_COLLAPSE);
    expect(model.rows.map((r) => r.kind)).toEqual(["file-header"]);
  });

  it("handles an empty file list", () => {
    expect(buildRowModel([], "split", NO_COLLAPSE).rows).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// Stats + heights
// ---------------------------------------------------------------------------

describe("stats and heights", () => {
  it("sums additions/deletions and finds the longest line", () => {
    const files = [
      file({ additions: 3, deletions: 1, hunks: [hunk([line("+", "short", null, 1)])] }),
      file({ additions: 2, deletions: 5, hunks: [hunk([line("+", "a-very-long-line-here", null, 1)])] }),
    ];
    const model = buildRowModel(files, "unified", NO_COLLAPSE);
    expect(model.totalAdditions).toBe(5);
    expect(model.totalDeletions).toBe(6);
    expect(model.maxTextChars).toBe("a-very-long-line-here".length);
    expect(model.maxLineText).toBe("a-very-long-line-here");
  });

  it("picks the widest line by rendered columns, not raw chars (tabs)", () => {
    // "\tX" renders 9 columns; the 6-char plain line only 6.
    const files = [
      file({ hunks: [hunk([line("+", "\tX", null, 1), line("-", "abcdef", 1, null)])] }),
    ];
    const model = buildRowModel(files, "unified", NO_COLLAPSE);
    expect(model.maxTextChars).toBe("abcdef".length);
    expect(model.maxLineText).toBe("\tX");
  });

  it("leaves maxLineText empty when no file has lines", () => {
    const model = buildRowModel([file({ path: "empty.txt" })], "unified", NO_COLLAPSE);
    expect(model.maxLineText).toBe("");
  });

  it("maps every row kind to a uniform bucket height", () => {
    const model = buildRowModel(
      [
        file({
          path: "x",
          binary: true,
          is_image: true,
          hunks: [hunk([line("+", "t", null, 1)])],
        }),
        file({ path: "y", hunks: [hunk(changeHunk().lines)] }),
      ],
      "split",
      NO_COLLAPSE,
    );
    for (const row of model.rows) {
      const expected = {
        "file-header": ROW_HEIGHTS.fileHeader,
        "hunk-header": ROW_HEIGHTS.hunkHeader,
        line: ROW_HEIGHTS.line,
        context: ROW_HEIGHTS.line,
        pair: ROW_HEIGHTS.line,
        binary: ROW_HEIGHTS.binary,
        image: ROW_HEIGHTS.image,
      }[row.kind];
      expect(rowHeight(row)).toBe(expected);
    }
  });

  it("rowHeights returns one height per row", () => {
    const model = buildRowModel([file({ hunks: [changeHunk()] })], "split", NO_COLLAPSE);
    const heights = rowHeights(model.rows);
    expect(heights).toHaveLength(model.rows.length);
    expect(new Set(heights).size).toBeLessThanOrEqual(3); // header/hunk/line
  });
});
