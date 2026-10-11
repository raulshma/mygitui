/**
 * Two-way scroll sync between the history view's DOM commit list and
 * GraphCanvas's native scroll container.
 *
 * The two panes keep their scrollTops in lockstep instead of sharing one
 * scroller: the visible DOM-row window IS the canvas's reported window
 * (`onVisibleRowsChange`), the DOM pane hides its own gutter, and the
 * canvas's scrollbar effectively drives both. Wheel over either pane,
 * keyboard nav and programmatic scrolls all converge through
 * equality-guarded pushes (a ≥ 1px delta is required on both directions —
 * no feedback loop).
 *
 * Three explicit strategies (the original inline choreography, preserved):
 *  - fast path — the canvas exposes its scroll container
 *    (`GraphCanvas.bind:scrollEl`); both sides mirror raw scrollTop values;
 *  - fallback — with no scroll element (canvas not mounted yet) the canvas's
 *    exported `scrollToRow(index)` centers the corresponding row instead;
 *  - degraded local-window mode — with no canvas window reported, the render
 *    window is computed locally from the list scroller's scrollTop.
 *
 * Everything runs through injected ports (getters/setters over plain
 * objects — no real DOM), so the whole machine is unit-testable with fake
 * element objects. The two window states stay view-owned (`$state`, they
 * drive rendering) and are read/written through the `ownRange`/
 * `canvasWindow` ports, which keeps the view's `$derived` render window
 * reactive.
 */

/** A half-open render window `[start, end)` of commit rows. */
export interface ScrollWindow {
  start: number;
  end: number;
}

/** The canvas's reported drawn row window `[first, last)` (incl. overscan). */
export interface CanvasWindow {
  first: number;
  last: number;
}

/**
 * One scrollable side of the sync. `getScrollTop` returns null while the
 * element is unmounted (the port's "absent" signal — it drives strategy
 * selection); `getClientHeight` may be 0 while hidden.
 */
export interface ScrollEndpoint {
  getScrollTop(): number | null;
  setScrollTop(top: number): void;
  getClientHeight(): number;
}

/**
 * The graph side: a {@link ScrollEndpoint} when the fast path is live, plus
 * the declared imperative fallback (GraphCanvas `bind:this` export) used
 * when no scroll element is bound.
 */
export interface GraphScrollPort extends ScrollEndpoint {
  /** GraphCanvas's `scrollToRow(index)` — centers the row (B2 contract). */
  scrollToRow(index: number): void;
}

export interface HistoryScrollSyncDeps {
  /** The DOM commit-list scroller. */
  list: ScrollEndpoint;
  /** GraphCanvas's scroll container + `scrollToRow` fallback. */
  graph: GraphScrollPort;
  /** Fixed row height in CSS px — both panes share one grid. */
  rowHeight: number;
  /** Rows rendered beyond each viewport edge in the local window. */
  overscan: number;
  /** Rows currently available (clamps the local window). */
  totalRows(): number;
  /** View-owned local window (`$state`), read + written by the sync. */
  ownRange(): ScrollWindow;
  setOwnRange(range: ScrollWindow): void;
  /** View-owned canvas window (`$state` or null), read + written. */
  canvasWindow(): CanvasWindow | null;
  setCanvasWindow(window: CanvasWindow | null): void;
}

export interface HistoryScrollSync {
  /** Render window: the canvas's reported window when live, else our own. */
  renderWindow(): ScrollWindow;
  /** Recomputes the local window from the list port (no-op when unmounted). */
  updateOwnRange(): void;
  /**
   * List scroll handler: re-window, then mirror into the canvas. Returns the
   * render window AFTER the update (the caller's load-more input).
   */
  onListScroll(): ScrollWindow;
  /** Canvas reports its drawn window (incl. its overscan) — adopt + follow. */
  onCanvasVisible(first: number, last: number): void;
  /** Reveal a row (keyboard nav, restored selection) and mirror to canvas. */
  scrollToIndex(index: number): void;
  /** Re-target reset: forget both windows and zero both scrollers. */
  reset(): void;
}

export function createHistoryScrollSync(
  deps: HistoryScrollSyncDeps,
): HistoryScrollSync {
  /**
   * Mirrors the list's scrollTop into the canvas (exact when the fast path
   * is live and the values differ by ≥ 1px; else the centered-row fallback).
   */
  function pushListToCanvas(): void {
    const top = deps.list.getScrollTop();
    if (top === null) return;
    const graphTop = deps.graph.getScrollTop();
    if (graphTop !== null) {
      // Equality guard: a sub-pixel/no-op write would bounce straight back.
      if (Math.abs(graphTop - top) >= 1) deps.graph.setScrollTop(top);
    } else {
      // Fallback: center the row at the middle of our viewport.
      const centerRow = Math.floor(
        (top + deps.list.getClientHeight() / 2) / deps.rowHeight,
      );
      deps.graph.scrollToRow(centerRow);
    }
  }

  /** Render window: the canvas's reported window when live, else our own. */
  function renderWindow(): ScrollWindow {
    const canvas = deps.canvasWindow();
    return canvas && canvas.last > canvas.first
      ? { start: Math.max(0, canvas.first), end: canvas.last }
      : deps.ownRange();
  }

  /** Recomputes the local window from the list port (no-op when unmounted). */
  function updateOwnRange(): void {
    const top = deps.list.getScrollTop();
    const height = deps.list.getClientHeight();
    if (top === null || height === 0) return;
    const first = Math.floor(top / deps.rowHeight);
    const count = Math.ceil(height / deps.rowHeight);
    const start = Math.max(0, first - deps.overscan);
    const end = Math.min(deps.totalRows(), first + count + deps.overscan);
    const current = deps.ownRange();
    if (start !== current.start || end !== current.end) {
      deps.setOwnRange({ start, end });
    }
  }

  return {
    renderWindow,
    updateOwnRange,

    onListScroll(): ScrollWindow {
      updateOwnRange();
      pushListToCanvas();
      return renderWindow();
    },

    onCanvasVisible(first: number, last: number): void {
      deps.setCanvasWindow({ first, last });
      // The user scrolled the canvas directly — follow it exactly. Our
      // resulting scroll event pushes the same value back (no-op).
      const graphTop = deps.graph.getScrollTop();
      const listTop = deps.list.getScrollTop();
      if (
        graphTop !== null &&
        listTop !== null &&
        Math.abs(listTop - graphTop) >= 1
      ) {
        deps.list.setScrollTop(graphTop);
      }
    },

    scrollToIndex(index: number): void {
      const listTop = deps.list.getScrollTop();
      if (listTop === null) {
        // List unmounted: the canvas is all there is — center the row there.
        deps.graph.scrollToRow(index);
        return;
      }
      const top = index * deps.rowHeight;
      const height = deps.list.getClientHeight();
      if (top < listTop) {
        deps.list.setScrollTop(top);
      } else if (top + deps.rowHeight > listTop + height) {
        deps.list.setScrollTop(top + deps.rowHeight - height);
      }
      pushListToCanvas();
    },

    reset(): void {
      deps.setOwnRange({ start: 0, end: 0 });
      deps.setCanvasWindow(null);
      if (deps.list.getScrollTop() !== null) deps.list.setScrollTop(0);
      if (deps.graph.getScrollTop() !== null) deps.graph.setScrollTop(0);
    },
  };
}
