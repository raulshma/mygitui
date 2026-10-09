/**
 * Unit tests for the line-granular selection model (diff/lineSelection.ts):
 * begin/extend clamped to the hunk, range merging, and the render-side
 * selection check that mirrors the backend's `mark_kept` semantics.
 */

import { describe, expect, it } from "vitest";
import type { DiffLine } from "$lib/ipc/types";
import {
  beginSelection,
  clearSelection,
  extendSelection,
  hunkNewSides,
  isLineSelected,
  mergeRanges,
  selectableAnchor,
  selectionKey,
  selectionRanges,
  selectedSet,
} from "$lib/components/diff/lineSelection";

function line(
  origin: DiffLine["origin"],
  text: string,
  oldNo: number | null,
  newNo: number | null,
): DiffLine {
  return { old_no: oldNo, new_no: newNo, origin, text, highlights: [] };
}

/** ctx, -d1, -d2, +a1, +a2, ctx — the canonical paired hunk. */
function pairedHunk(): DiffLine[] {
  return [
    line(" ", "keep", 10, 10),
    line("-", "d1", 11, null),
    line("-", "d2", 12, null),
    line("+", "a1", null, 11),
    line("+", "a2", null, 12),
    line(" ", "tail", 13, 13),
  ];
}

describe("selectionKey", () => {
  it("keys by file and hunk", () => {
    expect(selectionKey(2, 3)).toBe("2:3");
  });
});

describe("beginSelection", () => {
  it("starts a one-line selection with the anchor set", () => {
    expect(beginSelection(0, 1, 7)).toEqual({
      fileIndex: 0,
      hunkIndex: 1,
      lines: [7],
      anchor: 7,
    });
  });

  it("rejects non-new-side anchors (0, negative, fractional)", () => {
    expect(beginSelection(0, 0, 0)).toBeNull();
    expect(beginSelection(0, 0, -3)).toBeNull();
    expect(beginSelection(0, 0, 2.5)).toBeNull();
  });
});

/** Hunk whose new sides are exactly 2..5 (plus old-side-only lines). */
function spanHunk(): DiffLine[] {
  return [
    line("+", "a", null, 2),
    line("-", "d", 1, null),
    line("+", "b", null, 3),
    line("+", "c", null, 4),
    line("+", "e", null, 5),
  ];
}

describe("extendSelection", () => {
  it("fills the inclusive span between anchor and target", () => {
    const sel = beginSelection(0, 0, 2)!;
    const ext = extendSelection(sel, 5, spanHunk());
    expect(ext.lines).toEqual([2, 3, 4, 5]);
    expect(ext.anchor).toBe(2);
  });

  it("extends backwards (target above the anchor)", () => {
    const sel = beginSelection(0, 0, 12)!;
    const ext = extendSelection(sel, 10, pairedHunk());
    expect(ext.lines).toEqual([10, 11, 12]);
  });

  it("clamps to the hunk: only existing new-side numbers survive", () => {
    // The hunk's new sides are 10,11,12,13 — a span of 10..20 clamps to those.
    const sel = beginSelection(0, 0, 10)!;
    const ext = extendSelection(sel, 20, pairedHunk());
    expect(ext.lines).toEqual([10, 11, 12, 13]);
  });

  it("clamps below the hunk start", () => {
    const sel = beginSelection(0, 0, 12)!;
    const ext = extendSelection(sel, 1, pairedHunk());
    expect(ext.lines).toEqual([10, 11, 12]);
  });

  it("keeps the anchor when the target lies beyond the hunk's numbers", () => {
    const sel = beginSelection(0, 0, 11)!;
    const ext = extendSelection(sel, 999, pairedHunk());
    expect(ext.lines).toEqual([11, 12, 13]);
  });

  it("does not mutate the input selection", () => {
    const sel = beginSelection(0, 0, 10)!;
    extendSelection(sel, 13, pairedHunk());
    expect(sel.lines).toEqual([10]);
  });
});

describe("hunkNewSides", () => {
  it("lists new-side numbers ascending, skipping old-side-only lines", () => {
    expect(hunkNewSides(pairedHunk())).toEqual([10, 11, 12, 13]);
  });
});

describe("mergeRanges / selectionRanges", () => {
  it("collapses consecutive numbers into one inclusive range", () => {
    expect(mergeRanges([1, 2, 3])).toEqual([{ start: 1, end: 3 }]);
  });

  it("splits at gaps and tolerates unsorted input", () => {
    expect(mergeRanges([5, 1, 2, 9])).toEqual([
      { start: 1, end: 2 },
      { start: 5, end: 5 },
      { start: 9, end: 9 },
    ]);
  });

  it("returns [] for an empty selection", () => {
    expect(mergeRanges([])).toEqual([]);
  });

  it("selectionRanges mirrors mergeRanges", () => {
    const sel = beginSelection(0, 0, 10)!;
    const ext = extendSelection(sel, 13, pairedHunk());
    expect(selectionRanges(ext)).toEqual([{ start: 10, end: 13 }]);
  });
});

describe("selectedSet", () => {
  it("is empty for null", () => {
    expect(selectedSet(null).size).toBe(0);
  });
  it("contains the selected numbers", () => {
    expect(selectedSet(beginSelection(0, 0, 3)!)).toEqual(new Set([3]));
  });
});

describe("clearSelection", () => {
  it("returns the cleared (null) state", () => {
    expect(clearSelection()).toBeNull();
  });
});

describe("selectableAnchor", () => {
  it("returns the line's own new-side number", () => {
    const hunk = pairedHunk();
    expect(selectableAnchor(hunk, 0)).toBe(10); // context
    expect(selectableAnchor(hunk, 3)).toBe(11); // '+'
  });

  it("anchors '-' lines to the nearest new-side number, forward first", () => {
    const hunk = pairedHunk();
    expect(selectableAnchor(hunk, 1)).toBe(11); // forward: paired '+'
    expect(selectableAnchor(hunk, 2)).toBe(11); // forward: paired '+'
  });

  it("falls back backward at the end of the hunk", () => {
    const hunk = [
      line(" ", "ctx", 1, 1),
      line("-", "d", 2, null),
    ];
    expect(selectableAnchor(hunk, 1)).toBe(1);
  });

  it("returns null when the hunk has no new-side numbers at all", () => {
    const hunk = [line("-", "d", 1, null)];
    expect(selectableAnchor(hunk, 0)).toBeNull();
  });

  it("returns null for out-of-range indexes", () => {
    expect(selectableAnchor(pairedHunk(), 99)).toBeNull();
  });
});

describe("isLineSelected (backend mark_kept semantics)", () => {
  it("'+' and context lines follow their own new-side number", () => {
    const hunk = pairedHunk();
    const sel = selectedSet(beginSelection(0, 0, 11)!);
    expect(isLineSelected(hunk, 3, sel)).toBe(true); // +a1 (new 11)
    expect(isLineSelected(hunk, 4, sel)).toBe(false); // +a2 (new 12)
    expect(isLineSelected(hunk, 0, sel)).toBe(false); // context 10
  });

  it("context inside a selected span highlights", () => {
    const sel = selectedSet(beginSelection(0, 0, 10)!);
    expect(isLineSelected(pairedHunk(), 0, sel)).toBe(true);
  });

  it("a '-' run highlights when any paired '+' line is selected", () => {
    const hunk = pairedHunk();
    const sel = selectedSet(beginSelection(0, 0, 12)!); // only +a2
    expect(isLineSelected(hunk, 1, sel)).toBe(true); // -d1
    expect(isLineSelected(hunk, 2, sel)).toBe(true); // -d2
  });

  it("a '-' run stays dim when no paired '+' line is selected", () => {
    const hunk = pairedHunk();
    const sel = selectedSet(beginSelection(0, 0, 10)!); // context only
    expect(isLineSelected(hunk, 1, sel)).toBe(false);
    expect(isLineSelected(hunk, 2, sel)).toBe(false);
  });

  it("a pure deletion run highlights via its nearest new-side number", () => {
    const hunk = [
      line(" ", "keep", 1, 1),
      line("-", "d1", 2, null),
      line("-", "d2", 3, null),
      line(" ", "tail", 4, 4),
    ];
    const sel = selectedSet(beginSelection(0, 0, 4)!); // nearest forward (tail)
    expect(isLineSelected(hunk, 1, sel)).toBe(true);
    expect(isLineSelected(hunk, 2, sel)).toBe(true);

    const selBack = selectedSet(beginSelection(0, 0, 1)!);
    // Forward wins: the run follows new-side 4 (tail), not selected 1.
    expect(isLineSelected(hunk, 1, selBack)).toBe(false);
  });

  it("returns false for unknown indexes or empty selections", () => {
    expect(isLineSelected(pairedHunk(), 99, selectedSet(null))).toBe(false);
  });
});
