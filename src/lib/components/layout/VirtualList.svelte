<script lang="ts" generics="T">
  /**
   * Shared uniform-height virtual list — the spacer + absolutely-positioned
   * window pattern (BlameView's markup, FilesPanel's viewport tracking),
   * with the geometry taken from the graph's shared `visibleRange`.
   *
   * Props: `items`, uniform `rowHeight` (a plain number — every current
   * consumer is uniform; per-row `rowHeight(i)` support can slot in behind
   * the same math later), `overscan`, `ariaRole`/`ariaLabel` for the row
   * container, `row` snippet receiving `(item, index)`, optional
   * `header`/`footer` snippets around the windowed list, and an `empty`
   * snippet rendered in place of the window when `items` is empty.
   *
   * Selection/focus passthrough: the wrapper only positions the row; the
   * `row` snippet provides everything row-specific (ids, roles,
   * aria-selected, tabindex, handlers), so consumers keep their exact
   * aria trees and keyboard models. The SCROLLER itself takes consumer
   * attributes via `scrollerClass`/`scrollerProps` — FilesPanel's
   * aria-activedescendant tree needs role/tabindex/keydown on the scroller,
   * while BlameView only needs a plain overflow-auto pane.
   *
   * Keyboard consumers scroll programmatically through the instance
   * (bind:this): `scrollToIndex(index, align)` (the old hand-rolled
   * edge-clamping math), `viewportSize()` for PageUp/Down distances and
   * `viewportScrollTop()`/`element()` for the odd direct checks.
   *
   * NOT migrated this pass (follow-ups): StatusPanel and AgentChanges use
   * per-section windows with interleaved sticky headers (entangled with
   * selection/remap specifics), HistoryView scroll-syncs with the graph
   * canvas, DiffViewer/GraphCanvas are bespoke canvas surfaces, and
   * BranchPanel pages ("Show all N") rather than virtualizing.
   */
  import type { Snippet } from "svelte";
  import { visibleRange } from "$lib/components/graph/layout";

  interface Props {
    items: T[];
    /** Uniform row height in px (spacer math and row boxes). */
    rowHeight: number;
    /** Rows rendered beyond each viewport edge (default 12). */
    overscan?: number;
    /** Role/label for the row container (list/listbox/…). */
    ariaRole?: string | undefined;
    ariaLabel?: string | undefined;
    /** One row: `(item, index)` — index is the flat position. */
    row: Snippet<[T, number]>;
    header?: Snippet;
    footer?: Snippet;
    /** Rendered instead of the window when `items` is empty. */
    empty?: Snippet;
    /** Extra classes for the scrolling viewport. */
    scrollerClass?: string;
    /** Attributes/handlers spread onto the scrolling viewport. */
    scrollerProps?: Record<string, unknown>;
  }

  let {
    items,
    rowHeight,
    overscan = 12,
    ariaRole = undefined,
    ariaLabel = undefined,
    row,
    header = undefined,
    footer = undefined,
    empty = undefined,
    scrollerClass = "",
    scrollerProps = {},
  }: Props = $props();

  let viewport: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewportHeight = $state(0);

  /** Tracks the scroller's viewport (rAF-free scroll/resize updates). */
  function trackViewport(node: HTMLDivElement) {
    viewport = node;
    const update = () => {
      scrollTop = node.scrollTop;
      // jsdom has no layout; a 1px floor keeps the window renderable in
      // tests (same guard as the panels this component replaces).
      viewportHeight = Math.max(1, node.clientHeight);
    };
    const observer = new ResizeObserver(update);
    observer.observe(node);
    node.addEventListener("scroll", update, { passive: true });
    update();
    return {
      destroy() {
        observer.disconnect();
        node.removeEventListener("scroll", update);
        if (viewport === node) viewport = undefined;
      },
    };
  }

  const range = $derived(
    visibleRange(scrollTop, viewportHeight, items.length, rowHeight, overscan),
  );
  const visible = $derived(items.slice(range.first, range.last));

  /**
   * Scrolls so row `index` is visible: "nearest" clamps to the edges (the
   * selectAndScroll math), "center" centers the row (the reveal math).
   */
  export function scrollToIndex(
    index: number,
    align: "nearest" | "center" = "nearest",
  ): void {
    const node = viewport;
    if (!node) return;
    const clamped = Math.min(
      Math.max(0, Math.round(index)),
      Math.max(0, items.length - 1),
    );
    const top = clamped * rowHeight;
    if (align === "center") {
      node.scrollTop = Math.max(0, top + rowHeight / 2 - node.clientHeight / 2);
      return;
    }
    const viewTop = node.scrollTop;
    const viewBottom = viewTop + node.clientHeight;
    if (top < viewTop) node.scrollTop = top;
    else if (top + rowHeight > viewBottom) {
      node.scrollTop = top + rowHeight - node.clientHeight;
    }
  }

  /** The scroller's client height (PageUp/Down row distances). */
  export function viewportSize(): number {
    return viewport?.clientHeight ?? 0;
  }

  /** The scroller's current scroll offset (out-of-view checks). */
  export function viewportScrollTop(): number {
    return viewport?.scrollTop ?? 0;
  }

  /** The scroller element itself (focus hand-off between panes). */
  export function element(): HTMLDivElement | null {
    return viewport ?? null;
  }
</script>

<div
  class="vl-scroller {scrollerClass}"
  {...scrollerProps}
  use:trackViewport
>
  {#if header}
    {@render header()}
  {/if}
  {#if items.length === 0 && empty}
    {@render empty()}
  {:else}
    <div
      class="vl-rows relative w-max min-w-full"
      style:height={`${items.length * rowHeight}px`}
      role={ariaRole}
      aria-label={ariaLabel}
    >
      {#each visible as item, i (range.first + i)}
        {@const index = range.first + i}
        <div
          class="vl-row absolute inset-x-0"
          style:top={`${index * rowHeight}px`}
          style:height={`${rowHeight}px`}
        >
          {@render row(item, index)}
        </div>
      {/each}
    </div>
  {/if}
  {#if footer}
    {@render footer()}
  {/if}
</div>
