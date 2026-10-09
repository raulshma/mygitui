<script lang="ts">
  /**
   * DiffViewer — the app's primary diff surface (wave-2 lane B1).
   *
   * Contract: `{ files: FileDiff[]; mode?: "split" | "unified" }` (mode is
   * additionally bindable; the internal toolbar button cycles it). Additive
   * optional prop: `onLoadImage` supplies real URLs for image diffs.
   *
   * Rendering is fully virtualized: buildRowModel flattens the files into
   * uniform-height row buckets (diff/rowModel.ts), buildLayout indexes them
   * (diff/virtualizer.ts), and only the visible window (± overscan) is ever
   * mounted. 10k-hunk diffs scroll at 60fps — the math is unit tested and
   * perf-gated in diff/virtualizer.test.ts.
   */
  import type { FileDiff } from "$lib/ipc/types";
  import { discard, stage } from "$lib/ipc/client";
  import { onUiEvent } from "$lib/palette/events";
  import { toast } from "$lib/toast";
  import { ai } from "$lib/ai/ai.svelte";
  import { formatHunkText } from "$lib/ai/features";
  import {
    onTokensLanded,
    requestTokens,
  } from "$lib/diff/highlight";
  import {
    buildRowModel,
    rowHeights,
    type DiffMode,
    type LoadImageFn,
  } from "$lib/components/diff/rowModel";
  import { buildLayout, visibleSlices } from "$lib/components/diff/virtualizer";
  import { measureLineWidth } from "$lib/components/diff/textWidth";
  import {
    beginSelection,
    extendSelection,
    isLineSelected,
    selectableAnchor,
    selectedSet as toSelectedSet,
    selectionRanges,
    type LineSelection,
  } from "$lib/components/diff/lineSelection";
  import DiffRow from "$lib/components/diff/DiffRow.svelte";
  import ExplainHunkPanel from "$lib/components/ai/ExplainHunkPanel.svelte";

  let {
    files,
    mode = $bindable("split"),
    onLoadImage,
    repoId = undefined,
    /** Hunk-level actions: `stage` = stage hunks, `unstage` = unstage hunks. */
    hunkStaging = undefined,
    /** Show "Discard hunk" buttons (workdir-only; backend checkpoints first). */
    hunkDiscard = false,
    /** Called after a successful hunk stage/unstage/discard. */
    onMutated = undefined,
  }: {
    files: FileDiff[];
    mode?: DiffMode;
    onLoadImage?: LoadImageFn;
    /** Required for hunk actions (targets the IPC by repo id). */
    repoId?: string;
    hunkStaging?: "stage" | "unstage";
    hunkDiscard?: boolean;
    onMutated?: () => void;
  } = $props();

  /** Rows rendered beyond the viewport (px, top and bottom). */
  const OVERSCAN_PX = 400;

  /** Collapsed file sections, keyed by path. */
  let collapsed: Record<string, boolean> = $state({});

  let viewportEl: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewportH = $state(0);

  const collapsedSet = $derived(new Set(Object.keys(collapsed).filter((p) => collapsed[p])));
  const model = $derived(buildRowModel(files, mode, collapsedSet));
  const layout = $derived(buildLayout(rowHeights(model.rows)));
  const slices = $derived(visibleSlices(layout, scrollTop, viewportH, OVERSCAN_PX));

  /**
   * Horizontal extent of the scroll content.
   *
   * Split halves share the pane 50/50 and clip long lines — the layout is
   * fully responsive and never grows a horizontal scrollbar. Unified fills
   * the pane (`100%` floor) and scrolls only when the widest line genuinely
   * exceeds it; the text width is measured in the rows' own monospace font
   * (see textWidth.ts), and 8.75rem covers both gutters + the sign column +
   * the text's right padding.
   */
  const contentMinWidth = $derived(
    mode === "unified"
      ? `max(100%, calc(8.75rem + ${measureLineWidth(model.maxLineText)}px))`
      : "100%",
  );

  function toggleCollapse(path: string): void {
    collapsed[path] = !collapsed[path];
  }

  function cycleMode(): void {
    mode = mode === "split" ? "unified" : "split";
  }

  // Palette command rides the bus; every mounted viewer flips its mode.
  $effect(() => onUiEvent("diff-toggle-mode", cycleMode));

  // -- M11: syntax highlighting (async Shiki, render-path never awaits) -------

  /** Global kill switch: palette command or preference can disable it. */
  let syntaxEnabled = $state(true);

  /**
   * Re-render trigger: bumps when background token parses land, so newly
   * highlighted visible rows repaint (tokens are read synchronously).
   */
  let syntaxTick = $state(0);
  $effect(() => {
    return onTokensLanded(() => syntaxTick++);
  });

  // Request parses for the visible window's lines (cheap: only uncached
  // entries actually hit Shiki).
  $effect(() => {
    if (!syntaxEnabled) return;
    void syntaxTick; // re-request after background parses land (next window)
    void mode;
    for (const file of files) {
      if (file.binary || file.is_image) continue;
      const texts: string[] = [];
      for (const hunk of file.hunks) {
        for (const line of hunk.lines) {
          texts.push(line.text);
        }
      }
      requestTokens(file.path, texts);
    }
  });

  // -- M9 F6: hunk staging / discard ------------------------------------------

  function describeError(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  function stageHunk(fileIndex: number, hunkIndex: number): void {
    if (!repoId || !hunkStaging) return;
    const file = files[fileIndex];
    if (!file) return;
    const unstage = hunkStaging === "unstage";
    stage(repoId, {
      targets: [{ hunk: { path: file.path, hunk: hunkIndex } }],
      unstage,
    })
      .then(() => onMutated?.())
      .catch((err: unknown) =>
        toast(`${unstage ? "Unstage" : "Stage"} failed: ${describeError(err)}`, {
          kind: "error",
        }),
      );
  }

  function discardHunk(fileIndex: number, hunkIndex: number): void {
    if (!repoId) return;
    const file = files[fileIndex];
    if (!file) return;
    discard(repoId, [{ hunk: { path: file.path, hunk: hunkIndex } }])
      .then(() => onMutated?.())
      .catch((err: unknown) =>
        toast(`Discard failed: ${describeError(err)}`, { kind: "error" }),
      );
  }

  // -- M12: line-granular selection -------------------------------------------
  //
  // ONE active selection per viewer, held here — outside the per-row render
  // path (rows only get derived booleans, so the 60fps virtualization is
  // untouched). Selection is only offered when the viewer can act on it:
  // staging (hunkStaging) and/or discarding (hunkDiscard), mirroring the
  // hunk-level gating props exactly.

  let selection = $state<LineSelection | null>(null);
  const selSet = $derived(toSelectedSet(selection));

  /** Selection is offered when any selection action is available. */
  const selectionEnabled = $derived(
    repoId !== undefined && (hunkStaging !== undefined || hunkDiscard),
  );

  function onGutterClick(
    fileIndex: number,
    hunkIndex: number,
    lineIndex: number,
    extend: boolean,
  ): void {
    const hunkLines = files[fileIndex]?.hunks[hunkIndex]?.lines;
    if (!hunkLines) return;
    const anchor = selectableAnchor(hunkLines, lineIndex);
    if (anchor === null) return;
    if (
      extend &&
      selection &&
      selection.fileIndex === fileIndex &&
      selection.hunkIndex === hunkIndex
    ) {
      selection = extendSelection(selection, anchor, hunkLines);
    } else {
      selection = beginSelection(fileIndex, hunkIndex, anchor);
    }
  }

  /** O(1)-ish per-row membership check handed down to DiffRow. */
  function selectedAt(
    fileIndex: number,
    hunkIndex: number,
    lineIndex: number | null,
  ): boolean {
    if (lineIndex === null || !selection) return false;
    if (selection.fileIndex !== fileIndex || selection.hunkIndex !== hunkIndex) {
      return false;
    }
    const hunkLines = files[fileIndex]?.hunks[hunkIndex]?.lines;
    if (!hunkLines) return false;
    return isLineSelected(hunkLines, lineIndex, selSet);
  }

  function selectionCountFor(fileIndex: number, hunkIndex: number): number {
    return selection &&
      selection.fileIndex === fileIndex &&
      selection.hunkIndex === hunkIndex
      ? selection.lines.length
      : 0;
  }

  function stageSelection(fileIndex: number, hunkIndex: number): void {
    if (!repoId || !hunkStaging || !selection) return;
    const file = files[fileIndex];
    if (!file) return;
    const unstage = hunkStaging === "unstage";
    stage(repoId, {
      targets: [
        {
          lines: {
            path: file.path,
            hunk: hunkIndex,
            ranges: selectionRanges(selection),
          },
        },
      ],
      unstage,
    })
      .then(() => {
        selection = null;
        onMutated?.();
      })
      .catch((err: unknown) =>
        toast(
          `${unstage ? "Unstage" : "Stage"} selected failed: ${describeError(err)}`,
          { kind: "error" },
        ),
      );
  }

  function discardSelection(fileIndex: number, hunkIndex: number): void {
    if (!repoId || !hunkDiscard || !selection) return;
    const file = files[fileIndex];
    if (!file) return;
    discard(repoId, [
      {
        lines: {
          path: file.path,
          hunk: hunkIndex,
          ranges: selectionRanges(selection),
        },
      },
    ])
      .then(() => {
        selection = null;
        onMutated?.();
      })
      .catch((err: unknown) =>
        toast(`Discard selected failed: ${describeError(err)}`, { kind: "error" }),
      );
  }

  // Escape anywhere (and clicks elsewhere in the diff) clear the selection.
  $effect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") selection = null;
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  });

  function clearSelectionOnClick(): void {
    selection = null;
  }

  // -- M12 AI: explain-hunk (visible once the repo opted in) -------------------

  let explain = $state<{ path: string; hunkText: string; label: string } | null>(
    null,
  );

  const explainEnabled = $derived(
    repoId !== undefined && ai.isOptedIn(repoId),
  );

  function explainHunk(fileIndex: number, hunkIndex: number): void {
    if (!repoId) return;
    const file = files[fileIndex];
    const hunk = file?.hunks[hunkIndex];
    if (!file || !hunk) return;
    explain = {
      path: file.path,
      hunkText: formatHunkText(hunk),
      label: `@@ -${hunk.old_start} +${hunk.new_start} @@`,
    };
  }

  // A new diff resets the scroll position (and any line selection).
  $effect(() => {
    void files;
    selection = null;
    scrollTop = 0;
    viewportEl?.scrollTo(0, 0);
  });

  // Viewport height via ResizeObserver (plus an immediate baseline read).
  $effect(() => {
    const el = viewportEl;
    if (!el) return;
    viewportH = el.clientHeight;
    const ro = new ResizeObserver((entries) => {
      viewportH = entries[0]?.contentRect.height ?? el.clientHeight;
    });
    ro.observe(el);
    return () => ro.disconnect();
  });

  // Scroll → visible window, throttled to animation frames.
  let scrollRaf = 0;
  function onScroll(): void {
    if (scrollRaf) return;
    scrollRaf = requestAnimationFrame(() => {
      scrollRaf = 0;
      scrollTop = viewportEl?.scrollTop ?? 0;
    });
  }
  $effect(() => {
    return () => {
      if (scrollRaf) cancelAnimationFrame(scrollRaf);
    };
  });
</script>

{#if files.length === 0}
  <div class="diff-viewer empty">
    <p class="empty-title">No changes</p>
    <p class="empty-hint">Select a file with differences to see its diff here.</p>
  </div>
{:else}
  <div class="diff-viewer">
    <div class="toolbar">
      <button
        type="button"
        class="mode-btn"
        onclick={cycleMode}
        aria-label={`Switch to ${mode === "split" ? "unified" : "split"} view`}
        title={`Current: ${mode} view — click to switch`}
      >{mode === "split" ? "Split" : "Unified"}</button>
      <span class="stats">
        <span class="stat-files">{files.length} {files.length === 1 ? "file" : "files"}</span>
        <span class="stat-adds">+{model.totalAdditions}</span>
        <span class="stat-dels">−{model.totalDeletions}</span>
      </span>
    </div>

    <!-- tabindex makes the scroller itself keyboard-scrollable (arrow keys);
         region is noninteractive by design — the ignore is deliberate. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="viewport"
      bind:this={viewportEl}
      onscroll={onScroll}
      tabindex="0"
      role="region"
      aria-label="Diff viewer — scrollable diff content"
    >
      <!-- Clicking anywhere that is not a gutter / action button clears the
           line selection (those stop propagation). -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="content" onclick={clearSelectionOnClick} style="height: {layout.totalHeight}px; min-width: {contentMinWidth}">
        {#each slices as slice (slice.bucketIndex)}
          <div class="slice" style="top: {slice.top}px">
            {#each model.rows.slice(slice.firstRow, slice.firstRow + slice.count) as row, i (slice.firstRow + i)}
              <DiffRow
                {row}
                onToggleCollapse={toggleCollapse}
                {onLoadImage}
                onStageHunk={repoId && hunkStaging ? stageHunk : undefined}
                onDiscardHunk={repoId && hunkDiscard ? discardHunk : undefined}
                stageLabel={hunkStaging === "unstage" ? "Unstage hunk" : "Stage hunk"}
                filePath={syntaxEnabled
                  ? (files[model.fileIndexes[slice.firstRow + i]]?.path ?? null)
                  : null}
                selectedAt={selectionEnabled ? selectedAt : undefined}
                onGutterClick={selectionEnabled ? onGutterClick : undefined}
                selectionCount={row.kind === "hunk-header"
                  ? selectionCountFor(row.fileIndex, row.hunkIndex)
                  : 0}
                selectionStageLabel={hunkStaging === "unstage"
                  ? "Unstage selected"
                  : "Stage selected"}
                onStageSelection={repoId && hunkStaging ? stageSelection : undefined}
                onDiscardSelection={repoId && hunkDiscard ? discardSelection : undefined}
                onExplainHunk={explainEnabled ? explainHunk : undefined}
              />
            {/each}
          </div>
        {/each}
      </div>
    </div>

    {#if explain && repoId}
      <ExplainHunkPanel
        {repoId}
        path={explain.path}
        hunkText={explain.hunkText}
        label={explain.label}
        onClose={() => (explain = null)}
      />
    {/if}
  </div>
{/if}

<style>
  .diff-viewer {
    /* Diff palette: static color-mix derivations of the M3 tokens — computed
       once by the CSS engine, never in JS at runtime. */
    --diff-add-bg: color-mix(in srgb, var(--m3-primary) 8%, transparent);
    --diff-del-bg: color-mix(in srgb, var(--m3-error) 8%, transparent);
    --diff-empty-bg: color-mix(in srgb, var(--m3-surface-variant) 40%, transparent);
    --diff-hl-bg: var(--m3-tertiary-container);
    --diff-divider: var(--m3-outline-variant);
    /* M12 line selection highlight. */
    --diff-sel-bg: color-mix(in srgb, var(--m3-primary) 22%, transparent);

    position: relative; /* anchors the docked explain panel */
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--m3-surface-container-lowest, var(--m3-surface));
  }

  .toolbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.25rem 0.75rem;
    border-bottom: 1px solid var(--diff-divider);
    background: var(--m3-surface-container, var(--m3-surface));
  }

  .mode-btn {
    border: 1px solid var(--m3-outline-variant);
    border-radius: 999px;
    background: transparent;
    color: var(--m3-on-surface-variant);
    font-size: 0.75rem;
    padding: 0.25rem 0.875rem;
    cursor: pointer;
  }
  .mode-btn:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }
  .mode-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .stats {
    display: flex;
    gap: 0.75rem;
    font-size: 0.75rem;
    color: var(--m3-on-surface-variant);
  }
  .stat-adds { color: var(--m3-primary); font-variant-numeric: tabular-nums; }
  .stat-dels { color: var(--m3-error); font-variant-numeric: tabular-nums; }

  .viewport {
    flex: 1;
    min-height: 0;
    overflow: auto; /* wheel + native scrollbar; keyboard once focused */
    outline: none;
  }
  .viewport:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .content {
    position: relative;
    width: max-content;
    min-width: 100%;
  }

  .slice {
    position: absolute;
    left: 0;
    right: 0;
    overflow: hidden;
    contain: layout style;
  }

  /* ---- empty state ---- */
  .diff-viewer.empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.25rem;
    color: var(--m3-on-surface-variant);
  }
  .empty-title {
    margin: 0;
    font-size: 0.9375rem;
    font-weight: 500;
    color: var(--m3-on-surface);
  }
  .empty-hint {
    margin: 0;
    font-size: 0.75rem;
  }
</style>
