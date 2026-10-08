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
   */
  import { repoBlame } from "$lib/ipc/client";
  import { toast } from "$lib/toast";
  import { emitUiEvent } from "$lib/palette/events";
  import type { BlameLine } from "$lib/ipc/types";
  import { formatRelativeTime, hashHue } from "$lib/stores/history-logic";

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

  $effect(() => {
    const repo = repoId;
    const file = path;
    const rev = from; // tracked: reload when any of these change
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

<div class="blame" aria-label={"Blame for " + path}>
  <header class="bhead">
    <span class="bpath" title={path}>{path}</span>
    {#if from}
      <span class="bfrom">@ {from.slice(0, 8)}</span>
    {/if}
    {#if loading}
      <span class="bstate" role="status">Loading blame…</span>
    {:else if error}
      <span class="bstate berror" role="alert">{error}</span>
    {:else if rows.length === 0}
      <span class="bstate">No blame data</span>
    {/if}
  </header>

  {#if loading}
    <p class="empty" role="status">Loading blame for {path}…</p>
  {:else if error}
    <p class="empty" role="alert">Blame failed: {error}</p>
  {:else if rows.length === 0}
    <p class="empty">No blame lines for {path}.</p>
  {:else}
    <div class="bscroll" bind:this={scrollerEl} onscroll={onScroll}>
      <div
        class="bspacer"
        role="list"
        aria-label={"Blame lines for " + path}
        style:height={`${rows.length * ROW_HEIGHT}px`}
      >
        {#each visible as v (v.idx)}
          {#if v.row.kind === "group"}
            <div
              class="bgroup"
              style:top={`${v.idx * ROW_HEIGHT}px`}
              style:height={`${ROW_HEIGHT}px`}
            >
              <button
                class="bsha"
                style:color={shaColor(v.row.sha)}
                title="Show this commit in history"
                onclick={() => shaClick(v.row.sha)}
              >
                {v.row.sha.slice(0, 8)}
              </button>
              <span class="bauthor">{v.row.line?.signature.name ?? ""}</span>
              <span class="bdate">
                {v.row.line ? formatRelativeTime(v.row.line.signature.time) : ""}
              </span>
              <span class="bcount">{v.row.count} {v.row.count === 1 ? "line" : "lines"}</span>
            </div>
          {:else}
            <div
              class="bline"
              role="listitem"
              style:top={`${v.idx * ROW_HEIGHT}px`}
              style:height={`${ROW_HEIGHT}px`}
            >
              <span class="bno">{v.row.line?.line_no}</span>
              <span class="btext">{v.row.line?.text ?? ""}</span>
            </div>
          {/if}
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .blame {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--m3-surface);
  }

  .bhead {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    padding: 0.25rem 0.75rem;
    border-bottom: 1px solid var(--m3-outline-variant);
    background: var(--m3-surface-container, var(--m3-surface));
    font-size: 0.75rem;
  }

  .bpath {
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .bfrom {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface-variant);
  }

  .bstate {
    margin-left: auto;
    color: var(--m3-on-surface-variant);
  }

  .bstate.berror {
    color: var(--m3-error);
  }

  .empty {
    margin: auto;
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
  }

  .bscroll {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .bspacer {
    position: relative;
    min-width: 100%;
    width: max-content;
  }

  .bgroup,
  .bline {
    position: absolute;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0 0.75rem 0 0.5rem;
    font-size: 0.75rem;
    white-space: pre;
  }

  .bgroup {
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-bottom: 1px solid var(--m3-outline-variant);
    color: var(--m3-on-surface-variant);
  }

  .bsha {
    flex: none;
    padding: 0 0.25rem;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    background: none;
    border: none;
    cursor: pointer;
  }

  .bsha:hover {
    text-decoration: underline;
  }

  .bsha:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
    border-radius: 2px;
  }

  .bauthor {
    flex: none;
    max-width: 12rem;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--m3-on-surface);
  }

  .bdate,
  .bcount {
    flex: none;
    color: var(--m3-on-surface-variant);
  }

  .bcount {
    margin-left: auto;
  }

  .bline {
    color: var(--m3-on-surface);
  }

  .bno {
    flex: none;
    width: 3rem;
    text-align: right;
    color: var(--m3-on-surface-variant);
    font-family: ui-monospace, Consolas, monospace;
    user-select: none;
  }

  .btext {
    font-family: ui-monospace, Consolas, monospace;
    tab-size: 4;
  }
</style>
