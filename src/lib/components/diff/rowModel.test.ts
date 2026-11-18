/**
 * Unit tests for the diff row model (diff/rowModel.ts): unified ordering,
 * split-mode alignment pairs, context spanning, collapse, binary/image
 * placeholders and stats.
 */

import { describe, expect, it } from "vitest";
import type { DiffHunk, DiffLine, FileDiff } from "$lib/ipc/types";
import {
  buildRowModel,
  fileChangeKind,
  fileKindLabel,
  fileKindLetter,
  filePatchText,
  findMatches,
  hunkCounts,
  hunkPatchText,
  isContextLine,
  rowHeight,
  rowHeights,
  rowLineText,
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
// File change kind (header badge)
// ---------------------------------------------------------------------------

describe("fileChangeKind", () => {
  it("detects renames from a differing old_path", () => {
    expect(fileChangeKind(file({ old_path: "src/old.txt" }))).toBe("renamed");
    // Deleted files may carry an equal old_path; that is not a rename.
    expect(fileChangeKind(file({ old_path: "src/a.txt" }))).toBe("modified");
  });

  it("detects added/deleted files from their hunk sides", () => {
    const added = file({
      additions: 2,
      hunks: [hunk([line("+", "a", null, 1), line("+", "b", null, 2)])],
    });
    const deleted = file({
      deletions: 1,
      hunks: [hunk([line("-", "gone", 1, null)])],
    });
    expect(fileChangeKind(added)).toBe("added");
    expect(fileChangeKind(deleted)).toBe("deleted");
  });

  it("treats a mixed hunk as modified", () => {
    expect(fileChangeKind(file({ hunks: [changeHunk()] }))).toBe("modified");
  });

  it("falls back to counters for hunkless (binary/empty) files", () => {
    expect(fileChangeKind(file({ additions: 3 }))).toBe("added");
    expect(fileChangeKind(file({ deletions: 2 }))).toBe("deleted");
    expect(fileChangeKind(file({ binary: true }))).toBe("modified");
  });

  it("maps kinds to compact letters and labels", () => {
    expect(fileKindLetter("added")).toBe("A");
    expect(fileKindLetter("deleted")).toBe("D");
    expect(fileKindLetter("renamed")).toBe("R");
    expect(fileKindLetter("modified")).toBe("M");
    expect(fileKindLabel("renamed")).toBe("Renamed");
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
      line(" ", "ctx", 1, 1),
      line("-", "d1", 2, null),
      line("+", "a1", null, 2),
      line("+", "a2", null, 3),
    ]);
    const model = buildRowModel([file({ hunks: [h] })], "split", NO_COLLAPSE);
    const pairs = model.rows.filter((r) => r.kind === "pair") as Array<{
      left: DiffLine | null;
      right: DiffLine | null;
    }>;
    expect(pairs).toHaveLength(2);
    expect(pairs[0]).toMatchObject({ left: { text: "d1" }, right: { text: "a1" } });
    expect(pairs[1]).toMatchObject({ left: null, right: { text: "a2" } });
  });

  it("renders added files inline instead of a blank left half", () => {
    const added = file({
      additions: 2,
      hunks: [hunk([line("+", "a", null, 1), line("+", "b", null, 2)])],
    });
    const model = buildRowModel([added], "split", NO_COLLAPSE);
    expect(model.rows.map((r) => r.kind)).toEqual([
      "file-header",
      "hunk-header",
      "single",
      "single",
    ]);
    expect(model.rows[2]).toMatchObject({
      kind: "single",
      side: "new",
      line: { old_no: null, new_no: 1, text: "a" },
      fileIndex: 0,
      hunkIndex: 0,
      lineIndex: 0,
    });
    expect(model.hasSplitRows).toBe(false);
  });

  it("renders deleted files inline with the old gutter", () => {
    const deleted = file({
      deletions: 2,
      hunks: [hunk([line("-", "a", 1, null), line("-", "b", 2, null)])],
    });
    const model = buildRowModel([deleted], "split", NO_COLLAPSE);
    expect(model.rows.map((r) => r.kind)).toEqual([
      "file-header",
      "hunk-header",
      "single",
      "single",
    ]);
    for (const row of model.rows.filter((r) => r.kind === "single")) {
      expect(row).toMatchObject({ side: "old" });
    }
    expect(model.hasSplitRows).toBe(false);
  });

  it("keeps two-column split rows for modified files in the same model", () => {
    const files = [
      file({ path: "added.txt", additions: 1, hunks: [hunk([line("+", "a", null, 1)])] }),
      file({ path: "changed.txt", hunks: [changeHunk()] }),
    ];
    const model = buildRowModel(files, "split", NO_COLLAPSE);
    expect(model.rows.some((r) => r.kind === "single")).toBe(true);
    expect(model.rows.some((r) => r.kind === "context")).toBe(true);
    expect(model.hasSplitRows).toBe(true);
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
      "single", // b.txt has no previous content → inline row
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
        single: ROW_HEIGHTS.line,
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

// ---------------------------------------------------------------------------
// Whitespace toggles (rowModelOptions)
// ---------------------------------------------------------------------------

describe("buildRowModel whitespace options", () => {
  /** A ws-only pair ('foo ' → 'foo') plus a real change, one hunk. */
  function wsMixedHunk(): DiffHunk {
    return hunk([
      line(" ", "ctx", 1, 1),
      line("-", "foo ", 2, null),
      line("+", "foo", null, 2),
      line("-", "real del", 3, null),
      line("+", "real add", null, 3),
    ]);
  }

  it("hideWhitespaceRows drops trim-equal +/- pairs, keeps the rest", () => {
    const model = buildRowModel(
      [file({ hunks: [wsMixedHunk()] })],
      "unified",
      NO_COLLAPSE,
      { hideWhitespaceRows: true },
    );
    const texts = model.rows
      .filter((r) => r.kind === "line")
      .map((r) => (r as { line: DiffLine }).line.text);
    // The 'foo '/foo pair is gone; context and the real change stay.
    expect(texts).toEqual(["ctx", "real del", "real add"]);
  });

  it("hideWhitespaceRows keeps counts consistent with the visible rows", () => {
    const off = buildRowModel([file({ additions: 2, deletions: 2, hunks: [wsMixedHunk()] })], "unified", NO_COLLAPSE);
    const on = buildRowModel(
      [file({ additions: 2, deletions: 2, hunks: [wsMixedHunk()] })],
      "unified",
      NO_COLLAPSE,
      { hideWhitespaceRows: true },
    );
    expect(off.totalAdditions).toBe(2);
    expect(off.totalDeletions).toBe(2);
    expect(on.totalAdditions).toBe(1);
    expect(on.totalDeletions).toBe(1);
    // Hunk header counts shrink with the hidden rows too.
    const header = on.rows.find((r) => r.kind === "hunk-header") as {
      oldCount: number;
      newCount: number;
    };
    expect(header.oldCount).toBe(2); // ctx + real del
    expect(header.newCount).toBe(2); // ctx + real add
  });

  it("hideWhitespaceRows re-pairs split rows over the hidden pair", () => {
    const model = buildRowModel(
      [file({ hunks: [wsMixedHunk()] })],
      "split",
      NO_COLLAPSE,
      { hideWhitespaceRows: true },
    );
    const kinds = model.rows.map((r) => r.kind);
    expect(kinds).toEqual(["file-header", "hunk-header", "context", "pair"]);
    const pair = model.rows[3] as { left: DiffLine | null; right: DiffLine | null };
    expect(pair.left).toMatchObject({ text: "real del" });
    expect(pair.right).toMatchObject({ text: "real add" });
  });

  it("hideWhitespaceRows never hides context or unpaired changes", () => {
    const h = hunk([
      line(" ", "  ", 1, 1), // whitespace-only CONTEXT — must stay
      line("+", "  ", null, 2), // unpaired blank add — must stay
    ]);
    const model = buildRowModel([file({ hunks: [h] })], "unified", NO_COLLAPSE, {
      hideWhitespaceRows: true,
    });
    const texts = model.rows
      .filter((r) => r.kind === "line")
      .map((r) => (r as { line: DiffLine }).line.text);
    expect(texts).toEqual(["  ", "  "]);
  });

  it("hideWhitespaceRows keeps original lineIndex targets (selection)", () => {
    const model = buildRowModel(
      [file({ hunks: [wsMixedHunk()] })],
      "unified",
      NO_COLLAPSE,
      { hideWhitespaceRows: true },
    );
    const lineRows = model.rows.filter((r) => r.kind === "line") as Array<{
      lineIndex: number;
    }>;
    // Hidden indices 1/2 are skipped, but the kept rows still carry their
    // ORIGINAL positional index in the hunk's line list.
    expect(lineRows.map((r) => r.lineIndex)).toEqual([0, 3, 4]);
  });

  it("ignoreWhitespaceFiles drops zero-hunk text files, keeps binary/image", () => {
    const files = [
      file({ path: "ws-only.txt" }), // ws-only delta → 0 hunks
      file({ path: "blob.bin", binary: true }), // binary placeholder stays
      file({ path: "img.png", is_image: true }),
      file({ hunks: [hunk([line("+", "x", null, 1)])] }),
    ];
    const model = buildRowModel(files, "unified", NO_COLLAPSE, {
      ignoreWhitespaceFiles: true,
    });
    const headers = model.rows.filter((r) => r.kind === "file-header") as Array<{
      file: FileDiff;
    }>;
    expect(headers.map((h) => h.file.path)).toEqual(["blob.bin", "img.png", "src/a.txt"]);
    expect(model.fileCount).toBe(3);
  });

  it("without options the model is unchanged and fileCount matches the input", () => {
    const files = [file({ path: "a.txt" }), file({ path: "b.txt" })];
    const model = buildRowModel(files, "unified", NO_COLLAPSE);
    expect(model.fileCount).toBe(2);
    expect(model.rows.filter((r) => r.kind === "file-header")).toHaveLength(2);
  });
});

// ---------------------------------------------------------------------------
// Copy actions (unified patch synthesis)
// ---------------------------------------------------------------------------

describe("filePatchText / hunkPatchText", () => {
  it("synthesizes proper headers and @@ hunks for a modified file", () => {
    const f = file({ old_path: null, hunks: [changeHunk()] });
    const patch = filePatchText(f);
    const lines = patch.split("\n");
    expect(lines[0]).toBe("diff --git a/src/a.txt b/src/a.txt");
    expect(lines[1]).toBe("--- a/src/a.txt");
    expect(lines[2]).toBe("+++ b/src/a.txt");
    expect(lines[3]).toBe("@@ -1,4 +1,4 @@");
    expect(lines.slice(4)).toEqual([
      " keep",
      "-old",
      "-old2",
      "+new",
      "+new2",
      " tail",
    ]);
  });

  it("uses /dev/null sides and a/old_path for added, deleted, renamed files", () => {
    const added = file({
      path: "new.txt",
      additions: 1,
      hunks: [hunk([line("+", "x", null, 1)])],
    });
    expect(filePatchText(added).split("\n").slice(0, 3)).toEqual([
      "diff --git a/new.txt b/new.txt",
      "--- /dev/null",
      "+++ b/new.txt",
    ]);
    const deleted = file({
      path: "old.txt",
      deletions: 1,
      hunks: [hunk([line("-", "x", 1, null)])],
    });
    expect(filePatchText(deleted).split("\n").slice(0, 3)).toEqual([
      "diff --git a/old.txt b/old.txt",
      "--- a/old.txt",
      "+++ /dev/null",
    ]);
    const renamed = file({
      path: "b.txt",
      old_path: "a.txt",
      hunks: [hunk([line(" ", "x", 1, 1)])],
    });
    const header = filePatchText(renamed).split("\n").slice(0, 3);
    expect(header[0]).toBe("diff --git a/a.txt b/b.txt");
    expect(header[1]).toBe("--- a/a.txt");
    expect(header[2]).toBe("+++ b/b.txt");
  });

  it("hunkPatchText returns one hunk's patch and null for a bad index", () => {
    const f = file({
      hunks: [hunk([line("+", "a", null, 1)], 1, 1), hunk([line("-", "b", 5, null)], 5, 5)],
    });
    expect(hunkPatchText(f, 1)).toBe("@@ -5,1 +5,0 @@\n-b");
    expect(hunkPatchText(f, 9)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Find-in-diff
// ---------------------------------------------------------------------------

describe("findMatches", () => {
  it("matches case-insensitively across unified rows and reports ranges", () => {
    const model = buildRowModel([file({ hunks: [changeHunk()] })], "unified", NO_COLLAPSE);
    // rows: 0 header, 1 hunk-header, 2 keep, 3 old, 4 old2, 5 new, ...
    const hits = findMatches(model.rows, "OLD");
    expect(hits).toEqual([
      { rowIndex: 3, side: null, start: 0, end: 3 },
      { rowIndex: 4, side: null, start: 0, end: 3 },
    ]);
  });

  it("addresses split halves (left then right) per pair row", () => {
    const model = buildRowModel([file({ hunks: [changeHunk()] })], "split", NO_COLLAPSE);
    // rows: 0 header, 1 hunk-header, 2 context, 3 pair(old|new), 4 pair(old2|new2)
    const hits = findMatches(model.rows, "old2");
    expect(hits).toEqual([{ rowIndex: 4, side: "left", start: 0, end: 4 }]);
  });

  it("returns every non-overlapping occurrence and skips empty queries", () => {
    const model = buildRowModel(
      [file({ hunks: [hunk([line("+", "abab", null, 1)])] })],
      "unified",
      NO_COLLAPSE,
    );
    expect(findMatches(model.rows, "ab").map((m) => m.start)).toEqual([0, 2]);
    expect(findMatches(model.rows, "")).toEqual([]);
  });

  it("skips header rows (no searchable line text)", () => {
    const model = buildRowModel(
      [file({ path: "keep-old.txt", hunks: [changeHunk()] })],
      "unified",
      NO_COLLAPSE,
    );
    expect(findMatches(model.rows, "keep-old.txt")).toEqual([]);
  });
});

describe("rowLineText", () => {
  it("returns the row text, honoring the requested split half", () => {
    const split = buildRowModel([file({ hunks: [changeHunk()] })], "split", NO_COLLAPSE);
    const pair = split.rows[3];
    expect(rowLineText(pair, "left")).toBe("old");
    expect(rowLineText(pair, "right")).toBe("new");
    expect(rowLineText(pair, null)).toBe("old");
    const unified = buildRowModel([file({ hunks: [changeHunk()] })], "unified", NO_COLLAPSE);
    expect(rowLineText(unified.rows[3], null)).toBe("old");
    expect(rowLineText(unified.rows[0], null)).toBeNull(); // header row
  });
});
