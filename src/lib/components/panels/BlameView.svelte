<script lang="ts">
  /**
   * BlameView (B3 lane) — virtualized line-by-line blame for one file.
   *
   * HOW TO MOUNT (HistoryView already wires this from its commit-detail file
   * list; to mount from RepoView or anywhere else):
   *   import BlameView from "$lib/components/panels/BlameView.svelte";
   *   <BlameView repoId={tab.id} path="src/main.rs" from={selectedSha} />
   * `from` is optional (blames the file as of HEAD when omitted).
   *
   * Rendering: contiguous lines with the same authoring sha are grouped under
   * one header row (sha chip + author + date + line count); line rows show
   * only line number + text, so the header "collapses" the repeated metadata.
   * All rows have a fixed height and are virtualized (spacer + absolute
   * rows) over an independently scrolling pane. Clicking a sha shows the
   * M2 placeholder toast (commit actions land in M2).
   *
   * M12 (lane C): every line row carries an age tint derived from its
   * authoring commit's signature time (newest = strongest accent, oldest =
   * faded; ratios computed once per result in blameModel.ts) plus a hover
   * tooltip (author + date). Clicking a LINE offers a re-blame action
   * "Blame from <sha>'s parent" (root commits have none) which pushes onto
   * a breadcrumb stack — "Back" pops it. Sha chips keep emitting
   * history-select-commit.
   */
  import { repoBlame, streamLog } from "$lib/ipc/client";
  import { toast } from "$lib/toast";
  import { emitUiEvent } from "$lib/palette/events";
  import type { BlameLine, LogFilter } from "$lib/ipc/types";
  import {
    formatDateTime,
    formatRelativeTime,
    hashHue,
  } from "$lib/git/format";
  import { ageBackground, ageRatios } from "./blameModel";
  import { Button } from "$lib/components/ui/button/index.js";

  let {
    repoId,
    path,
    from = undefined,
  }: {
    repoId: string;
    path: string;
    from?: string;
  } = $props();

  const ROW_HEIGHT = 22;
  const OVERSCAN = 12;

  /** One rendered row: either a group header or a blame line. */
  interface BlameRow {
    kind: "group" | "line";
    sha: string;
    line?: BlameLine;
    count?: number;
  }

  let lines = $state<BlameLine[] | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let reqToken = 0;

  // -- M12: re-blame breadcrumb stack -----------------------------------------

  /** Parent `from` revisions to blame through, deepest last. */
  let blameFrom = $state<string[]>([]);
  /** Current re-blame offer ("Blame from <sha>'s parent"). */
  let reblame = $state<{ sha: string; parent: string } | null>(null);

  /** sha → parents (blame commits are immutable; cache for the view's life). */
  const parentsCache = new Map<string, string[]>();

  const PARENT_LOG_FILTER: LogFilter = {
    text: null,
    regex: false,
    author: null,
    path: null,
    refs: [],
    follow: false,
  };

  /** The revision currently being blamed (stack top, else the prop). */
  const effectiveFrom = $derived(
    blameFrom.length > 0 ? blameFrom[blameFrom.length - 1] : from,
  );

  // A new target file/repo/prop rev invalidates the re-blame walk.
  $effect(() => {
    void repoId;
    void path;
    void from;
    blameFrom = [];
    reblame = null;
  });

  /** Parents of `sha` (fetch once through the log stream; null = unknown). */
  async function parentsOf(sha: string): Promise<string[] | null> {
    const cached = parentsCache.get(sha);
    if (cached) return cached;
    try {
      const parents = await new Promise<string[]>((resolve, reject) => {
        void streamLog(
          repoId,
          { ...PARENT_LOG_FILTER, refs: [sha] },
          (page) => {
            resolve(page.commits.find((c) => c.sha === sha)?.parents ?? []);
          },
        ).catch(reject);
      });
      parentsCache.set(sha, parents);
      return parents;
    } catch {
      return null;
    }
  }

  async function lineClick(sha: string): Promise<void> {
    if (loading) return;
    const parents = await parentsOf(sha);
    if (parents === null) {
      toast(`Could not look up ${sha.slice(0, 8)} — cannot re-blame`, {
        kind: "error",
      });
      return;
    }
    if (parents.length === 0) {
      toast(`${sha.slice(0, 8)} is the root commit — no parent to blame from`);
      return;
    }
    reblame = { sha, parent: parents[0] };
  }

  /** Pushes the offered parent onto the stack (the load effect follows). */
  function doReblame(): void {
    if (!reblame) return;
    blameFrom = [...blameFrom, reblame.parent];
    reblame = null;
  }

  /** Pops one breadcrumb level ("Back"). */
  function reblameBack(): void {
    blameFrom = blameFrom.slice(0, -1);
    reblame = null;
  }

  // Flat render list: group headers interleaved with their lines.
  const rows = $derived.by(() => {
    const ls = lines ?? [];
    const out: BlameRow[] = [];
    let i = 0;
    while (i < ls.length) {
      let j = i;
      const sha = ls[i]?.sha ?? "";
      while (j < ls.length && ls[j]?.sha === sha) j++;
      out.push({ kind: "group", sha, count: j - i, line: ls[i] });
      for (let k = i; k < j; k++) out.push({ kind: "line", sha, line: ls[k] });
      i = j;
    }
    return out;
  });

  /** M12: per-sha freshness (1 = newest commit in this result). */
  const heat = $derived(ageRatios(lines ?? []));

  $effect(() => {
    const repo = repoId;
    const file = path;
    const rev = effectiveFrom; // tracked: reload when any of these change
    const token = ++reqToken;
    lines = null;
    error = null;
    loading = true;
    repoBlame(repo, file, rev)
      .then((result) => {
        if (token !== reqToken) return;
        lines = result;
        loading = false;
      })
      .catch((err: unknown) => {
        if (token !== reqToken) return;
        loading = false;
        error = err instanceof Error ? err.message : String(err);
        toast(`Blame failed: ${error}`, { kind: "error" });
      });
  });

  // -- virtualizer -----------------------------------------------------------

  let scrollerEl = $state<HTMLDivElement | undefined>(undefined);
  let range = $state({ start: 0, end: 0 });

  function updateRange(): void {
    const el = scrollerEl;
    if (!el) return;
    const total = rows.length;
    const first = Math.floor(el.scrollTop / ROW_HEIGHT);
    const count = Math.ceil(el.clientHeight / ROW_HEIGHT);
    const start = Math.max(0, first - OVERSCAN);
    const end = Math.min(total, first + count + OVERSCAN);
    if (start !== range.start || end !== range.end) range = { start, end };
  }

  function onScroll(): void {
    updateRange();
  }

  $effect(() => {
    // Fill the initial viewport once data lands (or the pane resizes on
    // mount) — afterwards the scroll handler keeps the range fresh.
    void rows.length;
    updateRange();
  });

  const visible = $derived.by(() => {
    const out: Array<{ row: BlameRow; idx: number }> = [];
    for (let i = range.start; i < range.end; i++) {
      const row = rows[i];
      if (row) out.push({ row, idx: i });
    }
    return out;
  });

  function shaColor(sha: string): string {
    return `hsl(${hashHue(sha)} 60% 55%)`;
  }

  /** M9: selecting the authoring commit in history (typed event bus). */
  function shaClick(sha: string): void {
    emitUiEvent("history-select-commit", { sha });
  }
</script>

<div
  class="blame flex h-full min-h-0 flex-col bg-background"
  aria-label={"Blame for " + path}
>
  <header class="bhead flex items-baseline gap-2 border-b border-border bg-muted/50 px-3 py-1 text-xs">
    <span
      class="bpath overflow-hidden text-ellipsis whitespace-nowrap font-mono text-foreground"
      title={path}>{path}</span
    >
    {#if effectiveFrom}
      <span class="bfrom shrink-0 font-mono text-muted-foreground"
        >@ {effectiveFrom.slice(0, 8)}</span
      >
    {/if}
    {#if blameFrom.length > 0}
      <Button
        class="h-auto shrink-0 whitespace-nowrap rounded-full px-2 py-0 text-[0.6875rem] font-normal text-muted-foreground"
        variant="outline"
        type="button"
        title={blameFrom.length > 1
          ? `Back to ${blameFrom[blameFrom.length - 2].slice(0, 8)}`
          : "Back to the original revision"}
        onclick={reblameBack}
      >
        ← Back
      </Button>
      {#if blameFrom.length > 1}
        <span class="bdepth shrink-0 text-[0.6875rem] text-muted-foreground"
          >{blameFrom.length} steps</span
        >
      {/if}
    {/if}
    {#if loading}
      <span class="bstate ml-auto text-muted-foreground" role="status"
        >Loading blame…</span
      >
    {:else if error}
      <span class="bstate berror ml-auto text-destructive" role="alert"
        >{error}</span
      >
    {:else if rows.length === 0}
      <span class="bstate ml-auto text-muted-foreground">No blame data</span>
    {/if}
  </header>

  {#if reblame}
    <div
      class="breblame flex shrink-0 items-center gap-2 border-b border-border bg-muted/50 px-3 py-1 text-xs"
      role="status"
    >
      <span
        class="breblame-text min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-muted-foreground"
      >
        Blame from <strong>{reblame.sha.slice(0, 8)}</strong>'s parent
        ({reblame.parent.slice(0, 8)})
      </span>
      <Button
        class="h-auto shrink-0 whitespace-nowrap rounded-full px-2.5 py-0.5 text-[0.6875rem]"
        type="button"
        onclick={doReblame}
      >
        Blame from {reblame.parent.slice(0, 8)}
      </Button>
      <Button
        class="h-auto shrink-0 whitespace-nowrap rounded-full px-2 py-0 text-[0.6875rem] font-normal text-muted-foreground"
        variant="outline"
        type="button"
        aria-label="Dismiss re-blame"
        onclick={() => (reblame = null)}
      >
        ×
      </Button>
    </div>
  {/if}

  {#if loading}
    <p class="empty m-auto text-[0.8125rem] text-muted-foreground" role="status">
      Loading blame for {path}…</p
    >
  {:else if error}
    <p class="empty m-auto text-[0.8125rem] text-muted-foreground" role="alert">
      Blame failed: {error}</p
    >
  {:else if rows.length === 0}
    <p class="empty m-auto text-[0.8125rem] text-muted-foreground">
      No blame lines for {path}.</p
    >
  {:else}
    <div class="bscroll min-h-0 flex-1 overflow-auto" bind:this={scrollerEl} onscroll={onScroll}>
      <div
        class="bspacer relative w-max min-w-full"
        role="list"
        aria-label={"Blame lines for " + path}
        style:height={`${rows.length * ROW_HEIGHT}px`}
      >
        {#each visible as v (v.idx)}
          {#if v.row.kind === "group"}
            <div
              class="bgroup absolute inset-x-0 flex items-center gap-2 bg-muted/70 py-0 pl-2 pr-3 text-xs text-muted-foreground whitespace-pre"
              style:top={`${v.idx * ROW_HEIGHT}px`}
              style:height={`${ROW_HEIGHT}px`}
            >
              <button
                class="bsha shrink-0 cursor-pointer border-0 bg-transparent px-1 font-mono text-xs hover:underline focus-visible:rounded-xs focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
                style:color={shaColor(v.row.sha)}
                title="Show this commit in history"
                onclick={() => shaClick(v.row.sha)}
              >
                {v.row.sha.slice(0, 8)}
              </button>
              <span class="bauthor max-w-48 shrink-0 overflow-hidden text-ellipsis text-foreground"
                >{v.row.line?.signature.name ?? ""}</span
              >
              <span class="bdate shrink-0 text-muted-foreground">
                {v.row.line ? formatRelativeTime(v.row.line.signature.time) : ""}
              </span>
              <span class="bcount ml-auto shrink-0 text-muted-foreground"
                >{v.row.count} {v.row.count === 1 ? "line" : "lines"}</span
              >
            </div>
          {:else}
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="bline group/bline absolute inset-x-0 flex cursor-pointer items-center gap-2 py-0 pl-2 pr-3 text-xs text-foreground whitespace-pre"
              role="listitem"
              style:top={`${v.idx * ROW_HEIGHT}px`}
              style:height={`${ROW_HEIGHT}px`}
              style:background={ageBackground(heat.get(v.row.sha) ?? 0)}
              title={v.row.line
                ? `${v.row.line.signature.name} · ${formatDateTime(v.row.line.signature.time)} — click to blame from this commit's parent`
                : ""}
              onclick={() => lineClick(v.row.sha)}
            >
              <span
                class="bno w-12 shrink-0 select-none text-right font-mono text-muted-foreground"
                >{v.row.line?.line_no}</span
              >
              <span
                class="btext font-mono [tab-size:4] group-hover/bline:underline group-hover/bline:decoration-dotted group-hover/bline:underline-offset-3"
                >{v.row.line?.text ?? ""}</span
              >
            </div>
          {/if}
        {/each}
      </div>
    </div>
  {/if}
</div>
