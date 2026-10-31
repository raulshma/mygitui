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
  } from "$lib/git/format";
  import { repoDiff } from "$lib/ipc/client";
  import type { CommitInfo } from "$lib/ipc/types";
  import type { CompareResult } from "$lib/components/panels/compare";
  import { toast } from "$lib/toast";
  import CommitDetail from "./CommitDetail.svelte";
  import SplitPane from "$lib/components/layout/SplitPane.svelte";
  import { readSplitRatio, writeSplitRatio } from "$lib/layout/splitPrefs";
  import {
    aggregateRange,
    rangeIndices,
  } from "./fileHistoryModel";
  import BlameView from "./BlameView.svelte";
  import { Badge } from "$lib/components/ui/badge";
  import { Button } from "$lib/components/ui/button/index.js";

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
  let compare = $state<CompareResult | null>(null);

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

<section
  class="file-history flex h-full min-h-0 flex-col [&>.split]:min-h-0 [&>.split]:flex-1"
  aria-label="File history for {path}"
>
  <header class="head flex items-center gap-3 border-b border-border px-4 py-2.5">
    <h1
      class="title m-0 min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.9375rem] font-medium"
      title={path}
    >
      {path}
    </h1>
    <span class="count shrink-0 text-xs text-muted-foreground"
      >{history.flat.commits.length} commit{history.flat.commits.length === 1 ? "" : "s"}</span
    >
    <Button
      class="shrink-0 rounded-lg px-2.5 py-1 text-xs font-normal"
      variant="outline"
      type="button"
      onclick={() => (blameOpen = !blameOpen)}
    >
      {blameOpen ? "Hide blame" : "Blame this file"}
    </Button>
  </header>

  {#if history.error}
    <p class="error m-0 px-4 py-3 text-[0.8125rem] text-destructive" role="alert">
      {history.error}
      <Button
        class="shrink-0 rounded-lg px-2.5 py-1 text-xs font-normal"
        variant="outline"
        type="button"
        onclick={() => history.restart()}
      >
        Retry
      </Button>
    </p>
  {:else if history.flat.commits.length === 0 && history.loading}
    <p class="note m-0 px-4 py-3 text-[0.8125rem] text-muted-foreground">Loading history…</p>
  {:else if history.flat.commits.length === 0}
    <p class="note m-0 px-4 py-3 text-[0.8125rem] text-muted-foreground">
      No commits touch this file.</p
    >
  {/if}

  {#if blameOpen}
    <div class="blame max-h-[40%] overflow-auto border-b border-border">
      <BlameView {repoId} {path} />
    </div>
  {/if}

  {#snippet listPane()}
    <div
      class="rows relative min-h-0 flex-1 overflow-y-auto focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
      onscroll={onScroll}
      role="listbox"
      aria-label="Commits touching {path}"
      tabindex="0"
    >
      <div style:height="{commits.length * ROW_HEIGHT}px"></div>
      <ul class="list absolute inset-x-0 m-0 list-none px-4 py-0" style:top="{firstVisible * ROW_HEIGHT}px">
        {#each commits.slice(firstVisible, firstVisible + visibleCount) as commit, i (commit.sha)}
          {@const idx = firstVisible + i}
          <li
            class="row flex cursor-pointer select-none items-center justify-between gap-4 border-b border-border py-0.5 {commit.sha ===
            selectedSha
              ? 'bg-primary/15 text-foreground'
              : multiIdx.includes(idx)
                ? 'bg-primary/5'
                : 'hover:bg-muted/50'}"
            class:selected={commit.sha === selectedSha}
            class:multisel={commit.sha !== selectedSha && multiIdx.includes(idx)}
            style:height="{ROW_HEIGHT}px"
            role="option"
            tabindex="-1"
            aria-selected={commit.sha === selectedSha || multiIdx.includes(idx)}
            onclick={(e) => onRowClick(commit, idx, e)}
            onkeydown={(e) => onRowKeydown(commit, idx, e)}
          >
            <div class="main flex min-w-0 flex-col">
              <span class="summary overflow-hidden text-ellipsis whitespace-nowrap text-sm" title={commit.message}
                >{commit.summary}</span
              >
              <span
                class="meta text-xs {commit.sha === selectedSha ? 'text-inherit' : 'text-muted-foreground'}"
                >{commit.author.name}
                · {formatRelativeTime(commit.committer.time)}</span
              >
            </div>
            <div class="side flex shrink-0 items-center gap-1.5">
              {#each commit.refs.slice(0, 2) as ref (ref)}
                <Badge
                  variant="outline"
                  class="ref h-auto rounded-full px-1.5 py-px whitespace-nowrap text-[0.6875rem]"
                  style="background: hsl({hashHue(ref)} 45% 88%)"
                  >{ref}</Badge
                >
              {/each}
              <span
                class="sha font-mono text-xs {commit.sha === selectedSha ? 'text-inherit' : 'text-muted-foreground'}"
                >{commit.sha.slice(0, 8)}</span
              >
              <Button
                class="shrink-0 rounded-lg px-2.5 py-1 text-xs font-normal"
                variant="outline"
                type="button"
                aria-label="Copy sha {commit.sha.slice(0, 8)}"
                onclick={(e) => {
                  e.stopPropagation();
                  copySha(commit.sha);
                }}
              >
                Copy
              </Button>
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
