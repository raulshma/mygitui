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
  import { onMount } from "svelte";
  import { discard, stage } from "$lib/ipc/client";
  import { onUiEvent } from "$lib/palette/events";
  import { toast } from "$lib/toast";
  import { ai } from "$lib/ai/ai.svelte";
  import { formatHunkText } from "$lib/ai/features";
  import {
    onTokensLanded,
    requestTokens,
    setScheme as setSyntaxScheme,
  } from "$lib/diff/highlight";
  import { currentScheme, THEME_SCHEME_EVENT } from "$lib/theme";
  import {
    buildRowModel,
    rowHeights,
    ROW_HEIGHTS,
    type DiffMode,
    type LoadImageFn,
  } from "$lib/components/diff/rowModel";
  import { buildLayout, visibleSlices } from "$lib/components/diff/virtualizer";
  import { measureLineWidth } from "$lib/components/diff/textWidth";
  import {
    GUTTER_PX,
    wrappedHeights,
    wrapMetrics,
  } from "$lib/components/diff/wrapHeights";
  import { readWrapPref, writeWrapPref } from "$lib/diff/wrapPref";
  import { readDiffModePref, writeDiffModePref } from "$lib/diff/modePref";
  import {
    isLineSelected,
    selectedSet as toSelectedSet,
    selectionRanges,
    type LineSelection,
  } from "$lib/components/diff/lineSelection";
  import { selectionSegments, type Segment } from "$lib/components/diff/splitSelection";
  import {
    createSelectionController,
    type SplitSelection,
  } from "$lib/diff/selectionController";
  import { pointFromEvent, rangeForSegment } from "$lib/diff/selectionDom";
  import { createWrapFeedback } from "$lib/diff/wrapFeedback";
  import DiffRow from "$lib/components/diff/DiffRow.svelte";
  import ExplainHunkPanel from "$lib/components/ai/ExplainHunkPanel.svelte";

  let {
    files,
    mode = $bindable(readDiffModePref()),
    wrap = $bindable(readWrapPref()),
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
    /** Wrap long lines (default from the persisted `mygitui.diff.wrap`). */
    wrap?: boolean;
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
  let viewportW = $state(0);

  const collapsedSet = $derived(new Set(Object.keys(collapsed).filter((p) => collapsed[p])));
  const model = $derived(buildRowModel(files, mode, collapsedSet));

  // -- wrap: estimated heights + measured corrections --------------------------

  /** Measured (DOM-true) heights for rendered rows, keyed by row index. */
  let heightOverrides = $state<Record<number, number>>({});

  /** Wrap metrics for the current viewport width; null = wrap off. */
  const wrapCtx = $derived(wrap ? wrapMetrics(viewportW, mode) : null);

  const baseHeights = $derived(
    wrapCtx ? wrappedHeights(model.rows, wrapCtx) : rowHeights(model.rows),
  );

  /** Layout heights: estimates corrected by measured rows where available. */
  const heights = $derived.by(() => {
    const entries = Object.entries(heightOverrides);
    if (entries.length === 0) return baseHeights;
    let patched = baseHeights;
    for (const [index, height] of entries) {
      const i = Number(index);
      if (i < patched.length && patched[i] !== height) {
        if (patched === baseHeights) patched = baseHeights.slice();
        patched[i] = height;
      }
    }
    return patched;
  });

  const layout = $derived(buildLayout(heights));
  const slices = $derived(visibleSlices(layout, scrollTop, viewportH, OVERSCAN_PX));

  /**
   * Rendered rows report their measured text height back; diff/wrapFeedback.ts
   * owns the rAF-batched estimate → measure → patch cycle (divergence-only
   * recording, oscillation guard). The component keeps the reactive
   * corrections record the layout is patched with.
   */
  const wrapFeedback = createWrapFeedback({
    lineHeight: ROW_HEIGHTS.line,
    heightAt: (index) => heights[index],
    overrides: () => heightOverrides,
    applyOverrides: (next) => (heightOverrides = next),
  });

  // New rows (files/mode/wrap/width change) invalidate measured corrections.
  $effect(() => {
    void model;
    void wrapCtx;
    wrapFeedback.invalidate();
  });

  function toggleWrap(): void {
    wrap = !wrap;
    writeWrapPref(wrap);
  }

  // Palette command rides the bus; every mounted viewer flips its wrap.
  $effect(() => onUiEvent("diff-toggle-wrap", toggleWrap));

  /**
   * Horizontal extent of the scroll content.
   *
   * Wrap on: fully responsive — rows wrap within the pane, never a
   * horizontal scrollbar. Wrap off: lines keep their single visual row, so
   * the content grows to the widest line and the viewport scrolls it — one
   * shared horizontal scrollbar in unified mode, and one for BOTH split
   * halves together (halves stay column-aligned; no per-row scrollbars).
   * One-sided files (added/deleted) render inline even in split mode, so a
   * model with only those rows grows by a single gutter + text width.
   * Widths are measured in the rows' own monospace font (textWidth.ts);
   * the gutter constants come from wrapHeights.ts (kept in sync with
   * DiffRow.svelte's CSS).
   */
  const contentMinWidth = $derived.by(() => {
    if (wrap) return "100%";
    const widest = measureLineWidth(model.maxLineText);
    if (mode === "unified") {
      return `max(100%, calc(${GUTTER_PX.unified}px + ${widest}px))`;
    }
    if (!model.hasSplitRows) {
      // Only one-sided files: rows span the pane with a single gutter.
      return `max(100%, calc(${GUTTER_PX.splitSide}px + ${widest}px))`;
    }
    // Both halves as wide as the widest line → the shared scrollbar serves
    // either side.
    return `max(100%, calc(${2 * GUTTER_PX.splitSide + 1}px + ${2 * widest}px))`;
  });

  function toggleCollapse(path: string): void {
    collapsed[path] = !collapsed[path];
  }

  function cycleMode(): void {
    setMode(mode === "split" ? "unified" : "split");
  }

  function setMode(next: DiffMode): void {
    mode = next;
    writeDiffModePref(next);
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

  // Shiki bakes colors at parse time, so the highlighter must follow the
  // applied scheme (github-light / github-dark). The initial async theme
  // apply is caught by the event too; on mount the call only syncs when the
  // default differs. A tick re-requests visible lines under the new theme.
  onMount(() => {
    const sync = (): void => {
      setSyntaxScheme(currentScheme());
      syntaxTick++;
    };
    sync();
    window.addEventListener(THEME_SCHEME_EVENT, sync);
    return () => window.removeEventListener(THEME_SCHEME_EVENT, sync);
  });

  // Request parses for the visible window's lines (cheap: only uncached
  // entries actually hit Shiki). The per-file text lists are derived once per
  // diff, not rebuilt for every token-landed tick.
  const syntaxTexts = $derived.by(() => {
    const out: Array<{ path: string; texts: string[] }> = [];
    for (const file of files) {
      if (file.binary || file.is_image) continue;
      const texts: string[] = [];
      for (const hunk of file.hunks) {
        for (const line of hunk.lines) {
          texts.push(line.text);
        }
      }
      out.push({ path: file.path, texts });
    }
    return out;
  });

  $effect(() => {
    if (!syntaxEnabled) return;
    void syntaxTick; // re-request after background parses land (next window)
    for (const file of syntaxTexts) {
      requestTokens(file.path, file.texts);
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
        selectionCtl.clearLines();
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
        selectionCtl.clearLines();
        onMutated?.();
      })
      .catch((err: unknown) =>
        toast(`Discard selected failed: ${describeError(err)}`, { kind: "error" }),
      );
  }

  // Escape anywhere (and clicks elsewhere in the diff) clear the selection.
  // Ctrl+C with a split-view selection copies the selected side's text —
  // the native copy path can't see it (no native selection exists); the
  // synthesis decision (native wins, nothing selected → no-op) lives in
  // the controller, which reports whether it handled the key.
  $effect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        selectionCtl.clearAll();
        return;
      }
      if (
        (event.ctrlKey || event.metaKey) &&
        (event.key === "c" || event.key === "C") &&
        selectionCtl.copy()
      ) {
        event.preventDefault();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  });

  function clearSelectionOnClick(): void {
    // The click fired right after a drag's mouseup belongs to that drag —
    // it must not erase the selection it just made.
    if (selectionCtl.consumeDragClick()) return;
    selectionCtl.clearAll();
  }

  // -- split view: side-confined text selection -------------------------------
  //
  // Native selection cannot express "the left halves of rows 1–3": it is one
  // contiguous DOM range, and split rows interleave their halves, so a
  // multi-row drag on one side always spans the other side's copies in
  // between (`user-select: contain` doesn't stop drags; `none` content is
  // skipped, not walled; multi-range selection is unsupported). So split
  // mode tracks the drag itself: mousedown inside a half anchors a point
  // (model row + char, resolved with caretRangeFromPoint), mousemove moves
  // the focus — moves onto the OPPOSITE side are ignored, which is the
  // entire confinement — and the segments are painted with the CSS Custom
  // Highlight API. Ctrl+C copies the segments; the drag-ending mousedown is
  // preventDefault()ed so no native selection ever starts. Unified mode
  // keeps native selection (its rows have no halves).
  // The state machine (drag lifecycle, same-side confinement, click-vs-drag
  // disambiguation, word select, Ctrl+C synthesis, gutter shift-extend) lives
  // in diff/selectionController.ts, unit-tested with fake deps; this component
  // injects the real DOM adapters (diff/selectionDom.ts) and mirrors the
  // selections into $state for rendering.
  const SELECTION_HL = "mygitui-diff-sel";
  let segSel = $state<SplitSelection | null>(null);

  /** The half's line text on `side` (null = filler half / not a split row). */
  function halfText(row: number, side: "left" | "right"): string | null {
    const r = model.rows[row];
    if (!r) return null;
    if (r.kind === "context") return r.line.text;
    if (r.kind === "pair") return (side === "left" ? r.left : r.right)?.text ?? null;
    return null;
  }

  const selectionCtl = createSelectionController(
    {
      hitTest: pointFromEvent,
      applySelection: (sel) => (segSel = sel),
      halfText,
      viewport: () => {
        const vp = viewportEl;
        if (!vp) return null;
        return {
          rect: vp.getBoundingClientRect(),
          scrollBy: (dy: number) => {
            vp.scrollTop += dy;
          },
        };
      },
      nativeSelectionText: () => window.getSelection()?.toString() ?? "",
      hunkLines: (fileIndex, hunkIndex) =>
        files[fileIndex]?.hunks[hunkIndex]?.lines ?? null,
    },
    { onLineSelection: (sel) => (selection = sel) },
  );

  const segs = $derived.by(() => {
    if (!segSel) return [] as Segment[];
    const side = segSel.side;
    return selectionSegments(segSel.anchor, segSel.focus, (row) => {
      const t = halfText(row, side);
      return t === null ? null : t.length;
    });
  });

  /**
   * Paints the segments onto the mounted rows. Depends on `scrollTop`
   * because scrolling remounts rows and their ranges must be rebuilt.
   * Depends on `syntaxTick` for the same reason: when background syntax
   * parses land, rows swap their plain text nodes for token spans, and DOM
   * mutations collapse any ranges painted against the old children.
   */
  $effect(() => {
    void scrollTop;
    void syntaxTick;
    const list = segs;
    const side = segSel?.side;
    // jsdom (unit tests) has no Highlight API — guard at runtime.
    if (list.length === 0 || !side || typeof Highlight === "undefined") return;
    const ranges: Range[] = [];
    for (const seg of list) {
      const range = rangeForSegment(viewportEl, seg, side);
      if (range) ranges.push(range);
    }
    if (ranges.length === 0) return;
    CSS.highlights.set(SELECTION_HL, new Highlight(...ranges));
    return () => {
      CSS.highlights.delete(SELECTION_HL);
    };
  });

  let stopDrag: (() => void) | null = null;

  function onContentMouseDown(event: MouseEvent): void {
    if (event.button !== 0 || mode !== "split") return;
    const half =
      event.target instanceof Element ? event.target.closest(".half") : null;
    if (!half) return;
    // No native selection may start in split mode — it can't represent a
    // side. (Clicks still dispatch; dblclick is handled below.)
    event.preventDefault();
    if (!selectionCtl.pointerDown(event)) return;
    const onMove = (ev: MouseEvent): void => selectionCtl.pointerMove(ev);
    const onUp = (): void => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      stopDrag = null;
      selectionCtl.pointerUp();
    };
    stopDrag?.();
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    stopDrag = onUp;
  }

  /** Double-click selects the word under the caret (native one is blocked). */
  function onContentDblClick(event: MouseEvent): void {
    if (event.button !== 0 || mode !== "split") return;
    const half =
      event.target instanceof Element ? event.target.closest(".half") : null;
    if (!half) return;
    event.preventDefault();
    selectionCtl.doubleClick(event);
  }

  // Unmount mid-drag: drop the window listeners and the measure loop.
  $effect(() => {
    return () => {
      stopDrag?.();
      wrapFeedback.dispose();
    };
  });

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

  // A new diff (or a mode/wrap change — row indices and geometry shift)
  // resets the scroll position and any selection.
  $effect(() => {
    void files;
    selectionCtl.clearLines();
    scrollTop = 0;
    viewportEl?.scrollTo(0, 0);
  });

  $effect(() => {
    void mode;
    void wrap;
    selectionCtl.clearSegments();
  });

  // Viewport size via ResizeObserver (plus an immediate baseline read).
  $effect(() => {
    const el = viewportEl;
    if (!el) return;
    viewportH = el.clientHeight;
    viewportW = el.clientWidth;
    const ro = new ResizeObserver((entries) => {
      viewportH = entries[0]?.contentRect.height ?? el.clientHeight;
      viewportW = entries[0]?.contentRect.width ?? el.clientWidth;
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
      <div class="segmented" role="group" aria-label="Diff layout">
        <button
          type="button"
          class="seg"
          class:on={mode === "split"}
          aria-pressed={mode === "split"}
          onclick={() => setMode("split")}
          title="Show old and new side by side"
        >Split</button>
        <button
          type="button"
          class="seg"
          class:on={mode === "unified"}
          aria-pressed={mode === "unified"}
          onclick={() => setMode("unified")}
          title="Show old and new in one column"
        >Unified</button>
      </div>
      <button
        type="button"
        class="tool-btn"
        class:on={wrap}
        aria-pressed={wrap}
        onclick={toggleWrap}
        aria-label="Wrap long lines"
        title={wrap
          ? "Long lines wrap — click to scroll instead"
          : "Long lines scroll — click to wrap"}
      >Wrap</button>
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
      <div class="content" onclick={clearSelectionOnClick} onmousedown={onContentMouseDown} ondblclick={onContentDblClick} style="height: {layout.totalHeight}px; min-width: {contentMinWidth}">
        {#each slices as slice (slice.bucketIndex)}
          <div class="slice" style="top: {slice.top}px">
            {#each model.rows.slice(slice.firstRow, slice.firstRow + slice.count) as row, i (slice.firstRow + i)}
              <DiffRow
                {row}
                {wrap}
                rowIndex={slice.firstRow + i}
                heightPx={wrap ? heights[slice.firstRow + i] : undefined}
                onMeasure={wrap ? wrapFeedback.measure : undefined}
                onToggleCollapse={toggleCollapse}
                {onLoadImage}
                onStageHunk={repoId && hunkStaging ? stageHunk : undefined}
                onDiscardHunk={repoId && hunkDiscard ? discardHunk : undefined}
                stageLabel={hunkStaging === "unstage" ? "Unstage hunk" : "Stage hunk"}
                filePath={syntaxEnabled
                  ? (files[model.fileIndexes[slice.firstRow + i]]?.path ?? null)
                  : null}
                syntaxRevision={syntaxTick}
                selectedAt={selectionEnabled ? selectedAt : undefined}
                onGutterClick={selectionEnabled ? selectionCtl.gutterClick : undefined}
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
    /* Diff palette: status hues from the app tokens (green add / red delete),
       mixed over the surrounding surface by the CSS engine — never in JS. */
    --diff-add-bg: color-mix(in srgb, var(--app-added, var(--m3-tertiary)) 12%, transparent);
    --diff-del-bg: color-mix(in srgb, var(--app-deleted, var(--m3-error)) 12%, transparent);
    --diff-add-gutter: color-mix(in srgb, var(--app-added, var(--m3-tertiary)) 20%, transparent);
    --diff-del-gutter: color-mix(in srgb, var(--app-deleted, var(--m3-error)) 20%, transparent);
    --diff-empty-bg: color-mix(in srgb, var(--m3-surface-variant) 12%, transparent);
    --diff-hl-bg: color-mix(in srgb, var(--app-modified, var(--m3-tertiary)) 32%, transparent);
    --diff-divider: var(--m3-outline-variant);
    --diff-file-bg: var(--m3-surface-container, var(--m3-surface));
    --diff-file-accent: color-mix(in srgb, var(--m3-primary) 55%, transparent);
    --diff-hunk-bg: color-mix(in srgb, var(--m3-primary) 6%, var(--m3-surface-container, var(--m3-surface)));
    --diff-hunk-accent: color-mix(in srgb, var(--m3-primary) 65%, transparent);
    /* M12 line selection highlight. */
    --diff-sel-bg: color-mix(in srgb, var(--m3-primary) 24%, transparent);

    position: relative; /* anchors the docked explain panel */
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--m3-surface-container-lowest, var(--m3-surface));
  }

  /*
   * Dark surfaces swallow the light-mode washes (12% over near-black is
   * barely a tint). Strengthen row/gutter/highlight tints so changed blocks
   * stay recognizable; the status hues themselves switch to their bright
   * dark variants via app.css `data-theme`.
   */
  :global(:root[data-theme="dark"]) .diff-viewer {
    --diff-add-bg: color-mix(in srgb, var(--app-added) 20%, transparent);
    --diff-del-bg: color-mix(in srgb, var(--app-deleted) 20%, transparent);
    --diff-add-gutter: color-mix(in srgb, var(--app-added) 34%, transparent);
    --diff-del-gutter: color-mix(in srgb, var(--app-deleted) 34%, transparent);
    --diff-empty-bg: color-mix(in srgb, var(--m3-surface-variant) 20%, transparent);
    --diff-hl-bg: color-mix(in srgb, var(--app-modified) 42%, transparent);
    --diff-sel-bg: color-mix(in srgb, var(--m3-primary) 30%, transparent);
  }

  .toolbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.375rem;
    min-height: 1.8rem;
    padding: 0.2rem 0.6rem;
    border-bottom: 1px solid var(--diff-divider);
    background: var(--m3-surface-container, var(--m3-surface));
  }

  /* Compact segmented layout switch. */
  .segmented {
    display: inline-flex;
    flex: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    overflow: hidden;
  }

  .seg {
    border: none;
    background: transparent;
    color: var(--m3-on-surface-variant);
    font: inherit;
    font-size: 0.6875rem;
    line-height: 1.5;
    padding: 0 0.5rem;
    cursor: pointer;
  }

  .seg + .seg {
    border-left: 1px solid var(--m3-outline-variant);
  }

  .seg:hover {
    background: var(--app-hover, var(--m3-surface-container-high, var(--m3-surface)));
    color: var(--m3-on-surface);
  }

  .seg.on {
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
    color: var(--m3-on-secondary-container, var(--m3-on-surface));
    font-weight: 600;
  }

  .tool-btn {
    flex: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    background: transparent;
    color: var(--m3-on-surface-variant);
    font: inherit;
    font-size: 0.6875rem;
    line-height: 1.5;
    padding: 0 0.5rem;
    cursor: pointer;
  }

  .tool-btn:hover {
    background: var(--app-hover, var(--m3-surface-container-high, var(--m3-surface)));
    color: var(--m3-on-surface);
  }

  .tool-btn.on {
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
    color: var(--m3-on-secondary-container, var(--m3-on-surface));
  }

  .seg:focus-visible,
  .tool-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .stats {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-left: auto;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant);
    font-variant-numeric: tabular-nums;
  }
  .stat-adds {
    color: var(--app-added, var(--m3-primary));
  }
  .stat-dels {
    color: var(--app-deleted, var(--m3-error));
  }

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

  /* Split-view selection paint (see the split view block in the script):
     the segments are ranges registered in CSS.highlights. The Custom
     Highlight API paints only color/background/text-decoration, which is
     all a text selection needs. */
  :global(::highlight(mygitui-diff-sel)) {
    background-color: color-mix(in srgb, var(--m3-primary) 35%, transparent);
    color: var(--m3-on-surface);
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
