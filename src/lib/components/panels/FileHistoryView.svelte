<!--
  FileHistoryView (M9 F5) — one file's history in a popout window
  (`?panel=filehistory&repo=<id>&path=<rel path>`).

  Reuses HistoryStore in follow mode (rename-following walk) and renders a
  lightweight virtualized commit list (no graph lanes — the file walk is a
  single line). Row actions: copy sha, blame this file (toggles BlameView
  below).

  Selection: a plain click opens the commit's detail in the lower pane
  (shared `CommitDetail`, which fetches the parent0→sha diff itself);
  shift-click extends a consecutive range and shows the AGGREGATE diff of
  the range — `oldest^..newest`, computed via `fileHistoryModel` — passed
  to CommitDetail as its `compare` (closing the compare tab falls back to
  the clicked commit's own diff). List↔detail is a user-resizable
  SplitPane (ratio persisted as `filehistory-detail`).
-->
<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { HistoryStore } from "$lib/stores/history.svelte";
  import {
    formatRelativeTime,
    hashHue,
  } from "$lib/stores/history-logic";
  import { repoDiff } from "$lib/ipc/client";
  import type { CommitInfo, FileDiff } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import CommitDetail from "./CommitDetail.svelte";
  import SplitPane from "$lib/components/layout/SplitPane.svelte";
  import { readSplitRatio, writeSplitRatio } from "$lib/layout/splitPrefs";
  import {
    aggregateRange,
    rangeIndices,
  } from "./fileHistoryModel";
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

  // -- selection + detail pane --------------------------------------------------

  const commits = $derived(history.flat.commits);

  let selectedSha = $state<string | null>(null);
  let selectedInfo = $state<CommitInfo | null>(null);
  /** Multi-selection as absolute row indices (shift-click range). */
  let multiIdx = $state<number[]>([]);
  /** Anchor row for shift-click ranges (last plain click). */
  let anchorIdx: number | null = null;
  let detailOpen = $state(false);
  /** Aggregate diff of the current range (null = show the single diff). */
  let compare = $state<{ files: FileDiff[]; base: string; target: string } | null>(null);

  /** List fraction of the split (detail takes the rest). */
  const DETAIL_RATIO_KEY = "filehistory-detail";
  let detailRatio = $state(readSplitRatio(DETAIL_RATIO_KEY, 0.55));

  function setDetailRatio(ratio: number): void {
    detailRatio = ratio;
    writeSplitRatio(DETAIL_RATIO_KEY, ratio);
  }

  function selectCommit(commit: CommitInfo, idx: number): void {
    selectedSha = commit.sha;
    selectedInfo = commit;
    multiIdx = [idx];
    anchorIdx = idx;
    compare = null;
    detailOpen = true;
  }

  /** Aggregate fetches are token-guarded: only the newest range lands. */
  let aggToken = 0;

  async function fetchAggregate(lo: number, hi: number): Promise<void> {
    const spec = aggregateRange(commits, lo, hi);
    if ("error" in spec) {
      toast(spec.error, { kind: "error" });
      return;
    }
    const token = ++aggToken;
    compare = null;
    try {
      const files = await repoDiff(
        repoId,
        { commit: spec.baseSha },
        { commit: spec.targetSha },
      );
      if (token !== aggToken) return;
      compare = { files, base: spec.baseLabel, target: spec.targetLabel };
    } catch (err: unknown) {
      if (token !== aggToken) return;
      toast(
        `Aggregate diff failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  /**
   * Row click: plain click (re)selects one commit (CommitDetail fetches
   * its diff); shift-click extends the range to anchor..clicked and shows
   * the aggregate diff of the whole range.
   */
  function onRowClick(commit: CommitInfo, idx: number, event: MouseEvent): void {
    if (event.shiftKey && anchorIdx !== null) {
      const lo = Math.min(anchorIdx, idx);
      const hi = Math.max(anchorIdx, idx);
      multiIdx = rangeIndices(lo, hi);
      selectedSha = commit.sha;
      selectedInfo = commit;
      detailOpen = true;
      void fetchAggregate(lo, hi);
      return;
    }
    selectCommit(commit, idx);
  }

  function onRowKeydown(commit: CommitInfo, idx: number, event: KeyboardEvent): void {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      selectCommit(commit, idx);
    }
  }
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

  {#snippet listPane()}
    <div
      class="rows"
      onscroll={onScroll}
      role="listbox"
      aria-label="Commits touching {path}"
      tabindex="0"
    >
      <div style:height="{commits.length * ROW_HEIGHT}px"></div>
      <ul class="list" style:top="{firstVisible * ROW_HEIGHT}px">
        {#each commits.slice(firstVisible, firstVisible + visibleCount) as commit, i (commit.sha)}
          {@const idx = firstVisible + i}
          <li
            class="row"
            class:selected={commit.sha === selectedSha}
            class:multisel={commit.sha !== selectedSha && multiIdx.includes(idx)}
            style:height="{ROW_HEIGHT}px"
            role="option"
            tabindex="-1"
            aria-selected={commit.sha === selectedSha || multiIdx.includes(idx)}
            onclick={(e) => onRowClick(commit, idx, e)}
            onkeydown={(e) => onRowKeydown(commit, idx, e)}
          >
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
                onclick={(e) => {
                  e.stopPropagation();
                  copySha(commit.sha);
                }}
              >
                Copy
              </button>
            </div>
          </li>
        {/each}
      </ul>
    </div>
  {/snippet}

  {#if detailOpen && (selectedInfo || compare)}
    <SplitPane
      axis="y"
      ratio={detailRatio}
      onRatio={setDetailRatio}
      label="Resize file history list and detail"
    >
      {#snippet a()}
        {@render listPane()}
      {/snippet}
      {#snippet b()}
        <CommitDetail
          {repoId}
          info={selectedInfo}
          {compare}
          actionCount={multiIdx.length}
          onClose={() => (detailOpen = false)}
          onCloseCompare={() => (compare = null)}
        />
      {/snippet}
    </SplitPane>
  {:else}
    {@render listPane()}
  {/if}
</section>

<style>
  .file-history {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  /* The list ↔ detail SplitPane fills the rest of the panel. */
  .file-history > :global(.split) {
    flex: 1;
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

  .rows:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
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
    cursor: pointer;
    /* shift-click extends the selection, not a text selection */
    user-select: none;
  }

  .row:hover {
    background: var(--m3-surface-container-low, var(--m3-surface));
  }

  .row.selected {
    background: var(--m3-secondary-container);
    color: var(--m3-on-secondary-container);
  }

  /* shift-click range members (the clicked commit stays `.selected`) */
  .row.multisel {
    background: color-mix(in srgb, var(--m3-secondary-container) 45%, var(--m3-surface));
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

  .row.selected .meta {
    color: inherit;
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

  .row.selected .sha {
    color: inherit;
  }
</style>
