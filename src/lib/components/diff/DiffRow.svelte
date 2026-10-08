<script lang="ts">
  /**
   * One virtualized row of the diff (see rowModel.ts for the row kinds).
   * Heights are inline from ROW_HEIGHTS so CSS can never disagree with the
   * virtualizer's uniform-height buckets.
   *
   * Layout:
   *   split    [ old# | - | text ][ new# | + | text ]  (equal halves)
   *   context  [ old# | new# | text spanning both halves ]
   *   unified  [ old# | new# | sign | text ]
   */
  import type { DiffLine } from "$lib/ipc/types";
  import { applyHighlights } from "$lib/components/diff/bytes";
  import { rowHeight, type DiffRow, type LoadImageFn } from "$lib/components/diff/rowModel";
  import { cachedTokens, type TokenSpan } from "$lib/diff/highlight";
  import ImageCompare from "$lib/components/diff/ImageCompare.svelte";

  let {
    row,
    onToggleCollapse,
    onLoadImage,
    onStageHunk,
    onDiscardHunk,
    /** Owning file path (syntax highlighting); null disables it. */
    filePath = null,
    /** Hunk-header staging button label ("Unstage hunk" when unstaging). */
    stageLabel = "Stage hunk",
  }: {
    row: DiffRow;
    onToggleCollapse?: (path: string) => void;
    onLoadImage?: LoadImageFn;
    /** Stage/unstage this hunk. */
    onStageHunk?: (fileIndex: number, hunkIndex: number) => void;
    /** Discard this hunk (workdir-only; checkpointed upstream). */
    onDiscardHunk?: (fileIndex: number, hunkIndex: number) => void;
    filePath?: string | null;
    stageLabel?: string;
  } = $props();

  /**
   * Syntax tokens when Shiki has them cached (M11): replaces word-level
   * highlights for that line. Synchronous only — the render path never
   * awaits; DiffViewer schedules the background parse.
   */
  function syntax(line: DiffLine): TokenSpan[] | null {
    if (filePath === null) return null;
    return cachedTokens(line.text, filePath);
  }
</script>

<!-- Line text: syntax tokens win over word-level highlights. Kept on one
     line: the cells are white-space:pre, so stray template whitespace would
     render as spaces. -->
{#snippet text(line: DiffLine)}
  {@const tokens = syntax(line)}
  {#if tokens !== null}
    {#each tokens as token}{#if token.color}<span style:color={token.color}>{token.text}</span>{:else}{token.text}{/if}{/each}
  {:else if line.highlights.length > 0}
    {#each applyHighlights(line.text, line.highlights) as seg}{#if seg.hl}<span class="hl">{seg.text}</span>{:else}{seg.text}{/if}{/each}
  {:else}
    {line.text}
  {/if}
{/snippet}

{#if row.kind === "file-header"}
  <div class="row file-header" style:height={`${rowHeight(row)}px`}>
    <button
      type="button"
      class="collapse"
      aria-expanded={!row.collapsed}
      aria-label={row.collapsed ? `Expand ${row.file.path}` : `Collapse ${row.file.path}`}
      onclick={() => onToggleCollapse?.(row.file.path)}
    ><span class="chev" aria-hidden="true">{row.collapsed ? "▸" : "▾"}</span></button>
    <span class="path" title={row.file.path}>
      {#if row.file.old_path && row.file.old_path !== row.file.path}
        <span class="old-path">{row.file.old_path}</span><span class="arrow" aria-hidden="true"> → </span>{row.file.path}
      {:else}{row.file.path}{/if}
    </span>
    {#if row.file.is_image}<span class="chip">image</span>{/if}
    {#if row.file.binary}<span class="chip">binary</span>{/if}
    <span class="counts">
      <span class="adds">+{row.file.additions}</span>
      <span class="dels">−{row.file.deletions}</span>
    </span>
  </div>

{:else if row.kind === "hunk-header"}
  <!-- Hunk headers are deliberate keyboard focus stops (spec'd a11y):
       tabbing through a diff lands on each hunk boundary. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="row hunk-header"
    style:height={`${rowHeight(row)}px`}
    tabindex="0"
    role="heading"
    aria-level="3"
    aria-label={`Hunk: old lines ${row.oldStart} to ${row.oldStart + row.oldCount - 1}, new lines ${row.newStart} to ${row.newStart + row.newCount - 1}`}
  >
    <span class="hh">@@ -{row.oldStart},{row.oldCount} +{row.newStart},{row.newCount} @@</span>
    {#if onStageHunk || onDiscardHunk}
      <span class="hunk-actions">
        {#if onStageHunk}
          <button
            type="button"
            onclick={() => onStageHunk?.(row.fileIndex, row.hunkIndex)}
          >{stageLabel}</button>
        {/if}
        {#if onDiscardHunk}
          <button
            type="button"
            class="discard"
            onclick={() => onDiscardHunk?.(row.fileIndex, row.hunkIndex)}
          >Discard hunk</button>
        {/if}
      </span>
    {/if}
  </div>

{:else if row.kind === "context"}
  <div class="row line context" style:height={`${rowHeight(row)}px`}>
    <span class="no">{row.line.old_no ?? ""}</span>
    <span class="no">{row.line.new_no ?? ""}</span>
    <span class="txt">{@render text(row.line)}</span>
  </div>

{:else if row.kind === "pair"}
  <div class="row line pair" style:height={`${rowHeight(row)}px`}>
    <div class="half{row.left ? " del" : " filler"}">
      {#if row.left}
        <span class="no">{row.left.old_no ?? ""}</span>
        <span class="sign del" aria-hidden="true">−</span>
        <span class="txt">{@render text(row.left)}</span>
      {/if}
    </div>
    <div class="half right{row.right ? " add" : " filler"}">
      {#if row.right}
        <span class="no">{row.right.new_no ?? ""}</span>
        <span class="sign add" aria-hidden="true">+</span>
        <span class="txt">{@render text(row.right)}</span>
      {/if}
    </div>
  </div>

{:else if row.kind === "line"}
  {@const isAdd = row.line.origin === "+"}
  {@const isDel = row.line.origin === "-"}
  <div class="row line single{isAdd ? " add" : isDel ? " del" : ""}" style:height={`${rowHeight(row)}px`}>
    <span class="no">{row.line.old_no ?? ""}</span>
    <span class="no">{row.line.new_no ?? ""}</span>
    <span class="sign{isAdd ? " add" : isDel ? " del" : ""}" aria-hidden="true">{row.line.origin.trim()}</span>
    <span class="txt">{@render text(row.line)}</span>
  </div>

{:else if row.kind === "binary"}
  <div class="row binary" style:height={`${rowHeight(row)}px`}>
    <span class="binary-note">Binary file — changes not shown</span>
  </div>

{:else if row.kind === "image"}
  <div class="row image" style:height={`${rowHeight(row)}px`}>
    <ImageCompare file={row.file} {onLoadImage} />
  </div>
{/if}

<style>
  .row {
    display: flex;
    width: 100%;
    box-sizing: border-box;
  }

  /* ---- gutters + text (shared) ------------------------------------------ */
  .no,
  .sign,
  .txt,
  .hh {
    font-family: ui-monospace, "Cascadia Mono", Consolas, "Courier New", monospace;
    font-size: 12px;
    line-height: 20px;
    white-space: pre;
  }

  .no {
    flex: none;
    min-width: 3.25rem;
    padding-right: 0.5rem;
    text-align: right;
    color: var(--m3-on-surface-variant);
    font-size: 11px;
    user-select: none;
    overflow: hidden;
    text-overflow: clip;
  }

  .sign {
    flex: none;
    width: 1.25rem;
    text-align: center;
    user-select: none;
  }

  .txt {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    padding-right: 1rem;
  }

  /* ---- line coloring ------------------------------------------------------ */
  .row.line.add > *,
  .half.add > * {
    background: var(--diff-add-bg);
  }
  .row.line.del > *,
  .half.del > * {
    background: var(--diff-del-bg);
  }

  .half.filler {
    background: var(--diff-empty-bg);
  }

  .sign.del { color: var(--m3-error); font-weight: 600; }
  .sign.add { color: var(--m3-primary); font-weight: 600; }

  /* Word-level highlight: <mark>-equivalent span, no UA <mark> surprises. */
  .hl {
    background: var(--diff-hl-bg);
    color: var(--m3-on-tertiary-container);
    border-radius: 2px;
  }

  /* ---- split pair halves -------------------------------------------------- */
  .half {
    flex: 1 1 50%;
    min-width: 0;
    display: flex;
  }
  .half.right {
    border-left: 1px solid var(--diff-divider);
  }

  .row.context {
    position: relative;
  }
  .row.context .txt {
    flex: 1;
  }
  .row.context::after {
    content: "";
    position: absolute;
    left: 50%;
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--diff-divider);
  }

  /* ---- unified single line ------------------------------------------------ */
  .row.single {
    display: flex;
  }

  /* ---- file header --------------------------------------------------------- */
  .row.file-header {
    align-items: center;
    gap: 0.5rem;
    padding: 0 0.75rem 0 0.25rem;
    background: var(--m3-surface-container, var(--m3-surface));
    border-block: 1px solid var(--diff-divider);
  }

  .collapse {
    flex: none;
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: transparent;
    color: var(--m3-on-surface-variant);
    font-size: 11px;
    cursor: pointer;
  }
  .collapse:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }
  .collapse:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.8125rem;
    font-weight: 500;
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  }
  .old-path {
    color: var(--m3-on-surface-variant);
  }
  .arrow {
    color: var(--m3-on-surface-variant);
  }

  .chip {
    flex: none;
    font-size: 0.625rem;
    line-height: 1.4;
    padding: 0 0.4rem;
    border: 1px solid var(--m3-outline-variant);
    border-radius: 999px;
    color: var(--m3-on-surface-variant);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .counts {
    flex: none;
    display: flex;
    gap: 0.5rem;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
  }
  .adds { color: var(--m3-primary); }
  .dels { color: var(--m3-error); }

  /* ---- hunk header (focus stop) ------------------------------------------- */
  .row.hunk-header {
    align-items: center;
    padding: 0 0.75rem;
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-bottom: 1px solid var(--diff-divider);
  }
  .hh {
    font-size: 11px;
    color: var(--m3-on-surface-variant);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row.hunk-header:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }
  .hunk-actions {
    display: none;
    gap: 0.25rem;
    margin-left: auto;
    flex: none;
  }
  .row.hunk-header:hover .hunk-actions,
  .row.hunk-header:focus-within .hunk-actions,
  .row.hunk-header:focus-visible .hunk-actions {
    display: inline-flex;
  }
  .hunk-actions button {
    border: 1px solid var(--m3-outline-variant, transparent);
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface, none);
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 10px;
    padding: 0.05rem 0.4rem;
    cursor: pointer;
  }
  .hunk-actions button.discard {
    color: var(--m3-error);
  }

  /* ---- placeholders -------------------------------------------------------- */
  .row.binary {
    align-items: center;
    justify-content: center;
  }
  .binary-note {
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
  }

  .row.image {
    display: block;
  }
</style>
