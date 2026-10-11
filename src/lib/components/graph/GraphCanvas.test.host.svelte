<!--
  Test-only host for GraphCanvas's `bind:scrollEl` contract: a parent that
  binds the canvas's native scroll container and hands it to the test via an
  exported getter (component exports are reachable from `mount`, props are
  not). Used exclusively by GraphCanvas.test.ts.
-->
<script lang="ts">
  import GraphCanvas from "./GraphCanvas.svelte";
  import type { LogPage } from "$lib/ipc/types";

  let { pages }: { pages: LogPage[] } = $props();

  let scrollEl = $state<HTMLDivElement | undefined>(undefined);

  /** The canvas's scroll container once mounted (undefined before). */
  export function getScrollEl(): HTMLDivElement | undefined {
    return scrollEl;
  }
</script>

<GraphCanvas {pages} onCommitClick={() => {}} onReachEnd={() => {}} bind:scrollEl={scrollEl} />
