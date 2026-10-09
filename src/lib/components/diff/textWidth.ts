/**
 * Pixel measurement for diff line text (the scroller's horizontal extent).
 *
 * The row model knows the widest line as a *string*; the scroller needs
 * pixels. Canvas measureText with the exact row font stack is exact (wide
 * glyphs included), and tab characters advance to 8-column stops to match
 * the rows' `white-space: pre; tab-size: 8` rendering. jsdom has no canvas —
 * the fallback estimates from visual column count, which is all tests need.
 */

/** Must match the row text style in DiffRow.svelte (font + tab-size). */
const MONO_FONT = '12px ui-monospace, "Cascadia Mono", Consolas, "Courier New", monospace';
const TAB_SIZE = 8;

/** Fallback advance per column when no canvas exists (≈ 12px monospace). */
const FALLBACK_CH_PX = 7.2;

let ctx: CanvasRenderingContext2D | null = null;
let ctxTried = false;

function measurer(): CanvasRenderingContext2D | null {
  if (ctxTried) return ctx;
  ctxTried = true;
  try {
    const candidate = document.createElement("canvas").getContext("2d");
    if (candidate && typeof candidate.measureText === "function") {
      candidate.font = MONO_FONT;
      ctx = candidate;
    }
  } catch {
    ctx = null;
  }
  return ctx;
}

const cache = new Map<string, number>();

/** Rendered width of one diff line in px (tab stops included). */
export function measureLineWidth(text: string): number {
  const hit = cache.get(text);
  if (hit !== undefined) return hit;
  const px = computeWidth(text);
  if (cache.size > 512) cache.clear();
  cache.set(text, px);
  return px;
}

function computeWidth(text: string): number {
  const c = measurer();
  const ch = c ? c.measureText("0").width || FALLBACK_CH_PX : FALLBACK_CH_PX;
  const widthOf = (s: string): number =>
    c ? c.measureText(s).width : visualColumns(s) * FALLBACK_CH_PX;
  let px = 0;
  let col = 0;
  let i = 0;
  while (i < text.length) {
    const tab = text.indexOf("\t", i);
    const end = tab === -1 ? text.length : tab;
    px += widthOf(text.slice(i, end));
    col += end - i;
    if (tab === -1) break;
    const next = (Math.floor(col / TAB_SIZE) + 1) * TAB_SIZE;
    px += (next - col) * ch;
    col = next;
    i = tab + 1;
  }
  return px;
}

/** Visual columns of `text`: each tab advances to the next `tabSize` stop. */
export function visualColumns(text: string, tabSize = TAB_SIZE): number {
  let col = 0;
  for (const ch of text) {
    col = ch === "\t" ? (Math.floor(col / tabSize) + 1) * tabSize : col + 1;
  }
  return col;
}
