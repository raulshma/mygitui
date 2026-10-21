/**
 * Anchored overlay helper for popovers, inline dialogs and menus.
 *
 * Anchored panels used to live inside their trigger's container, so any
 * `overflow: hidden` ancestor clipped them (every SplitPane clips, which cut
 * the AI opt-in dialog off at the sidebar edge). This action moves the node
 * under <body>, keeps it `position: fixed`, flips it to the side with room,
 * and clamps it to the viewport. It repositions on scroll/resize and when the
 * panel itself changes size, so it can never be cropped by an ancestor.
 */

export interface FloatingOptions {
  /**
   * Element the panel is anchored to. Defaults to the node's parent at mount
   * time (captured before the node is moved to <body>).
   */
  anchor?: HTMLElement | undefined;
  /** Prefer opening above the anchor; flips below when there is no room. */
  above?: boolean;
  /** Align the panel's inline-end edge with the anchor's (right-align). */
  alignEnd?: boolean;
  /** Gap between the anchor and the panel, in px. */
  gap?: number;
  /** Minimum distance from the viewport edges, in px. */
  margin?: number;
}

const DEFAULT_GAP = 4;
const DEFAULT_MARGIN = 8;

export function floating(node: HTMLElement, options: FloatingOptions) {
  let opts = options;
  const fallbackAnchor = node.parentElement;
  let frame = 0;

  function place(): void {
    frame = 0;
    const anchor = opts.anchor ?? fallbackAnchor;
    if (!anchor?.isConnected) {
      node.style.visibility = "hidden";
      return;
    }

    const a = anchor.getBoundingClientRect();
    const b = node.getBoundingClientRect();
    const gap = opts.gap ?? DEFAULT_GAP;
    const margin = opts.margin ?? DEFAULT_MARGIN;

    // Vertical: preferred side first, fall back to the other side, then
    // clamp so a tall panel still starts inside the viewport.
    let top = opts.above ? a.top - gap - b.height : a.bottom + gap;
    const overflows = opts.above
      ? top < margin
      : top + b.height > window.innerHeight - margin;
    if (overflows) {
      top = opts.above ? a.bottom + gap : a.top - gap - b.height;
    }
    top = Math.max(margin, Math.min(top, window.innerHeight - b.height - margin));

    // Horizontal: start-aligned by default; end-aligned for triggers docked
    // to the right (the commit-bar sparkle). Shift into the viewport after.
    let left = opts.alignEnd ? a.right - b.width : a.left;
    left = Math.max(margin, Math.min(left, window.innerWidth - b.width - margin));

    node.style.top = `${Math.round(top)}px`;
    node.style.left = `${Math.round(left)}px`;
    node.style.visibility = "visible";
  }

  function schedule(): void {
    if (frame !== 0) return;
    frame = requestAnimationFrame(place);
  }

  const observer =
    typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
  observer?.observe(node);
  window.addEventListener("resize", schedule);
  // Capture scrolls from any scroll container between the anchor and <body>.
  window.addEventListener("scroll", schedule, true);

  node.style.visibility = "hidden";
  document.body.appendChild(node);
  place();

  return {
    update(next: FloatingOptions): void {
      opts = next;
      place();
    },
    destroy(): void {
      cancelAnimationFrame(frame);
      observer?.disconnect();
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      node.remove();
    },
  };
}
