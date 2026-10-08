<script lang="ts">
  /**
   * GraphCanvas — full-viewport canvas commit graph with virtual scrolling.
   *
   * The backend pre-lays-out every LogPage (lane index per commit, edges to
   * the next row); this component only RENDERS. Scrolling is a native
   * overflow container + tall spacer div (wheel / drag / touch / scrollbar /
   * keyboard all come for free) with a sticky canvas that paints only the
   * visible rows ± `overscan`, so cost per frame is independent of the total
   * commit count (250k-commit repos scroll at 60fps).
   *
   * Props contract (B2, additive-optional beyond the required four):
   *   pages               cumulative LogPage[] — flattened incrementally,
   *                       never re-flattened per scroll frame (see rows.ts)
   *   rowHeight           fixed row height in CSS px (default 24)
   *   onCommitClick(sha)  fired when a lane node/channel is clicked or Enter
   *                       is pressed on the focused row
   *   onReachEnd()        fired while fewer than `reachEndThresholdRows` rows
   *                       remain after the visible window (parent streams
   *                       more pages; may fire repeatedly until buffered)
   *   onVisibleRowsChange(first, last)  [additive] the drawn row window
   *                       `[first, last)` INCLUDING overscan — HistoryView
   *                       syncs its DOM commit list from this
   *   headSha             [additive] HEAD commit sha → highlight band + ring
   *   laneWidth / padding [additive] lane column geometry (14 / 10 px)
   *   overscan            [additive] rows drawn beyond each viewport edge (20)
 *   nodeTolerance       [additive] hit tolerance around lane columns (6 px)
 *   reachEndThresholdRows [additive] onReachEnd lookahead in rows (500)
 *   bookmarks           [additive, M7] bookmarked commit shas → small dashed
 *                       ring marker on those nodes (display-only in M7)
 *   branchColors        [additive, M7] (refs: string[]) => color | null —
 *                       resolved per drawn row against the row commit's ref
 *                       decorations; a hit recolors the row's node + edges
 *                       instead of the lane palette
   *
   * Imperative API (via bind:this):
   *   scrollToRow(index: number): void — centers row `index` in the viewport
   *   (clamped to the scrollable range). Row stays put; does not steal focus.
   *
   * A11y: role="img" with "Commit graph, N commits"; ArrowUp/ArrowDown move
   * the focused row (with a drawn focus ring), Enter clicks it.
   */
  import { onDestroy } from "svelte";
  import type { LogPage } from "$lib/ipc/types";
  import { applyDprTransform, pixelSize, scrollToRowTarget, visibleRange } from "./layout";
  import { hitTest } from "./hitTest";
  import { FALLBACK_THEME, readTheme, type ThemeColors } from "./palette";
  import { drawGraph } from "./render";
  import { RowCache } from "./rows";

  interface Props {
    pages: LogPage[];
    rowHeight?: number;
    onCommitClick: (sha: string) => void;
    onReachEnd: () => void;
    onVisibleRowsChange?: (first: number, last: number) => void;
    headSha?: string | null;
    laneWidth?: number;
    padding?: number;
    overscan?: number;
    nodeTolerance?: number;
    reachEndThresholdRows?: number;
    /** M7: shas of bookmarked commits (small dashed ring marker). */
    bookmarks?: ReadonlySet<string>;
    /** M7: refs decorations → branch color override (null = lane palette). */
    branchColors?: (refs: string[]) => string | null;
  }

  let {
    pages,
    rowHeight = 24,
    onCommitClick,
    onReachEnd,
    onVisibleRowsChange,
    headSha = null,
    laneWidth = 14,
    padding = 10,
    overscan = 20,
    nodeTolerance = 6,
    reachEndThresholdRows = 500,
    bookmarks = undefined,
    branchColors = undefined,
  }: Props = $props();

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  const cache = new RowCache();

  let scrollEl: HTMLDivElement | undefined = $state();
  let canvasEl: HTMLCanvasElement | undefined = $state();
  let viewport = $state({ w: 0, h: 0 });
  let dpr = $state(1);
  let scrollTop = $state(0);
  let rowCount = $state(0);
  let focusRow = $state(0);
  let hasFocus = $state(false);
  /** Row/lane under the pointer (drives cursor + redraw); null when off-rows. */
  let hover: { row: number; lane: number | null } | null = $state(null);
  /** Raw pointer position (canvas-local) — moves the tooltip without redraws. */
  let tipPos = $state({ x: 0, y: 0 });
  let theme: ThemeColors = $state(FALLBACK_THEME);

  const totalHeight = $derived(rowCount * rowHeight);
  const visible = $derived(visibleRange(scrollTop, viewport.h, rowCount, rowHeight, overscan));
  // Reading `rowCount` makes these recompute after every data sync.
  const headRow = $derived(headSha && rowCount > 0 ? cache.indexOfSha(headSha) : -1);
  const safeFocusRow = $derived(rowCount === 0 ? -1 : Math.min(focusRow, rowCount - 1));

  const tip = $derived.by(() => {
    if (!hover || hover.row < 0) return null;
    void rowCount; // invalidate when the flattened list changes
    const commit = cache.commitAt(hover.row);
    if (!commit) return null;
    const x = Math.min(Math.max(8, tipPos.x + 14), Math.max(8, viewport.w - 268));
    const y = Math.min(Math.max(4, tipPos.y + 18), Math.max(4, viewport.h - 48));
    return { x, y, summary: commit.summary, sha: commit.sha.slice(0, 10) };
  });

  // ---------------------------------------------------------------------------
  // Data / notifications
  // ---------------------------------------------------------------------------

  /** Flatten incrementally; `rowCount` bumps trigger every dependent effect. */
  $effect(() => {
    if (cache.update(pages)) rowCount = cache.length;
  });

  /** Parent (HistoryView) syncs its DOM commit list to this window. */
  $effect(() => {
    const { first, last } = visible;
    onVisibleRowsChange?.(first, last);
  });

  /** Infinite scroll: fire while the buffer after the window is thin. */
  $effect(() => {
    if (rowCount === 0) return;
    const { last } = visible;
    if (rowCount - last < reachEndThresholdRows) onReachEnd();
  });

  // ---------------------------------------------------------------------------
  // Viewport sizing (ResizeObserver + DPR)
  // ---------------------------------------------------------------------------

  function syncViewport(): void {
    const el = scrollEl;
    if (!el) return;
    const w = el.clientWidth;
    const h = el.clientHeight;
    if (w !== viewport.w || h !== viewport.h) viewport = { w, h };
    const ratio = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
    if (ratio !== dpr) dpr = ratio;
  }

  $effect(() => {
    const el = scrollEl;
    if (!el) return;
    syncViewport();
    let ro: ResizeObserver | undefined;
    if (typeof ResizeObserver !== "undefined") {
      ro = new ResizeObserver(() => syncViewport());
      ro.observe(el);
    }
    const onWindowResize = () => syncViewport();
    window.addEventListener("resize", onWindowResize);
    return () => {
      ro?.disconnect();
      window.removeEventListener("resize", onWindowResize);
    };
  });

  /**
   * Re-read M3 tokens once per theme change (never per frame). Theme swaps
   * happen via `prefers-color-scheme` (initTheme rewrites the tokens) and via
   * the initial initTheme() apply — both covered by the media listener plus a
   * MutationObserver on <html>'s inline style, where initTheme writes tokens.
   */
  $effect(() => {
    const read = () => {
      theme = readTheme(
        typeof document === "undefined" ? null : getComputedStyle(document.documentElement),
      );
    };
    read();
    const disposers: Array<() => void> = [watchMedia("(prefers-color-scheme: dark)", read)];
    if (typeof MutationObserver !== "undefined") {
      const mo = new MutationObserver(read);
      mo.observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });
      disposers.push(() => mo.disconnect());
    }
    return () => disposers.forEach((dispose) => dispose());
  });

  /** Track device-pixel-ratio changes (zoom / monitor moves). */
  $effect(() => {
    const sync = () => {
      const ratio = window.devicePixelRatio || 1;
      if (ratio !== dpr) dpr = ratio;
    };
    const stop = watchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`, sync);
    window.addEventListener("resize", sync);
    return () => {
      stop();
      window.removeEventListener("resize", sync);
    };
  });

  function watchMedia(query: string, onChange: () => void): () => void {
    try {
      if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
        return () => {};
      }
      const mq = window.matchMedia(query);
      if (typeof mq.addEventListener === "function") {
        mq.addEventListener("change", onChange);
        return () => mq.removeEventListener("change", onChange);
      }
      if (typeof mq.addListener === "function") {
        // Legacy Safari < 14.
        mq.addListener(onChange);
        return () => mq.removeListener(onChange);
      }
    } catch {
      // MediaQuery failures must never break rendering.
    }
    return () => {};
  }

  // ---------------------------------------------------------------------------
  // Painting
  // ---------------------------------------------------------------------------

  $effect(() => {
    const canvas = canvasEl;
    if (!canvas || viewport.w <= 0 || viewport.h <= 0) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const size = pixelSize(viewport.w, viewport.h, dpr);
    if (canvas.width !== size.w) canvas.width = size.w;
    if (canvas.height !== size.h) canvas.height = size.h;
    applyDprTransform(ctx, dpr);
    const { first, last } = visible;
    // M7 branch colors: resolve the override per drawn row from the row
    // commit's ref decorations (first matching rule wins — see RepoView).
    let colorOverrides: Map<number, string> | undefined;
    if (branchColors) {
      colorOverrides = new Map();
      for (let i = first; i < last; i += 1) {
        const commit = cache.commitAt(i);
        if (!commit || commit.refs.length === 0) continue;
        const hit = branchColors(commit.refs);
        if (hit) colorOverrides.set(i, hit);
      }
    }
    drawGraph(ctx, {
      rows: cache.rows,
      first,
      last,
      scrollTop,
      width: viewport.w,
      height: viewport.h,
      rowHeight,
      laneWidth,
      padding,
      palette: theme.lanes,
      surface: theme.surface,
      outline: theme.outline,
      hoverRow: hover?.row ?? -1,
      headRow,
      focusRow: safeFocusRow,
      focusRing: hasFocus,
      ...(colorOverrides ? { colorOverrides } : {}),
      ...(bookmarks ? { bookmarks } : {}),
    });
  });

  // ---------------------------------------------------------------------------
  // Scrolling (native overflow container; state synced on rAF)
  // ---------------------------------------------------------------------------

  interface FrameHandle {
    cancel: () => void;
  }

  function onFrame(callback: () => void): FrameHandle {
    if (typeof requestAnimationFrame === "function") {
      const id = requestAnimationFrame(callback);
      return { cancel: () => cancelAnimationFrame(id) };
    }
    const id = setTimeout(callback, 16);
    return { cancel: () => clearTimeout(id) };
  }

  let scrollFrame: FrameHandle | null = null;

  function onScroll(): void {
    if (scrollFrame) return;
    const el = scrollEl;
    if (!el) return;
    scrollFrame = onFrame(() => {
      scrollFrame = null;
      scrollTop = el.scrollTop;
      syncViewport(); // scrollbar appearing/disappearing changes clientWidth
      if (lastPointer) applyHover(lastPointer);
    });
  }

  // ---------------------------------------------------------------------------
  // Pointer (hover throttled to 30ms, tooltip, click hit-testing)
  // ---------------------------------------------------------------------------

  let lastPointer: { x: number; y: number } | null = null;
  let lastMoveAt = 0;
  let trailingTimer: ReturnType<typeof setTimeout> | undefined;
  let pendingMove: { x: number; y: number } | null = null;

  function applyHover(local: { x: number; y: number }): void {
    if (!cache.length) {
      if (hover) hover = null;
      return;
    }
    const hit = hitTest({
      x: local.x,
      y: local.y,
      scrollTop: scrollEl?.scrollTop ?? scrollTop,
      rows: cache.rows,
      rowHeight,
      laneWidth,
      padding,
      tolerance: nodeTolerance,
    });
    tipPos = { x: local.x, y: local.y };
    const next = hit.row >= 0 ? { row: hit.row, lane: hit.lane } : null;
    if ((next?.row ?? -1) !== (hover?.row ?? -1) || (next?.lane ?? null) !== (hover?.lane ?? null)) {
      hover = next;
    }
  }

  function onPointerMove(event: PointerEvent): void {
    const canvas = canvasEl;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const local = { x: event.clientX - rect.left, y: event.clientY - rect.top };
    lastPointer = local;
    const now = performance.now();
    if (now - lastMoveAt >= 30) {
      lastMoveAt = now;
      pendingMove = null;
      applyHover(local);
    } else if (trailingTimer === undefined) {
      pendingMove = local;
      trailingTimer = setTimeout(() => {
        trailingTimer = undefined;
        lastMoveAt = performance.now();
        if (pendingMove) applyHover(pendingMove);
        pendingMove = null;
      }, 32);
    } else {
      pendingMove = local;
    }
  }

  function onPointerLeave(): void {
    lastPointer = null;
    pendingMove = null;
    hover = null;
  }

  function onClick(event: MouseEvent): void {
    const canvas = canvasEl;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const hit = hitTest({
      x: event.clientX - rect.left,
      y: event.clientY - rect.top,
      scrollTop: scrollEl?.scrollTop ?? 0,
      rows: cache.rows,
      rowHeight,
      laneWidth,
      padding,
      tolerance: nodeTolerance,
    });
    if (hit.row < 0) return;
    focusRow = hit.row;
    canvas.focus();
    if (hit.lane !== null) {
      const sha = cache.rows[hit.row]?.sha;
      if (sha) onCommitClick(sha);
    }
  }

  // ---------------------------------------------------------------------------
  // Keyboard (arrow scrolling, Enter = click focused row)
  // ---------------------------------------------------------------------------

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const dir = event.key === "ArrowDown" ? 1 : -1;
      focusRow = Math.min(Math.max(focusRow + dir, 0), Math.max(rowCount - 1, 0));
      const el = scrollEl;
      if (!el) return;
      const top = focusRow * rowHeight;
      const bottom = top + rowHeight;
      if (top < el.scrollTop) el.scrollTop = top;
      else if (bottom > el.scrollTop + el.clientHeight) el.scrollTop = bottom - el.clientHeight;
    } else if (event.key === "Enter") {
      const sha = safeFocusRow >= 0 ? cache.rows[safeFocusRow]?.sha : undefined;
      if (sha) onCommitClick(sha);
    }
  }

  // ---------------------------------------------------------------------------
  // Imperative API (bind:this)
  // ---------------------------------------------------------------------------

  /** Center row `index` in the viewport (clamped to the scrollable range). */
  export function scrollToRow(index: number): void {
    const el = scrollEl;
    if (!el) return;
    const target = scrollToRowTarget(index, cache.length, rowHeight, el.clientHeight);
    if (el.scrollTop !== target) el.scrollTop = target;
  }

  onDestroy(() => {
    scrollFrame?.cancel();
    scrollFrame = null;
    if (trailingTimer !== undefined) clearTimeout(trailingTimer);
    trailingTimer = undefined;
  });
</script>

<div class="gc-root">
  <div bind:this={scrollEl} class="gc-scroll" onscroll={onScroll}>
    <!-- Tall spacer drives the native scrollbar; height = rows × rowHeight. -->
    <div class="gc-spacer" style:height="{totalHeight}px">
      <!-- Sticky viewport: stays pinned while the spacer scrolls past. -->
      <div class="gc-sticky" style:height="{viewport.h}px">
        <!-- svelte-ignore a11y_no_interactive_element_to_noninteractive_role -->
        <canvas
          bind:this={canvasEl}
          class="gc-canvas"
          role="img"
          aria-label={"Commit graph, " + rowCount + (rowCount === 1 ? " commit" : " commits")}
          tabindex="0"
          style:width="{viewport.w}px"
          style:height="{viewport.h}px"
          style:cursor={hover && hover.lane !== null ? "pointer" : "default"}
          onpointermove={onPointerMove}
          onpointerleave={onPointerLeave}
          onclick={onClick}
          onkeydown={onKeydown}
          onfocus={() => (hasFocus = true)}
          onblur={() => (hasFocus = false)}
        ></canvas>
        {#if tip}
          <div class="gc-tip" role="tooltip" style:left="{tip.x}px" style:top="{tip.y}px">
            <span class="gc-tip-summary">{tip.summary}</span>
            <span class="gc-tip-sha">{tip.sha}</span>
          </div>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .gc-root {
    position: relative;
    display: block;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }

  /* Native scrolling: wheel, drag, touch and the scrollbar come for free. */
  .gc-scroll {
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .gc-spacer {
    position: relative;
    min-height: 100%;
  }

  .gc-sticky {
    position: sticky;
    top: 0;
    overflow: hidden;
  }

  .gc-canvas {
    display: block;
    outline: none; /* focus is drawn on-canvas as a row ring */
  }

  .gc-tip {
    position: absolute;
    z-index: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-width: 260px;
    padding: 4px 8px;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-inverse-surface, #322f35);
    color: var(--m3-inverse-on-surface, #f5eff7);
    font-size: 12px;
    line-height: 1.35;
    box-shadow: var(--m3-elevation-1, none);
    pointer-events: none;
  }

  .gc-tip-summary {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .gc-tip-sha {
    font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
    opacity: 0.8;
  }
</style>
