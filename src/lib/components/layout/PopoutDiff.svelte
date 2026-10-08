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
  import { onRepoChanged, streamDiff } from "$lib/ipc/client";
  import type { FileDiff, RepoId } from "$lib/ipc/types";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";

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

  // The popout has no tab store; watch the backend directly.
  $effect(() => {
    const id = repoId;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    onRepoChanged((event) => {
      if (event.repo_id === id) reload += 1;
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // Popouts still work without the watcher; Refresh stays available.
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  });
</script>

<section class="popout-diff">
  <header class="bar">
    <span class="title">Working tree diff</span>
    <span class="spacer"></span>
    {#if loading}
      <span class="state" role="status">Loading…</span>
    {:else if error}
      <span class="state err" role="alert">{error}</span>
    {:else}
      <span class="state" role="status">{files.length} file{files.length === 1 ? "" : "s"}</span>
    {/if}
    <button class="refresh" type="button" onclick={() => (reload += 1)}>
      Refresh
    </button>
  </header>

  {#if error && files.length === 0}
    <p class="empty">Could not load the diff.</p>
  {:else}
    <div class="viewer">
      <DiffViewer {files} />
    </div>
  {/if}
</section>

<style>
  .popout-diff {
    display: flex;
    flex-direction: column;
    height: 100vh;
    min-height: 0;
    background: var(--m3-surface);
    color: var(--m3-on-surface);
  }

  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.3rem 0.75rem;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container, var(--m3-surface));
    font-size: 0.8125rem;
  }

  .title {
    font-weight: 500;
  }

  .spacer {
    flex: 1;
  }

  .state {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .state.err {
    color: var(--m3-error, red);
  }

  .refresh {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface, inherit);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.1rem 0.6rem;
    cursor: pointer;
  }

  .refresh:hover,
  .refresh:focus-visible {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .empty {
    padding: 2rem 1rem;
    text-align: center;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .viewer {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .viewer > :global(*) {
    flex: 1;
    min-height: 0;
  }
</style>
