<script lang="ts">
  /**
   * PopoutDiff — the `?panel=diff` popout window's whole content.
   *
   * The in-workspace diff pane shows the selected file's diff streamed by
   * RepoView; a popped-out window has no RepoView state, so this component
   * wires its own: it streams the full working-tree diff (HEAD → worktree)
   * on mount, re-streams when the backend announces repo changes, and via
   * the Refresh button. Renders DiffViewer alone, filling the window.
   */
  import { streamDiff } from "$lib/ipc/client";
  import type { FileDiff, RepoId } from "$lib/ipc/types";
  import { onRepoChange } from "$lib/events/repoEvents";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { Button } from "$lib/components/ui/button/index.js";

  let { repoId }: { repoId: RepoId } = $props();

  let files = $state<FileDiff[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  /** Generation counter; bumping re-triggers the streaming effect. */
  let reload = $state(0);

  $effect(() => {
    const id = repoId;
    const generation = reload;
    // Stream the whole worktree diff; pages accumulate into `files`.
    files = [];
    loading = true;
    error = null;
    const run = streamDiff(id, "head", "worktree", (page) => {
      files = [...files, ...page];
    })
      .then(() => {
        if (generation === reload) loading = false;
      })
      .catch((err: unknown) => {
        if (generation !== reload) return;
        loading = false;
        error = err instanceof Error ? err.message : String(err);
      });
    return () => {
      void run;
    };
  });

  // The popout has no tab store; register with the shared event module.
  // Policy: this pane streams the whole worktree diff, so ANY backend change
  // for this repo (workdir churn, head moves, resyncs) may alter it.
  $effect(() => {
    const id = repoId;
    return onRepoChange({
      filter: (change) => change.repo_id === id,
      handle: () => {
        reload += 1;
      },
    });
  });
</script>

<section
  class="popout-diff flex h-screen min-h-0 flex-col bg-card text-card-foreground"
>
  <header
    class="bar flex flex-none items-center gap-2 border-b border-border bg-popover px-3 py-[0.3rem] text-[0.8125rem]"
  >
    <span class="title font-medium">Working tree diff</span>
    <span class="spacer flex-1"></span>
    {#if loading}
      <span class="state text-muted-foreground" role="status">Loading…</span>
    {:else if error}
      <span class="state err text-destructive" role="alert">{error}</span>
    {:else}
      <span class="state text-muted-foreground" role="status"
        >{files.length} file{files.length === 1 ? "" : "s"}</span
      >
    {/if}
    <Button
      class="refresh h-auto flex-none rounded-full px-[0.6rem] py-[0.1rem] text-[0.72rem] hover:bg-accent hover:text-card-foreground focus-visible:bg-accent focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 focus-visible:ring-0"
      variant="outline"
      size="sm"
      type="button"
      onclick={() => (reload += 1)}
    >
      Refresh
    </Button>
  </header>

  {#if error && files.length === 0}
    <p class="empty px-4 py-8 text-center text-muted-foreground">
      Could not load the diff.
    </p>
  {:else}
    <div class="viewer flex min-h-0 flex-1 flex-col [&>*]:min-h-0 [&>*]:flex-1">
      <DiffViewer {files} />
    </div>
  {/if}
</section>
