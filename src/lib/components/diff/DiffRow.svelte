<script lang="ts">
  /**
   * One virtualized row of the diff (see rowModel.ts for the row kinds).
   * Heights are inline — from ROW_HEIGHTS by default, or the viewer's
   * wrap-aware estimate/correction via `heightPx` — so CSS can never
   * disagree with the virtualizer's uniform-height buckets.
   *
   * Layout:
 *   split    [ old# | - | text ][ new# | + | text ]  (equal halves)
 *   context  [ old# | text ][ new# | text ]  (same line, clipped per half)
   *   unified  [ old# | new# | sign | text ]
   */
  import type { DiffLine } from "$lib/ipc/types";
  import { applyHighlights } from "$lib/components/diff/bytes";
  import { rowHeight, type DiffRow, type LoadImageFn } from "$lib/components/diff/rowModel";
  import { cachedTokens, type TokenSpan } from "$lib/diff/highlight";
  import ImageCompare from "$lib/components/diff/ImageCompare.svelte";

  let {
    row,
    wrap = false,
    /** Global row index (with `onMeasure`, for wrap-height correction). */
    rowIndex = undefined,
    /** Explicit row height (wrap mode); default = the fixed rowHeight(). */
    heightPx = undefined,
    /** Wrap mode: report this row's rendered text height (rAF-batched upstream). */
    onMeasure = undefined,
    onToggleCollapse,
    onLoadImage,
    onStageHunk,
    onDiscardHunk,
    /** Owning file path (syntax highlighting); null disables it. */
    filePath = null,
    /** Hunk-header staging button label ("Unstage hunk" when unstaging). */
    stageLabel = "Stage hunk",
    /** M12 line selection: O(1) membership check for a hunk line index. */
    selectedAt = undefined,
    /** M12 line selection: a gutter was clicked (extend = shift-click). */
    onGutterClick = undefined,
    /** M12 line selection: lines selected in THIS hunk (0 = none). */
    selectionCount = 0,
    /** M12 line selection: stage/unstage button label for the selection. */
    selectionStageLabel = "Stage selected",
    /** M12 line selection: stage/unstage the selected ranges of this hunk. */
    onStageSelection = undefined,
    /** M12 line selection: discard the selected ranges of this hunk. */
    onDiscardSelection = undefined,
    /** M12 AI: explain this hunk (viewer opens its explain panel). */
    onExplainHunk = undefined,
  }: {
    row: DiffRow;
    wrap?: boolean;
    rowIndex?: number;
    heightPx?: number;
    onMeasure?: (rowIndex: number, textHeight: number) => void;
    onToggleCollapse?: (path: string) => void;
    onLoadImage?: LoadImageFn;
    /** Stage/unstage this hunk. */
    onStageHunk?: (fileIndex: number, hunkIndex: number) => void;
    /** Discard this hunk (workdir-only; checkpointed upstream). */
    onDiscardHunk?: (fileIndex: number, hunkIndex: number) => void;
    filePath?: string | null;
    stageLabel?: string;
    selectedAt?:
      | ((fileIndex: number, hunkIndex: number, lineIndex: number | null) => boolean)
      | undefined;
    onGutterClick?:
      | ((fileIndex: number, hunkIndex: number, lineIndex: number, extend: boolean) => void)
      | undefined;
    selectionCount?: number;
    selectionStageLabel?: string;
    onStageSelection?: ((fileIndex: number, hunkIndex: number) => void) | undefined;
    onDiscardSelection?: ((fileIndex: number, hunkIndex: number) => void) | undefined;
    onExplainHunk?: ((fileIndex: number, hunkIndex: number) => void) | undefined;
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

  /** M12: gutter click → selection start/extend (rows carry their owner). */
  function gutter(fileIndex: number, hunkIndex: number, lineIndex: number, event: MouseEvent): void {
    if (!onGutterClick) return;
    onGutterClick(fileIndex, hunkIndex, lineIndex, event.shiftKey);
  }

  /** M12: membership check bound to this row's hunk. */
  function selectedInRow(lineIndex: number | null): boolean {
    if (lineIndex === null || !selectedAt) return false;
    if (row.kind !== "line" && row.kind !== "context" && row.kind !== "pair") return false;
    return selectedAt(row.fileIndex, row.hunkIndex, lineIndex);
  }

  /**
   * Wrap-height correction: the row root's height is forced inline (the
   * virtualizer's contract), so the natural height is the tallest `.txt`
   * span. Reported upstream only in wrap mode (viewer gates the handler).
   */
  function measure(node: HTMLElement): { destroy: () => void } {
    const report = (): void => {
      // Re-checked per callback: the viewer passes undefined once wrap is
      // off, but rows stay mounted and their observers keep firing.
      if (!onMeasure || rowIndex === undefined) return;
      let max = 0;
      for (const el of node.querySelectorAll<HTMLElement>(".txt")) {
        max = Math.max(max, el.offsetHeight);
      }
      if (max > 0) onMeasure(rowIndex, max);
    };
    const ro = new ResizeObserver(report);
    ro.observe(node);
    for (const el of node.querySelectorAll<HTMLElement>(".txt")) ro.observe(el);
    return { destroy: () => ro.disconnect() };
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
    class:has-selection={selectionCount > 0}
    style:height={`${rowHeight(row)}px`}
    tabindex="0"
    role="heading"
    aria-level="3"
    aria-label={`Hunk: old lines ${row.oldStart} to ${row.oldStart + row.oldCount - 1}, new lines ${row.newStart} to ${row.newStart + row.newCount - 1}`}
  >
    <span class="hh">@@ -{row.oldStart},{row.oldCount} +{row.newStart},{row.newCount} @@</span>
    {#if selectionCount > 0}
      <span class="sel-count" aria-live="polite">{selectionCount} selected</span>
    {/if}
    <span class="hunk-actions">
      {#if selectionCount > 0 && onStageSelection}
        <button
          type="button"
          class="sel"
          onclick={(e) => {
            e.stopPropagation();
            onStageSelection?.(row.fileIndex, row.hunkIndex);
          }}
        >{selectionStageLabel}</button>
      {/if}
      {#if selectionCount > 0 && onDiscardSelection}
        <button
          type="button"
          class="sel discard"
          onclick={(e) => {
            e.stopPropagation();
            onDiscardSelection?.(row.fileIndex, row.hunkIndex);
          }}
        >Discard selected</button>
      {/if}
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
      {#if onExplainHunk}
        <button
          type="button"
          class="explain"
          title="Explain this change with AI"
          onclick={(e) => {
            e.stopPropagation();
            onExplainHunk?.(row.fileIndex, row.hunkIndex);
          }}
        >Explain</button>
      {/if}
    </span>
  </div>

{:else if row.kind === "context"}
  <!-- Context renders the line in BOTH halves (like GitHub/VS Code split):
       each side clips at the divider, so text never crosses into the other
       section. The empty sign spacer keeps text columns aligned with the
       pair rows directly above/below. `data-row-index` addresses the row
       for the viewer's split-selection highlights. -->
  <div
    class="row line context"
    data-row-index={rowIndex}
    style:height={`${heightPx ?? rowHeight(row)}px`}
    use:measure
  >
    <div class="half" class:selected={selectedInRow(row.lineIndex)}>
      <button
        type="button"
        class="no" class:click={onGutterClick !== undefined}
        tabindex="-1"
        aria-label="Select line {row.line.old_no ?? ""}"
        onclick={(e) => {
          e.stopPropagation();
          gutter(row.fileIndex, row.hunkIndex, row.lineIndex, e);
        }}
      >{row.line.old_no ?? ""}</button>
      <span class="sign" aria-hidden="true"></span>
      <span class="txt" class:wrap>{@render text(row.line)}</span>
    </div>
    <div class="half right" class:selected={selectedInRow(row.lineIndex)}>
      <button
        type="button"
        class="no" class:click={onGutterClick !== undefined}
        tabindex="-1"
        aria-label="Select line {row.line.new_no ?? ""}"
        onclick={(e) => {
          e.stopPropagation();
          gutter(row.fileIndex, row.hunkIndex, row.lineIndex, e);
        }}
      >{row.line.new_no ?? ""}</button>
      <span class="sign" aria-hidden="true"></span>
      <span class="txt" class:wrap>{@render text(row.line)}</span>
    </div>
  </div>

{:else if row.kind === "pair"}
  <div
    class="row line pair"
    data-row-index={rowIndex}
    style:height={`${heightPx ?? rowHeight(row)}px`}
    use:measure
  >
    <div
      class="half{row.left ? " del" : " filler"}"
      class:selected={row.left !== null && selectedInRow(row.leftIndex)}
    >
      {#if row.left}
        <button
          type="button"
          class="no" class:click={onGutterClick !== undefined}
          tabindex="-1"
          aria-label="Select line {row.left.old_no ?? ""}"
          onclick={(e) => {
            e.stopPropagation();
            gutter(row.fileIndex, row.hunkIndex, row.leftIndex ?? -1, e);
          }}
        >{row.left.old_no ?? ""}</button>
        <span class="sign del" aria-hidden="true">−</span>
        <span class="txt" class:wrap>{@render text(row.left)}</span>
      {/if}
    </div>
    <div
      class="half right{row.right ? " add" : " filler"}"
      class:selected={row.right !== null && selectedInRow(row.rightIndex)}
    >
      {#if row.right}
        <button
          type="button"
          class="no" class:click={onGutterClick !== undefined}
          tabindex="-1"
          aria-label="Select line {row.right.new_no ?? ""}"
          onclick={(e) => {
            e.stopPropagation();
            gutter(row.fileIndex, row.hunkIndex, row.rightIndex ?? -1, e);
          }}
        >{row.right.new_no ?? ""}</button>
        <span class="sign add" aria-hidden="true">+</span>
        <span class="txt" class:wrap>{@render text(row.right)}</span>
      {/if}
    </div>
  </div>

{:else if row.kind === "line"}
  {@const isAdd = row.line.origin === "+"}
  {@const isDel = row.line.origin === "-"}
  <div
    class="row line single{isAdd ? " add" : isDel ? " del" : ""}"
    class:selected={selectedInRow(row.lineIndex)}
    data-row-index={rowIndex}
    style:height={`${heightPx ?? rowHeight(row)}px`}
    use:measure
  >
    <button
      type="button"
      class="no" class:click={onGutterClick !== undefined}
      tabindex="-1"
      aria-label="Select line {row.line.old_no ?? ""}"
      onclick={(e) => {
        e.stopPropagation();
        gutter(row.fileIndex, row.hunkIndex, row.lineIndex, e);
      }}
    >{row.line.old_no ?? ""}</button>
    <button
      type="button"
      class="no" class:click={onGutterClick !== undefined}
      tabindex="-1"
      aria-label="Select line {row.line.new_no ?? ""}"
      onclick={(e) => {
        e.stopPropagation();
        gutter(row.fileIndex, row.hunkIndex, row.lineIndex, e);
      }}
    >{row.line.new_no ?? ""}</button>
    <span class="sign{isAdd ? " add" : isDel ? " del" : ""}" aria-hidden="true">{row.line.origin.trim()}</span>
    <span class="txt" class:wrap>{@render text(row.line)}</span>
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
    tab-size: 8; /* must match textWidth.ts (tab-stop math) */
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

  /* M12: gutters. tabindex=-1 keeps them OUT of the tab order — hunk
     headers remain the only deliberate stops. The `click` affordance only
     applies when the viewer actually wired a selection handler. */
  button.no {
    border: none;
    background: none;
    padding: 0 0.5rem 0 0;
    margin: 0;
    font: inherit;
  }
  button.no.click {
    cursor: pointer;
  }
  button.no.click:hover {
    color: var(--m3-on-surface);
    text-decoration: underline;
  }
  button.no.click:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  /* M12: selected-line highlight (wins over the add/del wash). */
  .row.line.selected > *,
  .half.selected > * {
    background: var(--diff-sel-bg, var(--diff-hl-bg));
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

  /* Wrap mode (viewer's default): long lines fold inside the pane instead of
     scrolling. break-all keeps the wrap estimate in wrapHeights.ts exact for
     uniform-width glyphs (no word-boundary preference to diverge from it). */
  .txt.wrap {
    white-space: pre-wrap;
    word-break: break-all;
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

  /* Context rows use the same two-half layout as pairs; the right half's
     border-left is the divider, so no ::after overlay is needed. */

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
  .row.hunk-header:focus-visible .hunk-actions,
  /* An active line selection keeps its actions visible without hover. */
  .row.hunk-header.has-selection .hunk-actions {
    display: inline-flex;
  }
  .sel-count {
    flex: none;
    font-size: 10px;
    color: var(--m3-primary);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
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
  .hunk-actions button.sel {
    border-color: var(--m3-primary);
    color: var(--m3-primary);
    font-weight: 600;
  }
  .hunk-actions button.explain {
    color: var(--m3-tertiary, var(--m3-primary));
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
