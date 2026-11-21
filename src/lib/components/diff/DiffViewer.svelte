<script lang="ts">
  /**
   * DiffViewer — the app's primary diff surface (wave-2 lane B1).
   *
   * Contract: `{ files: FileDiff[]; mode?: "split" | "unified" }` (mode is
   * additionally bindable; the shared DiffToolbar flips it). Additive
   * optional props: `onLoadImage` supplies real URLs for image diffs,
   * `containerNarrow` feeds the narrow-container defaults (§7.5: under
   * 700px the viewer auto-selects Unified and turns wrap off; a user
   * override sticks for the session, in memory only — the persisted prefs
   * keep ruling wide containers), and `onOpenInEditor` adds the toolbar
   * overflow's "Open in editor" item. Surfaces that track their own panel
   * width (commit detail) pass `containerNarrow`; standalone diff panes
   * leave it unset and are measured here.
   *
   * Rendering is fully virtualized: buildRowModel flattens the files into
   * uniform-height row buckets (diff/rowModel.ts), buildLayout indexes them
   * (diff/virtualizer.ts), and only the visible window (± overscan) is ever
   * mounted. 10k-hunk diffs scroll at 60fps — the math is unit tested and
   * perf-gated in diff/virtualizer.test.ts.
   */
  import { describeError } from "$lib/errors";
  import type { FileDiff } from "$lib/ipc/types";
  import { onMount, untrack } from "svelte";
  import { discard, stage } from "$lib/ipc/client";
  import { onUiEvent } from "$lib/palette/events";
  import { appSettings } from "$lib/state/appSettings.svelte";
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
    filePatchText,
    findMatches,
    hunkPatchText,
    rowHeights,
    rowLineText,
    ROW_HEIGHTS,
    type DiffMode,
    type FindSpan,
    type LoadImageFn,
  } from "$lib/components/diff/rowModel";
  import {
    buildLayout,
    firstVisibleRow,
    rowTop,
    visibleSlices,
  } from "$lib/components/diff/virtualizer";
  import { measureLineWidth } from "$lib/components/diff/textWidth";
  import { wrappedHeights, wrapMetrics } from "$lib/components/diff/wrapHeights";
  import { GUTTER_PX } from "$lib/components/diff/metrics";
  import { readWrapPref, writeWrapPref } from "$lib/diff/wrapPref";
  import { readDiffModePref, writeDiffModePref } from "$lib/diff/modePref";
  import { NARROW_PANEL_PX } from "$lib/layout/containerWidth.svelte";
  import DiffToolbar from "./DiffToolbar.svelte";
  import { diffPrefs } from "$lib/diff/diffPrefs.svelte";
  import { copyText } from "$lib/clipboard";
  import {
    showMenuAt,
    type MenuEntry,
  } from "$lib/components/menu/contextMenuStore.svelte";
  import {
    isLineSelected,
    selectedLineTexts,
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
  import EmptyState from "../layout/EmptyState.svelte";
  import { Hint } from "$lib/components/ui/tooltip/index.js";

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
    containerNarrow = null,
    onOpenInEditor = undefined,
    initialScrollTop = undefined,
    initialRow = undefined,
    onScrollChange = undefined,
    onVisibleRowChange = undefined,
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
    /**
     * Narrow-container control (plan §7.5): `true`/`false` when the OWNER
     * tracks the surface's panel width (commit detail passes its panel
     * measurement, so the files column never tips the diff into the
     * narrow defaults on its own); `null` (default) makes the viewer
     * measure its own root width — the standalone diff panes.
     */
    containerNarrow?: boolean | null;
    /** Toolbar overflow "Open in editor" (surfaces that can edit). */
    onOpenInEditor?: () => void;
    /**
     * Workspace-session restore: scroll position the FIRST landed files
     * batch applies (overriding the reset-to-0 below, once). Read at
     * mount; normal re-streams keep resetting to the top.
     */
    initialScrollTop?: number;
    /**
     * Workspace-session restore, row form: first-visible-row the first
     * landed batch scrolls to (converted through the layout — a row index
     * survives wrap/mode height changes that drift a px offset). Preferred
     * over `initialScrollTop` when both are present.
     */
    initialRow?: number;
    /**
     * Scroll capture (rAF-coalesced with the visible-window update): the
     * px offset plus the first visible row (the scroll-stable restore
     * position; callers may ignore either argument).
     */
    onScrollChange?: (scrollTop: number, firstVisibleRow: number) => void;
    /**
     * First-visible-row capture (workspace-session per-diff restore): a
     * row index survives wrap/mode height changes that would drift a px
     * offset. Reported alongside the scroll capture; only scroll events
     * report (a never-scrolled diff restores to the top, as before).
     */
    onVisibleRowChange?: (rowIndex: number) => void;
  } = $props();

  /** Rows rendered beyond the viewport (px, top and bottom). */
  const OVERSCAN_PX = 400;

  /** Collapsed file sections, keyed by path. */
  let collapsed: Record<string, boolean> = $state({});

  let viewportEl: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewportH = $state(0);
  let viewportW = $state(0);

  // svelte-ignore state_referenced_locally -- the restore seed is consumed
  // once, at the first landed files batch after mount; later prop updates
  // (a re-streamed diff) must keep the normal reset-to-top behavior.
  /** Saved position waiting to be applied when the diff rows land. */
  let pendingRestore: number | null =
    typeof initialScrollTop === "number" && initialScrollTop > 0
      ? initialScrollTop
      : null;
  // svelte-ignore state_referenced_locally -- same mount-time seed rule as
  // the px seed above; the row seed wins when both are present.
  /** Saved first-visible-row waiting to be applied when the rows land. */
  let pendingRestoreRow: number | null =
    typeof initialRow === "number" && initialRow > 0 ? initialRow : null;

  const collapsedSet = $derived(new Set(Object.keys(collapsed).filter((p) => collapsed[p])));

  // -- narrow-container defaults (plan §7.5) ------------------------------------
  //
  // A panel under `NARROW_PANEL_PX` (700px) auto-selects Unified and turns
  // wrap OFF (horizontal scroll instead of wrapped-line walls) — but a user
  // override sticks for the session, in memory only: the persisted prefs
  // keep governing wide containers exactly as before, and the narrow
  // override is deliberately not written to localStorage (the next session
  // at a narrow width starts from the auto default again).
  let modeOverride: DiffMode | null = $state(null);
  let wrapOverride: boolean | null = $state(null);

  const narrow = $derived(
    containerNarrow !== null
      ? containerNarrow
      : viewportW > 0 && viewportW < NARROW_PANEL_PX,
  );
  /** The mode actually rendered: session override > narrow auto > pref. */
  const effMode = $derived(modeOverride ?? (narrow ? "unified" : mode));
  /** The wrap actually rendered: session override > narrow auto > pref. */
  const effWrap = $derived(wrapOverride ?? (narrow ? false : wrap));

  // Whitespace toggles (diffPrefs) feed the model: "ignore whitespace"
  // drops the zero-hunk file entries the backend keeps for ws-only deltas,
  // "hide whitespace changes" filters the paired ws-only rows (rowModel.ts
  // owns the heuristic) so counts/virtualization stay consistent.
  const model = $derived(
    buildRowModel(files, effMode, collapsedSet, {
      ignoreWhitespaceFiles: diffPrefs.ignoreWs,
      hideWhitespaceRows: diffPrefs.hideWs,
    }),
  );

  // -- wrap: estimated heights + measured corrections --------------------------

  /** Measured (DOM-true) heights for rendered rows, keyed by row index. */
  let heightOverrides = $state<Record<number, number>>({});

  /** Wrap metrics for the current viewport width; null = wrap off. */
  const wrapCtx = $derived(effWrap ? wrapMetrics(viewportW, effMode) : null);

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

  /** Sets wrap, remembering the session override (persisted when wide). */
  function setWrap(next: boolean): void {
    wrap = next;
    wrapOverride = next;
    if (!narrow) writeWrapPref(next);
  }

  function toggleWrap(): void {
    setWrap(!effWrap);
  }

  // Palette command rides the bus; every mounted viewer flips its wrap.
  $effect(() => onUiEvent("diff-toggle-wrap", toggleWrap));

  // -- whitespace + word-highlight toggles (diffPrefs, see diffPrefs.svelte) --
  // The toggles moved into the shared DiffToolbar (which owns the setters
  // and the re-query event); DiffViewer only reads the store here.

  // -- find-in-diff (Ctrl/Cmd+F via the palette `diff.find` event) -----------
  //
  // Case-insensitive substring matching over the flattened row model (the
  // documented heuristic, see rowModel.findMatches). Matches highlight in
  // the rendered text; Enter/Shift+Enter step next/prev and the viewport
  // scrolls to the active match's row.

  let findOpen = $state(false);
  let findQuery = $state("");
  let findIdx = $state(0);
  let findInputEl = $state<HTMLInputElement | null>(null);

  const matches = $derived(findMatches(model.rows, findQuery));
  /** `findIdx` clamped into the live match list (rows may have shrunk). */
  const activeFindIdx = $derived(
    matches.length === 0 ? -1 : Math.min(findIdx, matches.length - 1),
  );

  /** Per-row highlight spans for DiffRow (active match marked). */
  const findSpansByRow = $derived.by(() => {
    if (matches.length === 0) return null;
    const map = new Map<number, FindSpan[]>();
    for (let i = 0; i < matches.length; i++) {
      const match = matches[i];
      const spans = map.get(match.rowIndex) ?? [];
      spans.push({
        side: match.side,
        start: match.start,
        end: match.end,
        active: i === activeFindIdx,
      });
      map.set(match.rowIndex, spans);
    }
    return map;
  });

  /** File/hunk header-row indexes for the jump navigation. */
  const headerRows = $derived.by(() => {
    const fileRows: number[] = [];
    const hunkRows: number[] = [];
    for (let i = 0; i < model.rows.length; i++) {
      const kind = model.rows[i].kind;
      if (kind === "file-header") fileRows.push(i);
      else if (kind === "hunk-header") hunkRows.push(i);
    }
    return { fileRows, hunkRows };
  });

  function openFind(): void {
    findOpen = true;
    requestAnimationFrame(() => {
      findInputEl?.focus();
      findInputEl?.select();
    });
  }

  function closeFind(): void {
    findOpen = false;
  }

  /** Find-as-you-type: reset to the first match and follow it. */
  $effect(() => {
    const query = findQuery;
    const open = findOpen;
    if (!open) return;
    findIdx = 0;
    untrack(() => {
      const first = matches[0];
      if (first !== undefined) scrollToRow(first.rowIndex);
    });
    void query;
  });

  function findStep(delta: number): void {
    if (matches.length === 0) return;
    findIdx = (activeFindIdx + delta + matches.length) % matches.length;
    const match = matches[findIdx];
    if (match !== undefined) scrollToRow(match.rowIndex);
  }

  function findKeydown(event: KeyboardEvent): void {
    if (event.key === "Enter") {
      event.preventDefault();
      findStep(event.shiftKey ? -1 : 1);
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeFind();
    }
  }

  // -- diff navigation (palette `diff.*` events, implemented against the
  //    row model: next/prev walk the file/hunk header rows, wrapping) -------

  /** Scrolls the viewport so `rowIndex` is its top row. */
  function scrollToRow(rowIndex: number): void {
    const top = rowTop(layout, rowIndex);
    scrollTop = top;
    viewportEl?.scrollTo(0, top);
  }

  /** Jumps to the next/previous header row (wraps at both ends). */
  function jumpHeaders(indices: readonly number[], forward: boolean): void {
    if (indices.length === 0) return;
    const current = firstVisibleRow(layout, scrollTop);
    let target: number;
    if (forward) {
      target = indices.find((i) => i > current) ?? indices[0];
    } else {
      let previous = -1;
      for (const i of indices) {
        if (i >= current) break;
        previous = i;
      }
      target = previous >= 0 ? previous : indices[indices.length - 1];
    }
    scrollToRow(target);
  }

  function jumpFirstChange(): void {
    const first = headerRows.hunkRows[0] ?? headerRows.fileRows[0];
    if (first !== undefined) scrollToRow(first);
  }

  // Palette commands / keybinds ride the bus; every mounted viewer jumps.
  $effect(() => {
    const unlisteners = [
      onUiEvent("diff-find", openFind),
      onUiEvent("diff-next-hunk", () => jumpHeaders(headerRows.hunkRows, true)),
      onUiEvent("diff-prev-hunk", () => jumpHeaders(headerRows.hunkRows, false)),
      onUiEvent("diff-next-file", () => jumpHeaders(headerRows.fileRows, true)),
      onUiEvent("diff-prev-file", () => jumpHeaders(headerRows.fileRows, false)),
      onUiEvent("diff-first-change", jumpFirstChange),
    ];
    return () => {
      for (const unlisten of unlisteners) unlisten();
    };
  });

  // -- copy actions (row / file-header context menu; clipboard via the
  //    shared copyText helper, like every other surface) ----------------------

  /** Toolbar overflow "Copy diff": the whole diff as one patch text. */
  function copyWholeDiff(): void {
    const parts: string[] = [];
    for (const file of files) {
      if (file.binary || file.is_image) continue;
      parts.push(filePatchText(file));
    }
    if (parts.length > 0) void copyText(parts.join("\n"), "diff");
  }

  /**
   * Scrolls the diff to `path`'s file-header row (the file lists call this
   * to navigate a long multi-file diff). Returns false when the row isn't
   * in the model (unknown path, or the diff is empty/binary-only).
   */
  export function revealFile(path: string): boolean {
    const index = model.rows.findIndex(
      (row) => row.kind === "file-header" && row.file.path === path,
    );
    if (index < 0) return false;
    scrollToRow(index);
    return true;
  }

  /**
   * Row context menu: copy the line under the pointer (a split row copies
   * the half it hit), the hunk patch, the whole file patch, or the path.
   */
  function rowMenu(event: MouseEvent, rowIndex: number): void {
    const row = model.rows[rowIndex];
    if (row === undefined) return;
    event.preventDefault();
    const half =
      event.target instanceof Element ? event.target.closest(".half") : null;
    const side: "left" | "right" = half?.classList.contains("right")
      ? "right"
      : "left";
    const entries: MenuEntry[] = [];
    const lineText = rowLineText(row, row.kind === "pair" ? side : null);
    if (lineText !== null) {
      entries.push({
        id: "copy-line",
        label: "Copy line",
        run: () => void copyText(lineText, "line"),
      });
    }
    if (row.kind !== "file-header" && row.kind !== "binary" && row.kind !== "image") {
      const file = files[row.fileIndex];
      const hunkText = hunkPatchText(file, row.hunkIndex);
      if (hunkText !== null) {
        entries.push({
          id: "copy-hunk",
          label: "Copy hunk patch",
          run: () => void copyText(hunkText, "hunk patch"),
        });
      }
      // Item 57: the selection-scoped explain rides the same menu as the
      // copy actions — offered only while the row's own file+hunk has the
      // ONE active selection (no selection → the item is hidden), and
      // behind the same AI gates as the hunk explain button.
      if (
        explainSelectionEnabled &&
        selection !== null &&
        selection.fileIndex === row.fileIndex &&
        selection.hunkIndex === row.hunkIndex
      ) {
        entries.push({
          id: "ai-explain-selection",
          label: "Explain selected lines",
          run: () => explainSelection(),
        });
      }
    }
    // The fileIndexes parallel array covers every row kind (binary/image
    // rows carry no fileIndex of their own).
    const file =
      row.kind === "file-header"
        ? row.file
        : files[model.fileIndexes[rowIndex] ?? 0];
    entries.push(
      {
        id: "copy-patch",
        label: "Copy file patch",
        run: () => void copyText(filePatchText(file), "patch"),
      },
      {
        id: "copy-path",
        label: "Copy path",
        run: () => void copyText(file.path, "path"),
      },
    );
    showMenuAt(event, entries);
  }

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
   * the gutter constants come from diff/metrics.ts — the same constants
   * DiffRow renders its gutters with, so the scroll width can never drift
   * from the rows.
   */
  const contentMinWidth = $derived.by(() => {
    if (effWrap) return "100%";
    const widest = measureLineWidth(model.maxLineText);
    if (effMode === "unified") {
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
    setMode(effMode === "split" ? "unified" : "split");
  }

  /** Sets the mode, remembering the session override (persisted when wide). */
  function setMode(next: DiffMode): void {
    mode = next;
    modeOverride = next;
    if (!narrow) writeDiffModePref(next);
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
  // entries actually hit Shiki). The visible window moves with scroll, so
  // requests follow it; requesting the WHOLE diff here would re-walk every
  // line of every file on each token-landed tick (plus a cache-key string
  // per line) — O(diff) work per ~50ms tick while a large diff parses.
  $effect(() => {
    if (!syntaxEnabled) return;
    void syntaxTick; // re-request after background parses land (next window)
    const textsByFile = new Map<number, string[]>();
    for (const slice of slices) {
      for (let i = slice.firstRow; i < slice.firstRow + slice.count; i++) {
        const row = model.rows[i];
        if (row === undefined) continue;
        // Only text-bearing rows carry a fileIndex (headers/binary/image
        // have none and nothing to highlight).
        let text: string | null;
        switch (row.kind) {
          case "line":
          case "single":
          case "context":
            text = row.line.text;
            break;
          case "pair":
            if (row.left !== null) requestTextFor(row, textsByFile, row.left.text);
            if (row.right !== null) requestTextFor(row, textsByFile, row.right.text);
            continue;
          default:
            continue; // headers / binary / image carry no highlightable text
        }
        requestTextFor(row, textsByFile, text);
      }
    }
    for (const [fileIndex, texts] of textsByFile) {
      const file = files[fileIndex];
      if (!file || file.binary || file.is_image) continue;
      requestTokens(file.path, texts);
    }
  });

  /** Collect one line's text under its row's file, creating the bucket. */
  function requestTextFor(
    row: { fileIndex: number },
    textsByFile: Map<number, string[]>,
    text: string,
  ): void {
    let texts = textsByFile.get(row.fileIndex);
    if (texts === undefined) {
      texts = [];
      textsByFile.set(row.fileIndex, texts);
    }
    texts.push(text);
  }

  // -- M9 F6: hunk staging / discard ------------------------------------------

  /**
   * One hunk mutation at a time. Targets are positional (`path` + hunk
   * index), so a double-click fired before the refreshed diff lands would
   * act on indices that no longer describe what the user clicked — a
   * discard could silently drop the wrong hunk. Actions ignore calls while
   * busy; the flag clears when the parent's refreshed `files` arrive, on
   * failure, or after a safety timeout (in case no refresh follows).
   */
  const MUTATION_GUARD_MS = 4000;
  let mutating = $state(false);
  let mutationTimer: ReturnType<typeof setTimeout> | null = null;

  function beginMutation(): boolean {
    if (mutating) return false;
    mutating = true;
    mutationTimer = setTimeout(() => {
      mutationTimer = null;
      mutating = false;
    }, MUTATION_GUARD_MS);
    return true;
  }

  function endMutation(): void {
    if (mutationTimer !== null) {
      clearTimeout(mutationTimer);
      mutationTimer = null;
    }
    mutating = false;
  }

  // The refreshed diff is the natural "done" signal: old indices are gone,
  // so the next action is safe again.
  let lastFiles: FileDiff[] | undefined;
  $effect(() => {
    const current = files;
    if (lastFiles === undefined) {
      lastFiles = current;
      return;
    }
    if (current !== lastFiles) {
      lastFiles = current;
      endMutation();
    }
  });

  function stageHunk(fileIndex: number, hunkIndex: number): void {
    if (!repoId || !hunkStaging) return;
    const file = files[fileIndex];
    if (!file || !beginMutation()) return;
    const unstage = hunkStaging === "unstage";
    stage(repoId, {
      targets: [{ hunk: { path: file.path, hunk: hunkIndex } }],
      unstage,
    })
      .then(() => onMutated?.())
      .catch((err: unknown) => {
        endMutation();
        toast(`${unstage ? "Unstage" : "Stage"} failed: ${describeError(err)}`, {
          kind: "error",
        });
      });
  }

  function discardHunk(fileIndex: number, hunkIndex: number): void {
    if (!repoId) return;
    const file = files[fileIndex];
    if (!file || !beginMutation()) return;
    discard(repoId, [{ hunk: { path: file.path, hunk: hunkIndex } }], {
      checkpoint: appSettings.checkpointsEnabled,
    })
      .then(() => onMutated?.())
      .catch((err: unknown) => {
        endMutation();
        toast(`Discard failed: ${describeError(err)}`, { kind: "error" });
      });
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
    if (!file || !beginMutation()) return;
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
      .catch((err: unknown) => {
        endMutation();
        toast(
          `${unstage ? "Unstage" : "Stage"} selected failed: ${describeError(err)}`,
          { kind: "error" },
        );
      });
  }

  function discardSelection(fileIndex: number, hunkIndex: number): void {
    if (!repoId || !hunkDiscard || !selection) return;
    const file = files[fileIndex];
    if (!file || !beginMutation()) return;
    discard(
      repoId,
      [
        {
          lines: {
            path: file.path,
            hunk: hunkIndex,
            ranges: selectionRanges(selection),
          },
        },
      ],
      { checkpoint: appSettings.checkpointsEnabled },
    )
      .then(() => {
        selectionCtl.clearLines();
        onMutated?.();
      })
      .catch((err: unknown) => {
        endMutation();
        toast(`Discard selected failed: ${describeError(err)}`, { kind: "error" });
      });
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
    if (event.button !== 0 || effMode !== "split") return;
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
    if (event.button !== 0 || effMode !== "split") return;
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

  // -- M12 AI: explain-hunk + item 57's selection-scoped explain (visible
  // once AI is on, the feature is on, and the repo opted in — the same
  // master + per-feature gates every other AI affordance follows, M13) ----

  let explain = $state<{
    path: string;
    hunkText: string;
    label: string;
    selection: { lineStart: number; lineEnd: number; selectedText: string } | null;
  } | null>(null);

  const explainEnabled = $derived(
    repoId !== undefined &&
      ai.isEnabled() &&
      ai.isFeatureEnabled("explain-hunk") &&
      ai.isOptedIn(repoId),
  );

  /** Context-menu "Explain selected lines" needs its feature gate too. */
  const explainSelectionEnabled = $derived(
    repoId !== undefined &&
      ai.isEnabled() &&
      ai.isFeatureEnabled("explain-selection") &&
      ai.isOptedIn(repoId),
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
      selection: null,
    };
  }

  /**
   * Opens the selection-scoped explain (item 57): the highlighted rows'
   * text (isLineSelected semantics, origin-marked), their new-side line
   * range, and the whole hunk as surrounding context — the prompt builder
   * caps the context via capDiff. Runs `explain-selection` through the
   * same panel + job pipeline as the hunk explain.
   */
  function explainSelection(): void {
    if (!repoId || !selection) return;
    const file = files[selection.fileIndex];
    const hunk = file?.hunks[selection.hunkIndex];
    if (!file || !hunk) return;
    const lines = selectedLineTexts(hunk.lines, toSelectedSet(selection));
    if (lines.length === 0) return;
    const lineStart = selection.lines[0];
    const lineEnd = selection.lines.at(-1) ?? lineStart;
    explain = {
      path: file.path,
      hunkText: formatHunkText(hunk),
      label: `lines ${lineStart}–${lineEnd}`,
      selection: {
        lineStart,
        lineEnd,
        selectedText: lines.join("\n"),
      },
    };
  }

  // A new diff (or a mode/wrap change — row indices and geometry shift)
  // resets the scroll position and any selection — unless a workspace
  // session restore is waiting: it consumes the reset once, when the rows
  // have landed (re-asserted on the next frame, after the virtualizer's
  // spacer has its final height). The row seed wins over the px seed and
  // converts through the layout at apply time (untracked: the layout's
  // later measure corrections must not re-run this effect — it would fall
  // into the reset branch and throw the restore away).
  $effect(() => {
    void files;
    selectionCtl.clearLines();
    const restoreRow = pendingRestoreRow;
    const restore = pendingRestore;
    if (files.length > 0 && (restoreRow !== null || restore !== null)) {
      pendingRestoreRow = null;
      pendingRestore = null;
      const top = untrack(() =>
        restoreRow !== null ? rowTop(layout, restoreRow) : restore,
      );
      if (typeof top === "number" && top > 0) {
        scrollTop = top;
        viewportEl?.scrollTo(0, top);
        requestAnimationFrame(() => viewportEl?.scrollTo(0, top));
      }
    } else {
      scrollTop = 0;
      viewportEl?.scrollTo(0, 0);
    }
  });

  $effect(() => {
    void effMode;
    void effWrap;
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
      const row = firstVisibleRow(layout, scrollTop);
      onScrollChange?.(scrollTop, row);
      onVisibleRowChange?.(row);
    });
  }
  $effect(() => {
    return () => {
      if (scrollRaf) cancelAnimationFrame(scrollRaf);
    };
  });
</script>

{#if files.length === 0 || model.rows.length === 0}
  <!-- rows.length === 0: every file entry was ws-only and the
       ignore-whitespace toggle dropped them (the normal empty state). -->
  <div class="diff-viewer empty relative flex h-full min-h-0 flex-col bg-background">
    <EmptyState title="No changes" hint="Select a file with differences to see its diff here." />
  </div>
{:else}
  <div class="diff-viewer relative flex h-full min-h-0 flex-col bg-background">
    <!-- Plan §7.4: ONE toolbar for every diff surface — segmented
         Split|Unified + icon toggles (Wrap, Ignore whitespace) + overflow
         (copy diff, open in editor, hide-ws / word-highlight). -->
    <DiffToolbar
      mode={effMode}
      wrap={effWrap}
      fileCount={model.fileCount}
      additions={model.totalAdditions}
      deletions={model.totalDeletions}
      onModeChange={setMode}
      onWrapChange={setWrap}
      onCopyDiff={copyWholeDiff}
      {onOpenInEditor}
    />

    {#if findOpen}
      <!-- Find bar (palette `diff.find` / Ctrl/Cmd+F): case-insensitive
           substring over the rendered rows; Enter / Shift+Enter step. -->
      <div class="findbar flex min-h-7 shrink-0 items-center gap-1.5 border-b border-[var(--diff-divider)] bg-card px-2.5 py-0.5" role="search" aria-label="Find in diff">
        <input
          bind:this={findInputEl}
          bind:value={findQuery}
          type="text"
          placeholder="Find (case-insensitive)"
          aria-label="Find in diff"
          class="h-6 min-w-0 flex-1 rounded-sm border border-border bg-background px-2 font-mono text-ui-meta leading-[1.5] outline-none placeholder:text-muted-foreground/70 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
          onkeydown={findKeydown}
        />
        <span class="find-count shrink-0 whitespace-nowrap text-ui-micro leading-[1.5] tabular-nums text-muted-foreground" role="status">
          {matches.length === 0
            ? "No matches"
            : `${activeFindIdx + 1} of ${matches.length}`}
        </span>
        <Hint label="Previous match (Shift+Enter)">
          {#snippet child({ props })}
            <button
              {...props}
              type="button"
              class="find-prev h-6 shrink-0 cursor-pointer rounded-sm border border-border bg-background px-2 text-ui-meta leading-[1.5] text-foreground disabled:cursor-not-allowed disabled:opacity-50"
              aria-label="Previous match"
              disabled={matches.length === 0}
              onclick={() => findStep(-1)}
            >↑</button>
          {/snippet}
        </Hint>
        <Hint label="Next match (Enter)">
          {#snippet child({ props })}
            <button
              {...props}
              type="button"
              class="find-next h-6 shrink-0 cursor-pointer rounded-sm border border-border bg-background px-2 text-ui-meta leading-[1.5] text-foreground disabled:cursor-not-allowed disabled:opacity-50"
              aria-label="Next match"
              disabled={matches.length === 0}
              onclick={() => findStep(1)}
            >↓</button>
          {/snippet}
        </Hint>
        <Hint label="Close find (Esc)">
          {#snippet child({ props })}
            <button
              {...props}
              type="button"
              class="find-close h-6 shrink-0 cursor-pointer rounded-sm border border-border bg-background px-2 text-ui-meta leading-[1.5] text-muted-foreground hover:text-foreground"
              aria-label="Close find"
              onclick={closeFind}
            >✕</button>
          {/snippet}
        </Hint>
      </div>
    {/if}

    <!-- tabindex makes the scroller itself keyboard-scrollable (arrow keys);
         region is noninteractive by design — the ignore is deliberate. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="viewport min-h-0 flex-1 overflow-auto outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
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
      <div class="content relative w-max min-w-full" onclick={clearSelectionOnClick} onmousedown={onContentMouseDown} ondblclick={onContentDblClick} style="height: {layout.totalHeight}px; min-width: {contentMinWidth}">
        {#each slices as slice (slice.bucketIndex)}
          <div class="slice absolute inset-x-0 overflow-hidden [contain:layout_style]" style="top: {slice.top}px">
            {#each model.rows.slice(slice.firstRow, slice.firstRow + slice.count) as row, i (slice.firstRow + i)}
              <DiffRow
                {row}
                wrap={effWrap}
                rowIndex={slice.firstRow + i}
                heightPx={effWrap ? heights[slice.firstRow + i] : undefined}
                onMeasure={effWrap ? wrapFeedback.measure : undefined}
                onToggleCollapse={toggleCollapse}
                {onLoadImage}
                onStageHunk={repoId && hunkStaging ? stageHunk : undefined}
                onDiscardHunk={repoId && hunkDiscard ? discardHunk : undefined}
                actionBusy={mutating}
                stageLabel={hunkStaging === "unstage" ? "Unstage hunk" : "Stage hunk"}
                filePath={syntaxEnabled
                  ? (files[model.fileIndexes[slice.firstRow + i]]?.path ?? null)
                  : null}
                syntaxRevision={syntaxTick}
                wordHighlights={diffPrefs.wordHighlights}
                findSpans={findSpansByRow?.get(slice.firstRow + i)}
                onRowMenu={(event) => rowMenu(event, slice.firstRow + i)}
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
        selection={explain.selection}
        onClose={() => (explain = null)}
      />
    {/if}
  </div>
{/if}

<style>
  /*
   * Diff palette: status hues from the app tokens (green add / red delete),
   * mixed over the surrounding surface by the CSS engine — never in JS.
   * These custom properties are consumed by DiffRow/ImageCompare via
   * Tailwind arbitrary utilities (bg-[var(--diff-*)] etc.), so they must
   * stay defined here on the viewer root.
   */
  .diff-viewer {
    --diff-add-bg: color-mix(in srgb, var(--app-added) 12%, transparent);
    --diff-del-bg: color-mix(in srgb, var(--app-deleted) 12%, transparent);
    --diff-add-gutter: color-mix(in srgb, var(--app-added) 20%, transparent);
    --diff-del-gutter: color-mix(in srgb, var(--app-deleted) 20%, transparent);
    --diff-empty-bg: color-mix(in srgb, var(--muted) 12%, transparent);
    --diff-hl-bg: color-mix(in srgb, var(--app-modified) 32%, transparent);
    --diff-divider: var(--border);
    --diff-file-bg: var(--card);
    --diff-file-accent: color-mix(in srgb, var(--primary) 55%, transparent);
    --diff-hunk-bg: color-mix(in srgb, var(--primary) 6%, var(--card));
    --diff-hunk-accent: color-mix(in srgb, var(--primary) 65%, transparent);
    /* M12 line selection highlight. */
    --diff-sel-bg: color-mix(in srgb, var(--primary) 24%, transparent);
    /* Find-in-diff: soft wash for matches, stronger for the active one. */
    --diff-find-bg: color-mix(in srgb, var(--app-modified) 24%, transparent);
    --diff-find-active-bg: color-mix(in srgb, var(--primary) 45%, transparent);
  }

  /*
   * Dark surfaces swallow the light-mode washes (12% over near-black is
   * barely a tint). Strengthen row/gutter/highlight tints so changed blocks
   * stay recognizable; the status hues themselves switch to their bright
   * dark variants via app.css `.dark`.
   */
  :global(.dark) .diff-viewer {
    --diff-add-bg: color-mix(in srgb, var(--app-added) 20%, transparent);
    --diff-del-bg: color-mix(in srgb, var(--app-deleted) 20%, transparent);
    --diff-add-gutter: color-mix(in srgb, var(--app-added) 34%, transparent);
    --diff-del-gutter: color-mix(in srgb, var(--app-deleted) 34%, transparent);
    --diff-empty-bg: color-mix(in srgb, var(--muted) 20%, transparent);
    --diff-hl-bg: color-mix(in srgb, var(--app-modified) 42%, transparent);
    --diff-sel-bg: color-mix(in srgb, var(--primary) 30%, transparent);
    --diff-find-bg: color-mix(in srgb, var(--app-modified) 36%, transparent);
    --diff-find-active-bg: color-mix(in srgb, var(--primary) 52%, transparent);
  }

  /* Split-view selection paint (see the split view block in the script):
     the segments are ranges registered in CSS.highlights. The Custom
     Highlight API paints only color/background/text-decoration, which is
     all a text selection needs. */
  :global(::highlight(mygitui-diff-sel)) {
    background-color: color-mix(in srgb, var(--primary) 35%, transparent);
    color: var(--foreground);
  }
</style>
