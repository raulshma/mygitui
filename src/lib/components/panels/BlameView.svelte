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
  } from "$lib/stores/history-logic";
  import { ageBackground, ageRatios } from "./blameModel";

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

<div class="blame" aria-label={"Blame for " + path}>
  <header class="bhead">
    <span class="bpath" title={path}>{path}</span>
    {#if effectiveFrom}
      <span class="bfrom">@ {effectiveFrom.slice(0, 8)}</span>
    {/if}
    {#if blameFrom.length > 0}
      <button
        class="bback"
        type="button"
        title={blameFrom.length > 1
          ? `Back to ${blameFrom[blameFrom.length - 2].slice(0, 8)}`
          : "Back to the original revision"}
        onclick={reblameBack}
      >
        ← Back
      </button>
      {#if blameFrom.length > 1}
        <span class="bdepth">{blameFrom.length} steps</span>
      {/if}
    {/if}
    {#if loading}
      <span class="bstate" role="status">Loading blame…</span>
    {:else if error}
      <span class="bstate berror" role="alert">{error}</span>
    {:else if rows.length === 0}
      <span class="bstate">No blame data</span>
    {/if}
  </header>

  {#if reblame}
    <div class="breblame" role="status">
      <span class="breblame-text">
        Blame from <strong>{reblame.sha.slice(0, 8)}</strong>'s parent
        ({reblame.parent.slice(0, 8)})
      </span>
      <button class="bgo" type="button" onclick={doReblame}>
        Blame from {reblame.parent.slice(0, 8)}
      </button>
      <button
        class="bback"
        type="button"
        aria-label="Dismiss re-blame"
        onclick={() => (reblame = null)}
      >
        ×
      </button>
    </div>
  {/if}

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
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="bline"
              role="listitem"
              style:top={`${v.idx * ROW_HEIGHT}px`}
              style:height={`${ROW_HEIGHT}px`}
              style:background={ageBackground(heat.get(v.row.sha) ?? 0)}
              title={v.row.line
                ? `${v.row.line.signature.name} · ${formatDateTime(v.row.line.signature.time)} — click to blame from this commit's parent`
                : ""}
              onclick={() => lineClick(v.row.sha)}
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

  .bback {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.6875rem;
    padding: 0 0.5rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .bback:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .bback:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .bdepth {
    flex: none;
    color: var(--m3-on-surface-variant);
    font-size: 0.6875rem;
  }

  .breblame {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.25rem 0.75rem;
    border-bottom: 1px solid var(--m3-outline-variant);
    background: var(--m3-surface-container, var(--m3-surface));
    font-size: 0.75rem;
  }

  .breblame-text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .bgo {
    flex: none;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-size: 0.6875rem;
    padding: 0.15rem 0.6rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .bgo:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
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
    cursor: pointer; /* click = offer re-blame from this line's commit */
  }

  .bline:hover .btext {
    text-decoration: underline dotted;
    text-underline-offset: 3px;
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
