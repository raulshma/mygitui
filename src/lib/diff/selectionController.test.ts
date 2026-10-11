/**
 * Unit tests for the DiffViewer selection controller (diff/selectionController.ts):
 * the drag lifecycle, click-vs-drag disambiguation, same-side confinement,
 * double-click word select, Ctrl+C clipboard synthesis, drag autoscroll, and
 * the gutter shift-extend machine — all against fake deps, no DOM.
 */

import { describe, expect, it, vi } from "vitest";
import type { DiffLine } from "$lib/ipc/types";
import type { LineSelection } from "$lib/components/diff/lineSelection";
import {
  createSelectionController,
  type PointerPosition,
  type SelectionDeps,
  type SplitSelection,
} from "./selectionController";

// ---------------------------------------------------------------------------
// Fakes

/** Four rows; row 2 is a pair filler (no half on the tracked side). */
const HALF: Record<number, string> = {
  0: "line-0-abcdef",
  1: "line-1-abcdef",
  3: "line-3-abcdef",
};

const at = (clientX: number, clientY: number, buttons = 1) => ({
  clientX,
  clientY,
  buttons,
});

/** A controller with recording apply hooks; `deps` patch the fakes. */
function setup(deps: Partial<SelectionDeps> = {}) {
  const applied: (SplitSelection | null)[] = [];
  const lines: (LineSelection | null)[] = [];
  const ctl = createSelectionController(
    {
      hitTest: () => null,
      applySelection: (sel) => void applied.push(sel),
      halfText: (row) => HALF[row] ?? null,
      ...deps,
    },
    { onLineSelection: (sel) => void lines.push(sel) },
  );
  return { ctl, applied, lines };
}

/** Split-side hit point helper. */
const hit = (row: number, char: number, side: "left" | "right") => ({
  row,
  char,
  side,
});

const HUNK: DiffLine[] = [
  { old_no: 1, new_no: 1, origin: " ", text: "keep", highlights: [] },
  { old_no: 2, new_no: null, origin: "-", text: "old", highlights: [] },
  { old_no: null, new_no: 2, origin: "+", text: "new", highlights: [] },
  { old_no: null, new_no: 3, origin: "+", text: "new2", highlights: [] },
  { old_no: 4, new_no: 4, origin: " ", text: "tail", highlights: [] },
];

// ---------------------------------------------------------------------------

describe("drag selection", () => {
  it("pointerDown anchors at the hit point and reports a drag", () => {
    const { ctl, applied } = setup({
      hitTest: () => hit(1, 4, "left"),
    });
    expect(ctl.pointerDown(at(10, 20))).toBe(true);
    expect(applied).toEqual([
      { side: "left", anchor: { row: 1, char: 4 }, focus: { row: 1, char: 4 } },
    ]);
  });

  it("pointerDown off text anchors nothing", () => {
    const { ctl, applied } = setup({ hitTest: () => null });
    expect(ctl.pointerDown(at(10, 20))).toBe(false);
    expect(applied).toEqual([]);
  });

  it("mousemove extends the focus on the same side, anchor stays put", () => {
    const { ctl, applied } = setup({
      hitTest: (pos: PointerPosition) =>
        pos.clientX < 5 ? hit(0, 0, "left") : hit(3, 2, "left"),
    });
    ctl.pointerDown(at(0, 0));
    ctl.pointerMove(at(50, 100));
    expect(applied.at(-1)).toEqual({
      side: "left",
      anchor: { row: 0, char: 0 },
      focus: { row: 3, char: 2 },
    });
  });

  it("mousemove without the primary button is ignored", () => {
    const { ctl, applied } = setup({ hitTest: () => hit(1, 1, "left") });
    ctl.pointerDown(at(0, 0));
    ctl.pointerMove(at(50, 100, 0)); // buttons released
    expect(applied).toHaveLength(1);
    ctl.pointerMove(at(50, 100)); // undefined buttons still counts as a drag
    expect(applied).toHaveLength(2);
  });

  it("mousemove before a pointerDown is inert", () => {
    const { ctl, applied } = setup({ hitTest: () => hit(1, 1, "left") });
    ctl.pointerMove(at(50, 100));
    expect(applied).toEqual([]);
  });

  it("crossing the divider extends only to the same row's dragging-side edge", () => {
    // Left drag sweeping right: grabs the rest of its own line.
    const left = setup({
      hitTest: (pos) =>
        pos.clientX < 5 ? hit(1, 0, "left") : hit(1, 0, "right"),
    });
    left.ctl.pointerDown(at(0, 0));
    left.ctl.pointerMove(at(999, 100)); // right half, same row
    expect(left.applied.at(-1)?.focus).toEqual({ row: 1, char: HALF[1]!.length });

    // Right drag sweeping left: to the row's first char.
    const right = setup({
      hitTest: (pos) =>
        pos.clientX < 5 ? hit(1, 4, "left") : hit(1, 4, "right"),
    });
    right.ctl.pointerDown(at(999, 0));
    right.ctl.pointerMove(at(0, 100)); // left half, same row
    expect(right.applied.at(-1)?.focus).toEqual({ row: 1, char: 0 });
  });

  it("crossing rows on the other side freezes the selection", () => {
    const { ctl, applied } = setup({
      hitTest: (pos) => (pos.clientX < 5 ? hit(1, 0, "left") : hit(3, 0, "right")),
    });
    ctl.pointerDown(at(0, 0));
    ctl.pointerMove(at(50, 300)); // wrong column AND wrong row
    expect(applied).toHaveLength(1); // no update
  });

  it("crossing to a filler half on the same row is ignored", () => {
    const { ctl, applied } = setup({
      hitTest: (pos) => (pos.clientX < 5 ? hit(1, 0, "left") : hit(2, 0, "right")),
    });
    ctl.pointerDown(at(0, 0));
    ctl.pointerMove(at(50, 200)); // row 2 has no right half
    expect(applied).toHaveLength(1);
  });

  it("pointerUp ends the drag; later moves are inert", () => {
    const { ctl, applied } = setup({
      hitTest: (pos) => (pos.clientX < 5 ? hit(0, 0, "left") : hit(1, 0, "left")),
    });
    ctl.pointerDown(at(0, 0));
    ctl.pointerUp();
    ctl.pointerMove(at(50, 100));
    expect(applied).toHaveLength(1);
  });
});

describe("drag autoscroll", () => {
  const rect = { top: 100, bottom: 500 };

  function withViewport() {
    const scrollBy = vi.fn();
    const { ctl } = setup({
      hitTest: () => hit(1, 0, "left"),
      viewport: () => ({ rect, scrollBy }),
    });
    ctl.pointerDown(at(0, 0));
    return { ctl, scrollBy };
  }

  it("scrolls up inside the top edge zone", () => {
    const { ctl, scrollBy } = withViewport();
    ctl.pointerMove(at(0, 110)); // 10px below the top edge
    expect(scrollBy).toHaveBeenCalledWith(-14);
  });

  it("scrolls down inside the bottom edge zone", () => {
    const { ctl, scrollBy } = withViewport();
    ctl.pointerMove(at(0, 490)); // 10px above the bottom edge
    expect(scrollBy).toHaveBeenCalledWith(14);
  });

  it("does not scroll at the exact edge boundary or mid-viewport", () => {
    const { ctl, scrollBy } = withViewport();
    ctl.pointerMove(at(0, 124)); // top + 24 — the boundary itself
    ctl.pointerMove(at(0, 476)); // bottom - 24 — the boundary itself
    ctl.pointerMove(at(0, 300)); // middle
    expect(scrollBy).not.toHaveBeenCalled();
  });

  it("copies with no viewport at all (absent or null)", () => {
    const absent = setup({ hitTest: () => hit(1, 0, "left") });
    absent.ctl.pointerDown(at(0, 0));
    expect(() => absent.ctl.pointerMove(at(0, 100))).not.toThrow();

    const nulled = setup({
      hitTest: () => hit(1, 0, "left"),
      viewport: () => null,
    });
    nulled.ctl.pointerDown(at(0, 0));
    expect(() => nulled.ctl.pointerMove(at(0, 100))).not.toThrow();
  });
});

describe("click vs drag (dragJustMoved)", () => {
  it("a drag that moved the focus swallows the following click", () => {
    const { ctl } = setup({
      hitTest: (pos) => (pos.clientX < 5 ? hit(0, 0, "left") : hit(0, 6, "left")),
    });
    ctl.pointerDown(at(0, 0));
    ctl.pointerMove(at(50, 0));
    ctl.pointerUp();
    expect(ctl.consumeDragClick()).toBe(true);
    expect(ctl.consumeDragClick()).toBe(false); // consumed once
  });

  it("a plain click does not swallow", () => {
    const { ctl } = setup({ hitTest: () => hit(0, 0, "left") });
    ctl.pointerDown(at(0, 0));
    ctl.pointerUp();
    expect(ctl.consumeDragClick()).toBe(false);
  });

  it("a same-point mousemove is not a drag", () => {
    const { ctl } = setup({ hitTest: () => hit(0, 0, "left") });
    ctl.pointerDown(at(0, 0));
    ctl.pointerMove(at(1, 1)); // same row/char → focus unchanged
    ctl.pointerUp();
    expect(ctl.consumeDragClick()).toBe(false);
  });

  it("a new pointerDown re-arms the flag", () => {
    const { ctl } = setup({
      hitTest: (pos) => (pos.clientX < 5 ? hit(0, 0, "left") : hit(0, 6, "left")),
    });
    ctl.pointerDown(at(0, 0));
    ctl.pointerMove(at(50, 0));
    ctl.pointerUp();
    ctl.consumeDragClick();
    ctl.pointerDown(at(0, 0));
    expect(ctl.consumeDragClick()).toBe(false);
  });
});

describe("double-click word select", () => {
  it("selects the word under the point", () => {
    const { ctl, applied } = setup({
      hitTest: () => hit(1, 8, "left"),
      halfText: (row) => (row === 1 ? "const alpha_beta = 1" : null),
    });
    ctl.doubleClick(at(30, 40));
    expect(applied).toEqual([
      { side: "left", anchor: { row: 1, char: 6 }, focus: { row: 1, char: 16 } },
    ]);
  });

  it("leaves the selection alone when no word is under the point", () => {
    const { ctl, applied } = setup({
      hitTest: () => hit(1, 5, "left"), // the space after "const"
      halfText: (row) => (row === 1 ? "const alpha_beta = 1" : null),
    });
    ctl.doubleClick(at(30, 40));
    expect(applied).toEqual([]);
  });

  it("does nothing off text", () => {
    const { ctl, applied } = setup({ hitTest: () => null });
    ctl.doubleClick(at(30, 40));
    expect(applied).toEqual([]);
  });
});

describe("Ctrl+C clipboard synthesis", () => {
  function dragSetup(overrides: Partial<SelectionDeps> = {}) {
    const writeClipboard = vi.fn();
    const s = setup({
      hitTest: (pos) => (pos.clientX < 5 ? hit(0, 5, "left") : hit(1, 2, "left")),
      writeClipboard,
      ...overrides,
    });
    return { writeClipboard, ...s };
  }

  it("copies the selected side's segments joined with newlines", () => {
    const { ctl, writeClipboard } = dragSetup();
    ctl.pointerDown(at(0, 0));
    ctl.pointerMove(at(50, 20)); // row 1, char 2
    expect(ctl.copy()).toBe(true);
    expect(writeClipboard).toHaveBeenCalledWith("0-abcdef\nli");
  });

  it("returns false with nothing selected and writes nothing", () => {
    const { ctl, writeClipboard } = dragSetup();
    expect(ctl.copy()).toBe(false);
    expect(writeClipboard).not.toHaveBeenCalled();
  });

  it("a native selection wins", () => {
    const { ctl, writeClipboard } = dragSetup({
      nativeSelectionText: () => "native text",
    });
    ctl.pointerDown(at(0, 0));
    ctl.pointerMove(at(50, 20));
    expect(ctl.copy()).toBe(false);
    expect(writeClipboard).not.toHaveBeenCalled();
  });

  it("returns false when the segments hold no text", () => {
    const { ctl, writeClipboard } = dragSetup({
      hitTest: () => hit(1, 0, "left"),
      halfText: (row) => (row === 1 ? "" : null),
    });
    ctl.pointerDown(at(0, 0));
    expect(ctl.copy()).toBe(false);
    expect(writeClipboard).not.toHaveBeenCalled();
  });
});

describe("gutter clicks (line selection)", () => {
  function gutterSetup(hunkLines: SelectionDeps["hunkLines"] = () => HUNK) {
    return setup({ hunkLines });
  }

  it("a click selects the line's new-side number", () => {
    const { ctl, lines } = gutterSetup();
    ctl.gutterClick(0, 0, 1, false); // the '-' line → nearest new-side 2
    expect(lines).toEqual([
      { fileIndex: 0, hunkIndex: 0, lines: [2], anchor: 2 },
    ]);
  });

  it("shift-click extends from the anchor, clamped to the hunk", () => {
    const { ctl, lines } = gutterSetup();
    ctl.gutterClick(0, 0, 0, false); // new-side 1
    ctl.gutterClick(0, 0, 4, true); // new-side 4
    expect(lines.at(-1)).toEqual({
      fileIndex: 0,
      hunkIndex: 0,
      lines: [1, 2, 3, 4],
      anchor: 1,
    });
  });

  it("shift-click in another hunk starts a fresh selection", () => {
    const { ctl, lines } = gutterSetup((fileIndex, hunkIndex) =>
      fileIndex === 0 && hunkIndex === 0 ? HUNK : HUNK.slice(3),
    );
    ctl.gutterClick(0, 0, 0, false);
    ctl.gutterClick(0, 1, 0, true); // different hunk → begin, not extend
    expect(lines.at(-1)).toEqual({
      fileIndex: 0,
      hunkIndex: 1,
      lines: [3],
      anchor: 3,
    });
  });

  it("a hunk without lines or without any new-side number is inert", () => {
    const none = gutterSetup(() => null);
    none.ctl.gutterClick(0, 0, 0, false);
    expect(none.lines).toEqual([]);

    const oldOnly = gutterSetup(() => [
      { old_no: 1, new_no: null, origin: "-", text: "old", highlights: [] },
    ]);
    oldOnly.ctl.gutterClick(0, 0, 0, false);
    expect(oldOnly.lines).toEqual([]);
  });

  it("clearLines clears, and clearAll clears both kinds", () => {
    const { ctl, lines, applied } = setup({
      hitTest: () => hit(0, 0, "left"),
      hunkLines: () => HUNK,
    });
    ctl.gutterClick(0, 0, 0, false);
    ctl.clearLines();
    expect(lines.at(-1)).toBeNull();

    ctl.gutterClick(0, 0, 0, false);
    ctl.pointerDown(at(0, 0));
    ctl.clearAll();
    expect(applied.at(-1)).toBeNull();
    expect(lines.at(-1)).toBeNull();
  });
});
