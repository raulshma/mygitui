<!--
  FileHistoryView (M9 F5) — one file's history in a popout window
  (`?panel=filehistory&repo=<id>&path=<rel path>`).

  Reuses HistoryStore in follow mode (rename-following walk) and renders a
  lightweight virtualized commit list (no graph lanes — the file walk is a
  single line). Row actions: copy sha, blame this file (toggles BlameView
  below), and open the commit's diff in the main window is out of scope for
  the popout bus (windows have separate document events) — copy instead.
-->
<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { HistoryStore } from "$lib/stores/history.svelte";
  import {
    formatRelativeTime,
    hashHue,
  } from "$lib/stores/history-logic";
  import { toast } from "$lib/toast";
  import BlameView from "./BlameView.svelte";

  let { repoId, path }: { repoId: string; path: string } = $props();

  const ROW_HEIGHT = 44;
  const OVERSCAN = 10;

  const history = new HistoryStore({ follow: true });

  onMount(() => {
    history.setFilter({ path });
    history.start(repoId);
  });
  onDestroy(() => history.destroy());

  let scrollTop = $state(0);
  let viewportHeight = $state(600);

  function onScroll(event: Event): void {
    const el = event.currentTarget as HTMLElement;
    scrollTop = el.scrollTop;
    viewportHeight = el.clientHeight;
    if (
      el.scrollHeight - scrollTop - viewportHeight < 800 &&
      history.hasMore
    ) {
      history.loadMore();
    }
  }

  const firstVisible = $derived(
    Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN),
  );
  const visibleCount = $derived(
    Math.min(
      history.flat.commits.length,
      Math.ceil(viewportHeight / ROW_HEIGHT) + OVERSCAN * 2,
    ),
  );

  function copySha(sha: string): void {
    void navigator.clipboard.writeText(sha).then(() =>
      toast("Copied commit sha", { kind: "success" }),
    );
  }

  let blameOpen = $state(false);
</script>

<section class="file-history" aria-label="File history for {path}">
  <header class="head">
    <h1 class="title" title={path}>{path}</h1>
    <span class="count"
      >{history.flat.commits.length} commit{history.flat.commits.length === 1 ? "" : "s"}</span
    >
    <button class="action" type="button" onclick={() => (blameOpen = !blameOpen)}>
      {blameOpen ? "Hide blame" : "Blame this file"}
    </button>
  </header>

  {#if history.error}
    <p class="error" role="alert">
      {history.error}
      <button class="action" type="button" onclick={() => history.restart()}>
        Retry
      </button>
    </p>
  {:else if history.flat.commits.length === 0 && history.loading}
    <p class="note">Loading history…</p>
  {:else if history.flat.commits.length === 0}
    <p class="note">No commits touch this file.</p>
  {/if}

  {#if blameOpen}
    <div class="blame">
      <BlameView {repoId} {path} />
    </div>
  {/if}

  <div
    class="rows"
    onscroll={onScroll}
    role="feed"
    aria-label="Commits touching {path}"
  >
    <div style:height="{history.flat.commits.length * ROW_HEIGHT}px"></div>
    <ul class="list" style:top="{firstVisible * ROW_HEIGHT}px">
      {#each history.flat.commits.slice(firstVisible, firstVisible + visibleCount) as commit (commit.sha)}
        <li class="row" style:height="{ROW_HEIGHT}px">
          <div class="main">
            <span class="summary" title={commit.message}>{commit.summary}</span>
            <span class="meta"
              >{commit.author.name}
              · {formatRelativeTime(commit.committer.time)}</span
            >
          </div>
          <div class="side">
            {#each commit.refs.slice(0, 2) as ref (ref)}
              <span
                class="ref"
                style:background="hsl({hashHue(ref)} 45% 88%)"
                >{ref}</span
              >
            {/each}
            <span class="sha">{commit.sha.slice(0, 8)}</span>
            <button
              class="action"
              type="button"
              aria-label="Copy sha {commit.sha.slice(0, 8)}"
              onclick={() => copySha(commit.sha)}
            >
              Copy
            </button>
          </div>
        </li>
      {/each}
    </ul>
  </div>
</section>

<style>
  .file-history {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.625rem 1rem;
    border-bottom: 1px solid var(--m3-outline-variant, transparent);
  }

  .title {
    margin: 0;
    font-size: 0.9375rem;
    font-weight: 500;
    font-family: ui-monospace, Consolas, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }

  .count {
    color: var(--m3-on-surface-variant, inherit);
    font-size: 0.75rem;
    flex: none;
  }

  .action {
    flex: none;
    border: 1px solid var(--m3-outline-variant, transparent);
    border-radius: var(--m3-shape-small, 8px);
    background: none;
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.75rem;
    padding: 0.25rem 0.625rem;
    cursor: pointer;
  }

  .action:hover,
  .action:focus-visible {
    background: var(--m3-surface-container-high, inherit);
  }

  .error,
  .note {
    margin: 0;
    padding: 0.75rem 1rem;
    color: var(--m3-on-surface-variant, inherit);
    font-size: 0.8125rem;
  }

  .error {
    color: var(--m3-error);
  }

  .blame {
    max-height: 40%;
    overflow: auto;
    border-bottom: 1px solid var(--m3-outline-variant, transparent);
  }

  .rows {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .list {
    position: absolute;
    left: 0;
    right: 0;
    margin: 0;
    padding: 0 1rem;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.25rem 0;
    border-bottom: 1px solid var(--m3-outline-variant, transparent);
  }

  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .summary {
    font-size: 0.875rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    color: var(--m3-on-surface-variant, inherit);
    font-size: 0.75rem;
  }

  .side {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex: none;
  }

  .ref {
    font-size: 0.6875rem;
    padding: 0.0625rem 0.4rem;
    border-radius: var(--m3-shape-full, 9999px);
    white-space: nowrap;
  }

  .sha {
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    color: var(--m3-on-surface-variant, inherit);
  }
</style>
