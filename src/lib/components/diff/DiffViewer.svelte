<script lang="ts">
  /** Wave-2 stub. Contract (owned by B1 lane): */
  import type { FileDiff } from "$lib/ipc/types";

  let {
    files,
    mode = "split",
  }: {
    files: FileDiff[];
    mode?: "split" | "unified";
  } = $props();
</script>

<div class="diff-viewer" aria-label="Diff viewer">
  <p>{files.length} files</p>
</div>
