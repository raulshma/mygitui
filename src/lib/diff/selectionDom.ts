/**
 * DOM adapters for the split-view selection controller (thin plumbing only).
 *
 * These are the jsdom-unreachable bits of DiffViewer's split-view selection —
 * caret hit-testing and Range construction — lifted verbatim from the
 * component so DiffViewer.svelte stays declarative and the controller
 * (selectionController.ts) stays DOM-free and unit-testable. No state, no
 * decisions: each function is one step the component wires into the
 * controller's deps or paint effect.
 */

import type { Segment } from "$lib/components/diff/splitSelection";
import type {
  PointerPosition,
  Side,
  SplitPoint,
} from "./selectionController";

/**
 * The selection point (row/side/char) under a pointer position, via the
 * caret APIs. Null over filler halves / non-text zones — callers treat that
 * as "no update". This is the controller's `hitTest` in the real component;
 * only clientX/clientY are read, so any MouseEvent satisfies it.
 */
export function pointFromEvent(
  e: PointerPosition,
): SplitPoint | null {
  const doc = document as Document & {
    caretPositionFromPoint?: (
      x: number,
      y: number,
    ) => { offsetNode: Node; offset: number } | null;
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
  };
  let node: Node | null = null;
  let offset = 0;
  if (doc.caretPositionFromPoint) {
    const p = doc.caretPositionFromPoint(e.clientX, e.clientY);
    if (p) {
      node = p.offsetNode;
      offset = p.offset;
    }
  } else if (doc.caretRangeFromPoint) {
    const r = doc.caretRangeFromPoint(e.clientX, e.clientY);
    if (r) {
      node = r.startContainer;
      offset = r.startOffset;
    }
  }
  if (!node) return null;
  const el = node instanceof Element ? node : node.parentElement;
  const half = el?.closest(".half");
  if (!half) return null;
  const rowEl = half.closest("[data-row-index]");
  if (!rowEl) return null;
  const row = Number(rowEl.getAttribute("data-row-index"));
  if (!Number.isInteger(row)) return null;
  const txt = half.querySelector(".txt");
  if (!txt) return null; // filler half
  // Logical char offset: sum the text-node lengths ahead of the caret.
  const walker = document.createTreeWalker(txt, NodeFilter.SHOW_TEXT);
  let char = 0;
  let resolved = false;
  let n = walker.nextNode();
  while (n) {
    if (n === node) {
      char += offset;
      resolved = true;
      break;
    }
    char += (n as Text).data.length;
    n = walker.nextNode();
  }
  if (!resolved && node === txt && typeof offset === "number") {
    // Caret hit the .txt element itself: the offset is a child index —
    // sum the text content of the children before it.
    const children = txt.childNodes;
    for (let i = 0; i < offset && i < children.length; i++) {
      char += children[i]!.textContent?.length ?? 0;
    }
    resolved = true;
  }
  if (!resolved) {
    // Caret landed on the sign/gutter side of the half: snap to the
    // nearest end of the text.
    const rect = txt.getBoundingClientRect();
    char = e.clientX < rect.left ? 0 : char;
    resolved = true;
  }
  return {
    row,
    char,
    side: half.classList.contains("right") ? "right" : "left",
  };
}

/** A DOM range for one segment, or null when its row isn't mounted. */
export function rangeForSegment(
  viewport: HTMLElement | undefined,
  seg: Segment,
  side: Side,
): Range | null {
  const rowEl = viewport?.querySelector(`[data-row-index="${seg.row}"]`);
  if (!rowEl) return null;
  const half =
    side === "left"
      ? rowEl.querySelector(".half:not(.right)")
      : rowEl.querySelector(".half.right");
  const txt = half?.querySelector(".txt");
  if (!txt) return null;
  return textNodeRange(txt, seg.start, seg.end);
}

/** Range over the text inside `container` for char offsets [start, end). */
export function textNodeRange(
  container: Element,
  start: number,
  end: number,
): Range | null {
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  let pos = 0;
  let startNode: Text | null = null;
  let startOff = 0;
  let endNode: Text | null = null;
  let endOff = 0;
  let node = walker.nextNode();
  while (node) {
    const len = (node as Text).data.length;
    if (!startNode && pos + len > start) {
      startNode = node as Text;
      startOff = start - pos;
    }
    if (!endNode && pos + len >= end) {
      endNode = node as Text;
      endOff = end - pos;
    }
    if (startNode && endNode) break;
    pos += len;
    node = walker.nextNode();
  }
  if (!startNode || !endNode) return null;
  const range = document.createRange();
  range.setStart(startNode, startOff);
  range.setEnd(endNode, endOff);
  return range;
}
