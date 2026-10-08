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
  import {
    buildRowModel,
    rowHeights,
    type DiffMode,
    type LoadImageFn,
  } from "$lib/components/diff/rowModel";
  import { buildLayout, visibleSlices } from "$lib/components/diff/virtualizer";
  import DiffRow from "$lib/components/diff/DiffRow.svelte";

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

  /** Horizontal scroll width: gutters/signs in rem + longest line in ch. */
  const minWidth = $derived(
    mode === "split"
      ? `calc(9.5rem + 2 * ${model.maxTextChars}ch + 2rem)`
      : `calc(8rem + ${model.maxTextChars}ch + 1rem)`,
  );

  function toggleCollapse(path: string): void {
    collapsed[path] = !collapsed[path];
  }

  function cycleMode(): void {
    mode = mode === "split" ? "unified" : "split";
  }

  // Palette command rides the bus; every mounted viewer flips its mode.
  $effect(() => onUiEvent("diff-toggle-mode", cycleMode));

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

  // A new diff resets the scroll position.
  $effect(() => {
    void files;
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
      <div class="content" style="height: {layout.totalHeight}px; min-width: {minWidth}">
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
              />
            {/each}
          </div>
        {/each}
      </div>
    </div>
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
