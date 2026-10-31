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
   *   single   [ line# | sign | text ]  (one-sided file, full width)
   *   unified  [ old# | new# | sign | text ]
   */
  import type { DiffLine } from "$lib/ipc/types";
  import { applyHighlights } from "$lib/components/diff/bytes";
  import {
    fileChangeKind,
    fileKindLabel,
    fileKindLetter,
    rowHeight,
    type DiffRow,
    type LoadImageFn,
  } from "$lib/components/diff/rowModel";
  import { cachedTokens, type TokenSpan } from "$lib/diff/highlight";
  import { observeRowMeasure } from "$lib/components/diff/rowMeasure";
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
    /** Token-cache revision: bumping it repaints rows once parses land. */
    syntaxRevision = 0,
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
    syntaxRevision?: number;
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
    // Render-time dependency on the token revision: background parses only
    // land in the cache (non-reactive), so rows must be told to repaint.
    void syntaxRevision;
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
    if (row.kind !== "line" && row.kind !== "single" && row.kind !== "context" && row.kind !== "pair") return false;
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
      // off, but rows stay mounted and their observations keep firing.
      if (!onMeasure || rowIndex === undefined) return;
      let max = 0;
      for (const el of node.querySelectorAll<HTMLElement>(".txt")) {
        max = Math.max(max, el.offsetHeight);
      }
      if (max > 0) onMeasure(rowIndex, max);
    };
    return { destroy: observeRowMeasure(node, report) };
  }

  /**
   * Line wash + stronger gutter tint for add/del rows, as Tailwind arbitrary
   * utilities targeting the row's/half's children (replaces the old
   * `.add > *` / `.add > .no` scoped selectors).
   */
  function washCls(kind: "add" | "del"): string {
    return kind === "add"
      ? "[&>*]:bg-[var(--diff-add-bg)] [&>.no]:bg-[var(--diff-add-gutter)]"
      : "[&>*]:bg-[var(--diff-del-bg)] [&>.no]:bg-[var(--diff-del-gutter)]";
  }

  /** M12 selected-line wash — overrides the add/del wash on every child. */
  function selCls(active: boolean): string {
    return active ? "[&>*]:bg-[var(--diff-sel-bg)]!" : "";
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
    {#each applyHighlights(line.text, line.highlights) as seg}{#if seg.hl}<span class="hl rounded-[2px] bg-[var(--diff-hl-bg)]">{seg.text}</span>{:else}{seg.text}{/if}{/each}
  {:else}
    {line.text}
  {/if}
{/snippet}

{#if row.kind === "file-header"}
  {@const change = fileChangeKind(row.file)}
  <div class="row file-header flex w-full box-border items-center gap-[0.4rem] border-y border-[var(--diff-divider)] bg-[var(--diff-file-bg)] py-0 pl-1 pr-3 shadow-[inset_3px_0_var(--diff-file-accent)]" style:height={`${rowHeight(row)}px`}>
    <button
      type="button"
      class="collapse grid h-[22px] w-[22px] shrink-0 cursor-pointer place-items-center rounded-[4px] border-0 bg-transparent p-0 text-[11px] text-muted-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
      aria-expanded={!row.collapsed}
      aria-label={row.collapsed ? `Expand ${row.file.path}` : `Collapse ${row.file.path}`}
      onclick={() => onToggleCollapse?.(row.file.path)}
    ><span class="chev" aria-hidden="true">{row.collapsed ? "▸" : "▾"}</span></button>
    <span
      class={`fkind fk-${change} inline-flex h-[1.05rem] w-[1.05rem] shrink-0 items-center justify-center rounded-[3px] font-mono text-[0.625rem] leading-none font-bold ${
        change === "added"
          ? "text-[var(--app-added)] bg-[var(--app-added-bg)]"
          : change === "deleted"
            ? "text-[var(--app-deleted)] bg-[var(--app-deleted-bg)]"
            : change === "renamed"
              ? "text-[var(--app-renamed)] bg-[var(--app-renamed-bg)]"
              : "text-[var(--app-modified)] bg-[var(--app-modified-bg)]"
      }`}
      title={fileKindLabel(change)}
      aria-label={fileKindLabel(change)}
    >{fileKindLetter(change)}</span>
    <span class="path min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.8125rem] font-semibold" title={row.file.path}>
      {#if row.file.old_path && row.file.old_path !== row.file.path}
        <span class="old-path text-muted-foreground">{row.file.old_path}</span><span class="arrow text-muted-foreground" aria-hidden="true"> → </span>{row.file.path}
      {:else}{row.file.path}{/if}
    </span>
    {#if row.file.is_image}<span class="chip shrink-0 rounded-full border border-border px-[0.4rem] text-[0.625rem] leading-[1.4] uppercase tracking-[0.04em] text-muted-foreground">image</span>{/if}
    {#if row.file.binary}<span class="chip shrink-0 rounded-full border border-border px-[0.4rem] text-[0.625rem] leading-[1.4] uppercase tracking-[0.04em] text-muted-foreground">binary</span>{/if}
    <span class="counts flex shrink-0 gap-[0.3rem] font-mono text-[0.6875rem] tabular-nums">
      <span class="adds rounded-[3px] px-1 text-[var(--app-added)] bg-[var(--app-added-bg)]">+{row.file.additions}</span>
      <span class="dels rounded-[3px] px-1 text-[var(--app-deleted)] bg-[var(--app-deleted-bg)]">−{row.file.deletions}</span>
    </span>
  </div>

{:else if row.kind === "hunk-header"}
  <!-- Hunk headers are deliberate keyboard focus stops (spec'd a11y):
       tabbing through a diff lands on each hunk boundary. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="row hunk-header group/hh flex w-full box-border items-center gap-2 border-b border-[var(--diff-divider)] bg-[var(--diff-hunk-bg)] px-3 shadow-[inset_2px_0_var(--diff-hunk-accent)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
    class:has-selection={selectionCount > 0}
    style:height={`${rowHeight(row)}px`}
    tabindex="0"
    role="heading"
    aria-level="3"
    aria-label={`Hunk: old lines ${row.oldStart} to ${row.oldStart + row.oldCount - 1}, new lines ${row.newStart} to ${row.newStart + row.newCount - 1}`}
  >
    <span class="hh overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[11px] leading-[20px] font-medium text-muted-foreground [tab-size:8] group-hover/hh:text-foreground">@@ -{row.oldStart},{row.oldCount} +{row.newStart},{row.newCount} @@</span>
    {#if selectionCount > 0}
      <span class="sel-count shrink-0 whitespace-nowrap text-[10px] tabular-nums text-primary" aria-live="polite">{selectionCount} selected</span>
    {/if}
    <span class="hunk-actions ml-auto shrink-0 items-center gap-1 {selectionCount > 0 ? "inline-flex" : "hidden"} group-hover/hh:inline-flex group-focus-within/hh:inline-flex group-focus-visible/hh:inline-flex">
      {#if selectionCount > 0 && onStageSelection}
        <button
          type="button"
          class="sel cursor-pointer rounded-lg border border-primary bg-background px-[0.4rem] py-[0.05rem] font-sans text-[10px] font-semibold text-primary"
          onclick={(e) => {
            e.stopPropagation();
            onStageSelection?.(row.fileIndex, row.hunkIndex);
          }}
        >{selectionStageLabel}</button>
      {/if}
      {#if selectionCount > 0 && onDiscardSelection}
        <button
          type="button"
          class="sel discard cursor-pointer rounded-lg border border-primary bg-background px-[0.4rem] py-[0.05rem] font-sans text-[10px] font-semibold text-destructive"
          onclick={(e) => {
            e.stopPropagation();
            onDiscardSelection?.(row.fileIndex, row.hunkIndex);
          }}
        >Discard selected</button>
      {/if}
      {#if onStageHunk}
        <button
          type="button"
          class="cursor-pointer rounded-lg border border-border bg-background px-[0.4rem] py-[0.05rem] font-sans text-[10px] text-foreground"
          onclick={() => onStageHunk?.(row.fileIndex, row.hunkIndex)}
        >{stageLabel}</button>
      {/if}
      {#if onDiscardHunk}
        <button
          type="button"
          class="discard cursor-pointer rounded-lg border border-border bg-background px-[0.4rem] py-[0.05rem] font-sans text-[10px] text-destructive"
          onclick={() => onDiscardHunk?.(row.fileIndex, row.hunkIndex)}
        >Discard hunk</button>
      {/if}
      {#if onExplainHunk}
        <button
          type="button"
          class="explain cursor-pointer rounded-lg border border-border bg-background px-[0.4rem] py-[0.05rem] font-sans text-[10px] text-[var(--app-renamed)]"
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
    class="row line context flex w-full box-border"
    data-row-index={rowIndex}
    style:height={`${heightPx ?? rowHeight(row)}px`}
    use:measure
  >
    <div
      class="half flex min-w-0 flex-[1_1_50%] {selCls(selectedInRow(row.lineIndex))}"
      class:selected={selectedInRow(row.lineIndex)}
    >
      <button
        type="button"
        class="no m-0 min-w-[3.25rem] shrink-0 select-none overflow-hidden border-0 bg-transparent p-0 pr-2 text-right font-mono text-[11px] leading-[20px] text-clip whitespace-pre [tab-size:8] text-muted-foreground {onGutterClick !== undefined
          ? "click cursor-pointer hover:text-foreground hover:underline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
          : ""}"
        tabindex="-1"
        aria-label="Select line {row.line.old_no ?? ""}"
        onclick={(e) => {
          e.stopPropagation();
          gutter(row.fileIndex, row.hunkIndex, row.lineIndex, e);
        }}
      >{row.line.old_no ?? ""}</button>
      <span class="sign w-5 shrink-0 select-none text-center font-mono text-[12px] leading-[20px] whitespace-pre [tab-size:8]" aria-hidden="true"></span>
      <span class="txt min-w-0 flex-1 overflow-hidden pr-4 font-mono text-[12px] leading-[20px] [tab-size:8] {wrap ? "whitespace-pre-wrap break-all" : "whitespace-pre"}" class:wrap={wrap}>{@render text(row.line)}</span>
    </div>
    <div
      class="half right flex min-w-0 flex-[1_1_50%] border-l border-[var(--diff-divider)] {selCls(selectedInRow(row.lineIndex))}"
      class:selected={selectedInRow(row.lineIndex)}
    >
      <button
        type="button"
        class="no m-0 min-w-[3.25rem] shrink-0 select-none overflow-hidden border-0 bg-transparent p-0 pr-2 text-right font-mono text-[11px] leading-[20px] text-clip whitespace-pre [tab-size:8] text-muted-foreground {onGutterClick !== undefined
          ? "click cursor-pointer hover:text-foreground hover:underline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
          : ""}"
        tabindex="-1"
        aria-label="Select line {row.line.new_no ?? ""}"
        onclick={(e) => {
          e.stopPropagation();
          gutter(row.fileIndex, row.hunkIndex, row.lineIndex, e);
        }}
      >{row.line.new_no ?? ""}</button>
      <span class="sign w-5 shrink-0 select-none text-center font-mono text-[12px] leading-[20px] whitespace-pre [tab-size:8]" aria-hidden="true"></span>
      <span class="txt min-w-0 flex-1 overflow-hidden pr-4 font-mono text-[12px] leading-[20px] [tab-size:8] {wrap ? "whitespace-pre-wrap break-all" : "whitespace-pre"}" class:wrap={wrap}>{@render text(row.line)}</span>
    </div>
  </div>

{:else if row.kind === "pair"}
  <div
    class="row line pair flex w-full box-border"
    data-row-index={rowIndex}
    style:height={`${heightPx ?? rowHeight(row)}px`}
    use:measure
  >
    <div
      class="half flex min-w-0 flex-[1_1_50%] {row.left
        ? `del ${washCls("del")} ${selCls(selectedInRow(row.leftIndex))}`
        : "filler bg-[var(--diff-empty-bg)]"}"
      class:selected={row.left !== null && selectedInRow(row.leftIndex)}
    >
      {#if row.left}
        <button
          type="button"
          class="no m-0 min-w-[3.25rem] shrink-0 select-none overflow-hidden border-0 bg-transparent p-0 pr-2 text-right font-mono text-[11px] leading-[20px] text-clip whitespace-pre [tab-size:8] text-muted-foreground {onGutterClick !== undefined
            ? "click cursor-pointer hover:text-foreground hover:underline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            : ""}"
          tabindex="-1"
          aria-label="Select line {row.left.old_no ?? ""}"
          onclick={(e) => {
            e.stopPropagation();
            gutter(row.fileIndex, row.hunkIndex, row.leftIndex ?? -1, e);
          }}
        >{row.left.old_no ?? ""}</button>
        <span class="sign del w-5 shrink-0 select-none text-center font-mono text-[12px] leading-[20px] font-bold whitespace-pre [tab-size:8] text-[var(--app-deleted)]" aria-hidden="true">−</span>
        <span class="txt min-w-0 flex-1 overflow-hidden pr-4 font-mono text-[12px] leading-[20px] [tab-size:8] {wrap ? "whitespace-pre-wrap break-all" : "whitespace-pre"}" class:wrap={wrap}>{@render text(row.left)}</span>
      {/if}
    </div>
    <div
      class="half right flex min-w-0 flex-[1_1_50%] border-l border-[var(--diff-divider)] {row.right
        ? `add ${washCls("add")} ${selCls(selectedInRow(row.rightIndex))}`
        : "filler bg-[var(--diff-empty-bg)]"}"
      class:selected={row.right !== null && selectedInRow(row.rightIndex)}
    >
      {#if row.right}
        <button
          type="button"
          class="no m-0 min-w-[3.25rem] shrink-0 select-none overflow-hidden border-0 bg-transparent p-0 pr-2 text-right font-mono text-[11px] leading-[20px] text-clip whitespace-pre [tab-size:8] text-muted-foreground {onGutterClick !== undefined
            ? "click cursor-pointer hover:text-foreground hover:underline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            : ""}"
          tabindex="-1"
          aria-label="Select line {row.right.new_no ?? ""}"
          onclick={(e) => {
            e.stopPropagation();
            gutter(row.fileIndex, row.hunkIndex, row.rightIndex ?? -1, e);
          }}
        >{row.right.new_no ?? ""}</button>
        <span class="sign add w-5 shrink-0 select-none text-center font-mono text-[12px] leading-[20px] font-bold whitespace-pre [tab-size:8] text-[var(--app-added)]" aria-hidden="true">+</span>
        <span class="txt min-w-0 flex-1 overflow-hidden pr-4 font-mono text-[12px] leading-[20px] [tab-size:8] {wrap ? "whitespace-pre-wrap break-all" : "whitespace-pre"}" class:wrap={wrap}>{@render text(row.right)}</span>
      {/if}
    </div>
  </div>

{:else if row.kind === "single"}
  {@const isAdd = row.line.origin === "+"}
  {@const isDel = row.line.origin === "-"}
  {@const lineNo = row.side === "old" ? row.line.old_no : row.line.new_no}
  <div
    class="row line single{isAdd ? " add" : isDel ? " del" : ""} flex w-full box-border {isAdd || isDel ? washCls(isAdd ? "add" : "del") : ""} {selCls(selectedInRow(row.lineIndex))}"
    class:selected={selectedInRow(row.lineIndex)}
    data-row-index={rowIndex}
    style:height={`${heightPx ?? rowHeight(row)}px`}
    use:measure
  >
    <button
      type="button"
      class="no m-0 min-w-[3.25rem] shrink-0 select-none overflow-hidden border-0 bg-transparent p-0 pr-2 text-right font-mono text-[11px] leading-[20px] text-clip whitespace-pre [tab-size:8] text-muted-foreground {onGutterClick !== undefined
        ? "click cursor-pointer hover:text-foreground hover:underline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        : ""}"
      tabindex="-1"
      aria-label="Select line {lineNo ?? ""}"
      onclick={(e) => {
        e.stopPropagation();
        gutter(row.fileIndex, row.hunkIndex, row.lineIndex, e);
      }}
    >{lineNo ?? ""}</button>
    <span class="sign{isAdd ? " add" : isDel ? " del" : ""} w-5 shrink-0 select-none text-center font-mono text-[12px] leading-[20px] whitespace-pre [tab-size:8] {isAdd ? "font-bold text-[var(--app-added)]" : isDel ? "font-bold text-[var(--app-deleted)]" : ""}" aria-hidden="true">{row.line.origin.trim()}</span>
    <span class="txt min-w-0 flex-1 overflow-hidden pr-4 font-mono text-[12px] leading-[20px] [tab-size:8] {wrap ? "whitespace-pre-wrap break-all" : "whitespace-pre"}" class:wrap={wrap}>{@render text(row.line)}</span>
  </div>

{:else if row.kind === "line"}
  {@const isAdd = row.line.origin === "+"}
  {@const isDel = row.line.origin === "-"}
  <div
    class="row line single{isAdd ? " add" : isDel ? " del" : ""} flex w-full box-border {isAdd || isDel ? washCls(isAdd ? "add" : "del") : ""} {selCls(selectedInRow(row.lineIndex))}"
    class:selected={selectedInRow(row.lineIndex)}
    data-row-index={rowIndex}
    style:height={`${heightPx ?? rowHeight(row)}px`}
    use:measure
  >
    <button
      type="button"
      class="no m-0 min-w-[3.25rem] shrink-0 select-none overflow-hidden border-0 bg-transparent p-0 pr-2 text-right font-mono text-[11px] leading-[20px] text-clip whitespace-pre [tab-size:8] text-muted-foreground {onGutterClick !== undefined
        ? "click cursor-pointer hover:text-foreground hover:underline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        : ""}"
      tabindex="-1"
      aria-label="Select line {row.line.old_no ?? ""}"
      onclick={(e) => {
        e.stopPropagation();
        gutter(row.fileIndex, row.hunkIndex, row.lineIndex, e);
      }}
    >{row.line.old_no ?? ""}</button>
    <button
      type="button"
      class="no m-0 min-w-[3.25rem] shrink-0 select-none overflow-hidden border-0 bg-transparent p-0 pr-2 text-right font-mono text-[11px] leading-[20px] text-clip whitespace-pre [tab-size:8] text-muted-foreground {onGutterClick !== undefined
        ? "click cursor-pointer hover:text-foreground hover:underline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        : ""}"
      tabindex="-1"
      aria-label="Select line {row.line.new_no ?? ""}"
      onclick={(e) => {
        e.stopPropagation();
        gutter(row.fileIndex, row.hunkIndex, row.lineIndex, e);
      }}
    >{row.line.new_no ?? ""}</button>
    <span class="sign{isAdd ? " add" : isDel ? " del" : ""} w-5 shrink-0 select-none text-center font-mono text-[12px] leading-[20px] whitespace-pre [tab-size:8] {isAdd ? "font-bold text-[var(--app-added)]" : isDel ? "font-bold text-[var(--app-deleted)]" : ""}" aria-hidden="true">{row.line.origin.trim()}</span>
    <span class="txt min-w-0 flex-1 overflow-hidden pr-4 font-mono text-[12px] leading-[20px] [tab-size:8] {wrap ? "whitespace-pre-wrap break-all" : "whitespace-pre"}" class:wrap={wrap}>{@render text(row.line)}</span>
  </div>

{:else if row.kind === "binary"}
  <div class="row binary flex w-full box-border items-center justify-center" style:height={`${rowHeight(row)}px`}>
    <span class="binary-note text-[0.8125rem] text-muted-foreground">Binary file — changes not shown</span>
  </div>

{:else if row.kind === "image"}
  <div class="row image block w-full" style:height={`${rowHeight(row)}px`}>
    <ImageCompare file={row.file} {onLoadImage} />
  </div>
{/if}
