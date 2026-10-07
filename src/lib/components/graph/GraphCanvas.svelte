<script lang="ts">
  /** Wave-2 stub. Contract (owned by B2 lane): */
  import type { LogPage } from "$lib/ipc/types";

  let {
    pages,
    rowHeight = 24,
    onCommitClick,
    onReachEnd,
  }: {
    pages: LogPage[];
    rowHeight?: number;
    onCommitClick: (sha: string) => void;
    onReachEnd: () => void;
  } = $props();
</script>

<canvas aria-label="Commit graph"></canvas>
