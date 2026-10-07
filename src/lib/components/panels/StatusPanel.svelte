<script lang="ts">
  /** Wave-2 stub. Contract (owned by B4 lane): */
  import type { RepoStatus, StatusEntry } from "$lib/ipc/types";

  let {
    status,
    onOpenDiff,
    selected = $bindable([]),
  }: {
    status: RepoStatus | null;
    onOpenDiff: (entry: StatusEntry) => void;
    selected: string[];
  } = $props();
</script>

<aside aria-label="Working copy">
  {#if status}
    <p>{status.entries.length} entries</p>
  {:else}
    <p>Loading status…</p>
  {/if}
</aside>
