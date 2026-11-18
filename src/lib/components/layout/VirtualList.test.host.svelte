<!--
  Test-only host for VirtualList: a parent that passes the row/empty
  snippets (snippet props cannot be built from plain JS tests) and hands
  the instance's scroll API to the test via exported delegates (component
  exports are reachable from `mount`, snippet props are not). Used
  exclusively by VirtualList.svelte.test.ts.
-->
<script lang="ts">
  import VirtualList from "./VirtualList.svelte";

  export interface TestItem {
    id: string;
  }

  let {
    items,
    rowHeight = 24,
    overscan = 12,
    ariaRole = undefined,
    ariaLabel = undefined,
    scrollerProps = {},
  }: {
    items: TestItem[];
    rowHeight?: number;
    overscan?: number;
    ariaRole?: string | undefined;
    ariaLabel?: string | undefined;
    scrollerProps?: {
      role?: string;
      "aria-label"?: string;
      tabindex?: number;
    };
  } = $props();

  let list: {
    scrollToIndex(index: number, align?: "nearest" | "center"): void;
    viewportSize(): number;
    viewportScrollTop(): number;
    element(): HTMLDivElement | null;
  } | undefined = $state();

  /** Delegates: the host's scrollToIndex(index, align?). */
  export function scrollToIndex(
    index: number,
    align?: "nearest" | "center",
  ): void {
    list?.scrollToIndex(index, align);
  }

  /** Delegates: the host's viewportSize(). */
  export function viewportSize(): number {
    return list?.viewportSize() ?? 0;
  }

  /** Delegates: the host's viewportScrollTop(). */
  export function viewportScrollTop(): number {
    return list?.viewportScrollTop() ?? 0;
  }

  /** The list's scrolling viewport element (stub target in tests). */
  export function scroller(): HTMLElement | null {
    return list?.element() ?? null;
  }
</script>

<VirtualList
  bind:this={list}
  {items}
  {rowHeight}
  {overscan}
  {ariaRole}
  {ariaLabel}
  {scrollerProps}
>
  {#snippet row(item: TestItem, index: number)}
    <span class="test-row" data-index={index} data-id={item.id}>{item.id}</span>
  {/snippet}
</VirtualList>
