/**
 * Selection state machine for DiffViewer (pure orchestration; no DOM, no runes).
 *
 * Extracted from DiffViewer.svelte so the decisions are unit-testable with
 * fake deps in jsdom. The controller owns WHEN things happen — anchor, drag,
 * autoscroll trigger, click-vs-drag disambiguation (`dragJustMoved`), the
 * same-side confinement while sweeping across the split divider, double-click
 * word select, Ctrl+C clipboard synthesis, and the gutter-click shift-extend
 * machine. HOW they touch the browser is injected:
 *
 *   - `hitTest` — caret hit-testing (caretRangeFromPoint & co) → a split-half
 *     point; provided by selectionDom.ts. Never called with real DOM inside
 *     this module.
 *   - `applySelection` — reflects the split segment selection into the
 *     host's reactive state (the component paints it with the CSS Custom
 *     Highlight API; see DiffViewer's paint effect).
 *   - `onLineSelection` — same for the gutter (line) selection.
 *
 * The pure geometry lives in the existing modules and is orchestrated, not
 * duplicated: splitSelection.ts (segments, clipboard text, word spans) and
 * lineSelection.ts (begin/extend/anchor for gutter clicks).
 */

import type { DiffLine } from "$lib/ipc/types";
import {
  beginSelection,
  extendSelection,
  selectableAnchor,
  type LineSelection,
} from "$lib/components/diff/lineSelection";
import {
  segmentText,
  selectionSegments,
  wordSpanAt,
  type SelPoint,
} from "$lib/components/diff/splitSelection";

/** Which half of a split row. */
export type Side = "left" | "right";

/** A selection point that also knows its half (what hit-testing yields). */
export type SplitPoint = SelPoint & { side: Side };

/** One active split-view selection: anchor/focus points on a shared side. */
export interface SplitSelection {
  side: Side;
  anchor: SelPoint;
  focus: SelPoint;
}

/** Pointer position the controller needs (a MouseEvent supplies these). */
export interface PointerPosition {
  clientX: number;
  clientY: number;
}

/** Viewport access for drag autoscroll — read fresh on every move. */
export interface ViewportHandle {
  /** Viewport rect (top/bottom delimit the autoscroll edge zones). */
  rect: { top: number; bottom: number };
  /** Scrolls the viewport by `dy` px (positive = down). */
  scrollBy(dy: number): void;
}

/** Geometry/DOM operations the host injects (all fakeable in tests). */
export interface SelectionDeps {
  /**
   * The split-half point under a pointer position, or null when it is not
   * over selectable half text (filler half / gutter / outside the rows).
   */
  hitTest(pos: PointerPosition): SplitPoint | null;
  /** Reflects the split segment selection (null = cleared) into host state. */
  applySelection(sel: SplitSelection | null): void;
  /** A row half's line text on `side` (null = filler half / not a split row). */
  halfText(row: number, side: Side): string | null;
  /** The scroll viewport for drag autoscroll (null/absent = no autoscroll). */
  viewport?(): ViewportHandle | null;
  /** Text of the current native selection, if any (a native copy wins). */
  nativeSelectionText?(): string;
  /** Clipboard sink for synthesized copies (default: navigator.clipboard). */
  writeClipboard?(text: string): void;
  /** A hunk's lines for gutter selection (absent/null = not selectable). */
  hunkLines?(fileIndex: number, hunkIndex: number): readonly DiffLine[] | null;
}

/** Host notifications for the selections the controller does not apply. */
export interface SelectionCallbacks {
  /** Reflects the gutter line selection (null = cleared) into host state. */
  onLineSelection?(sel: LineSelection | null): void;
}

/** Distance from the viewport edge (px) inside which a drag autoscrolls. */
const AUTOSCROLL_EDGE_PX = 24;
/** Autoscroll speed (px per mousemove while inside the edge zone). */
const AUTOSCROLL_STEP_PX = 14;

/** The pointer → clipboard → gutter state machine. All methods are safe to
 * pass around by reference (closure-based; no `this`). */
export interface SelectionController {
  /**
   * Split-view mousedown: anchors a selection at the hit point and starts
   * drag tracking. Returns whether a drag began (the host attaches its
   * window mousemove/mouseup listeners only then).
   */
  pointerDown(pos: PointerPosition): boolean;
  /**
   * Mousemove during a drag: autoscrolls near the viewport edges, then moves
   * the focus — moves onto the opposite side are confined to the dragging
   * side's own row edge (the entire side confinement). Flags `dragJustMoved`
   * when the focus actually changed.
   */
  pointerMove(pos: PointerPosition & { buttons?: number }): void;
  /** Mouseup: ends the drag (`dragJustMoved` survives for the click). */
  pointerUp(): void;
  /** Double-click: selects the word under the point (native one is blocked). */
  doubleClick(pos: PointerPosition): void;
  /**
   * Ctrl+C: synthesizes the selected side's text onto the clipboard via
   * splitSelection.ts. A native selection wins; returns whether the copy was
   * synthesized (the host preventDefault()s only then).
   */
  copy(): boolean;
  /**
   * Gutter click: starts a line selection, or — with `extend` and a selection
   * in the same hunk — shift-extends it (lineSelection.ts semantics).
   */
  gutterClick(
    fileIndex: number,
    hunkIndex: number,
    lineIndex: number,
    extend: boolean,
  ): void;
  /**
   * Whether the click that just fired belongs to a drag that moved the
   * focus (consumes the flag) — such a click must not clear the selection
   * the drag just made.
   */
  consumeDragClick(): boolean;
  /** Clears the split segment selection. */
  clearSegments(): void;
  /** Clears the gutter line selection. */
  clearLines(): void;
  /** Clears both selections. */
  clearAll(): void;
}

export function createSelectionController(
  deps: SelectionDeps,
  callbacks: SelectionCallbacks = {},
): SelectionController {
  const writeClipboard =
    deps.writeClipboard ??
    ((text: string): void => {
      navigator.clipboard?.writeText(text).catch(() => {});
    });
  const nativeSelectionText = deps.nativeSelectionText ?? ((): string => "");
  const halfLength = (row: number, side: Side): number | null => {
    const t = deps.halfText(row, side);
    return t === null ? null : t.length;
  };

  /** Current split segment selection (mirrored into host state). */
  let segments: SplitSelection | null = null;
  /** Current gutter line selection (mirrored into host state). */
  let lines: LineSelection | null = null;
  /** Side the active drag started on (null = no drag). */
  let dragSide: Side | null = null;
  /**
   * Set when a drag actually moved the focus; the click that follows the
   * mouseup belongs to that drag — it must not erase the selection it made.
   */
  let dragJustMoved = false;

  function setSegments(sel: SplitSelection | null): void {
    segments = sel;
    deps.applySelection(sel);
  }

  function setLines(sel: LineSelection | null): void {
    lines = sel;
    callbacks.onLineSelection?.(sel);
  }

  return {
    pointerDown(pos): boolean {
      const start = deps.hitTest(pos);
      if (!start) return false;
      dragJustMoved = false;
      dragSide = start.side;
      setSegments({
        side: start.side,
        anchor: { row: start.row, char: start.char },
        focus: { row: start.row, char: start.char },
      });
      return true;
    },

    pointerMove(pos): void {
      if (dragSide === null) return;
      if (pos.buttons !== undefined && !(pos.buttons & 1)) return;
      // Autoscroll near the viewport edges (native drags get this for
      // free; ours doesn't create a native selection).
      const vp = deps.viewport?.();
      if (vp) {
        if (pos.clientY < vp.rect.top + AUTOSCROLL_EDGE_PX) {
          vp.scrollBy(-AUTOSCROLL_STEP_PX);
        } else if (pos.clientY > vp.rect.bottom - AUTOSCROLL_EDGE_PX) {
          vp.scrollBy(AUTOSCROLL_STEP_PX);
        }
      }
      const p = deps.hitTest(pos);
      if (!p || !segments) return;
      let focus: SelPoint = { row: p.row, char: p.char };
      if (p.side !== dragSide) {
        // Crossed the divider: only the SAME row may extend, and only to
        // this row's edge on the dragging side (native-like: sweeping
        // toward the other half grabs the rest of your own line, never
        // the other half itself; crossing rows from the wrong column
        // freezes the selection instead of growing it).
        if (p.row !== segments.focus.row) return;
        const t = halfLength(p.row, dragSide);
        if (t === null) return;
        focus = { row: p.row, char: dragSide === "left" ? t : 0 };
      }
      if (focus.row !== segments.focus.row || focus.char !== segments.focus.char) {
        dragJustMoved = true;
      }
      setSegments({ side: dragSide, anchor: segments.anchor, focus });
    },

    pointerUp(): void {
      dragSide = null;
    },

    doubleClick(pos): void {
      const pt = deps.hitTest(pos);
      if (!pt) return;
      const line = deps.halfText(pt.row, pt.side) ?? "";
      const [start, end] = wordSpanAt(line, pt.char);
      if (end > start) {
        setSegments({
          side: pt.side,
          anchor: { row: pt.row, char: start },
          focus: { row: pt.row, char: end },
        });
      }
    },

    copy(): boolean {
      const sel = segments;
      if (!sel) return false;
      if (nativeSelectionText()) return false; // a native selection wins
      const segs = selectionSegments(sel.anchor, sel.focus, (row) =>
        halfLength(row, sel.side),
      );
      const text = segmentText(segs, (row) => deps.halfText(row, sel.side) ?? "");
      if (!text) return false;
      writeClipboard(text);
      return true;
    },

    gutterClick(fileIndex, hunkIndex, lineIndex, extend): void {
      if (!deps.hunkLines) return;
      const hunkLines = deps.hunkLines(fileIndex, hunkIndex);
      if (!hunkLines) return;
      const anchor = selectableAnchor(hunkLines, lineIndex);
      if (anchor === null) return;
      if (
        extend &&
        lines &&
        lines.fileIndex === fileIndex &&
        lines.hunkIndex === hunkIndex
      ) {
        setLines(extendSelection(lines, anchor, hunkLines));
      } else {
        setLines(beginSelection(fileIndex, hunkIndex, anchor));
      }
    },

    consumeDragClick(): boolean {
      if (!dragJustMoved) return false;
      dragJustMoved = false;
      return true;
    },

    clearSegments(): void {
      setSegments(null);
    },

    clearLines(): void {
      setLines(null);
    },

    clearAll(): void {
      setSegments(null);
      setLines(null);
    },
  };
}
