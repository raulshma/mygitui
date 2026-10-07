<script lang="ts">
  /** Wave-2 stub. Contract (owned by B3 lane): owns the log stream + filter
   *  bar + GraphCanvas + commit detail pane. */
  let { repoId }: { repoId: string } = $props();
</script>

<section aria-label="Commit history">
  <p>History for {repoId}</p>
</section>
