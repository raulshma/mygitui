<script lang="ts">
  /**
   * HistoryView (B3 lane) — filter bar + streamed commit log + commit detail.
   *
   * Layout (top to bottom):
   *   1. filter bar (text / author / path / after / before; regex shown but
   *      disabled until M1.1) with an optional CompareBar strip,
   *   2. the log: DOM commit rows (left) beside GraphCanvas (right, 220px).
   *      GraphCanvas owns its native scroll container (B2 contract), so the
   *      two panes keep their scrollTops in lockstep instead of sharing one
   *      scroller: the visible DOM-row window IS the canvas's reported
   *      window (`onVisibleRowsChange`), the DOM pane hides its own gutter,
   *      and the canvas's scrollbar effectively drives both. Wheel over
   *      either pane, keyboard nav, and `scrollToRow`/`scrollToIndex` all
   *      converge (equality-guarded pushes — no feedback loop). If B2's
   *      internal `.gc-scroll` element can't be found, sync degrades to the
   *      exported `scrollToRow(index)` + a locally computed window.
   *   3. commit detail pane (bottom): full message, author/committer dates,
   *      clickable parents, and the commit's diff (repoDiff parent0 → sha,
   *      lazy + loading state) rendered in DiffViewer; per-file "Blame"
   *      swaps in a BlameView, and CompareBar results render here too.
   *      Both boundaries are user-resizable via `SplitPane` — list ↔ detail
   *      (stacked) and metadata ↔ diff (side by side) — with the ratios
   *      persisted as global prefs (`splitPrefs`, `mygitui.split.*`).
   *
   * Data: one HistoryStore per mounted view (see `$lib/stores/history.svelte`)
   * started/destroyed in an `$effect` keyed on `repoId`.
   *
   * Keyboard: the list is a role="listbox" — j/k or arrows move the
   * selection (g/G/Home/End jump), and aria-activedescendant tracks it.
   */
  import { untrack } from "svelte";
  import { cherryPick, repoDiff, revertCommits } from "$lib/ipc/client";
  import { describe, archiveSpec, pickSaveFile } from "$lib/ipc/client";
  import { branchCreate, tagCreate } from "$lib/ipc/client";
  import type { CommitInfo, FileDiff, MergeResult } from "$lib/ipc/types";
  import GraphCanvas from "$lib/components/graph/GraphCanvas.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import BlameView from "$lib/components/panels/BlameView.svelte";
  import CompareBar from "$lib/components/panels/CompareBar.svelte";
  import RebasePlanner from "$lib/components/rebase/RebasePlanner.svelte";
  import { orderForCherryPick } from "$lib/components/rebase/plannerModel";
  import { HistoryStore } from "$lib/stores/history.svelte";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import { bookmarks as bookmarkStore } from "$lib/components/graph/bookmarks.svelte";
  import { onUiEvent } from "$lib/palette/events";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { openPanelPopout } from "$lib/layout/popout";
  import SplitPane from "$lib/components/layout/SplitPane.svelte";
  import { readSplitRatio, writeSplitRatio } from "$lib/layout/splitPrefs";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import ResetDialog from "$lib/components/safety/ResetDialog.svelte";
  import TagAtDialog from "$lib/components/panels/TagAtDialog.svelte";
  import SignatureBadge from "$lib/components/commit/SignatureBadge.svelte";
  import {
    classifyRef,
    formatDateTime,
    formatRelativeTime,
    shortRefName,
  } from "$lib/stores/history-logic";
  import { toast } from "$lib/toast";

  let {
    repoId,
    root = "",
    onPopout,
    bookmarks,
    branchColors,
  }: {
    repoId: string;
    /** Worktree root (bookmark persistence key); empty in popouts. */
    root?: string;
    onPopout?: () => void;
    /** M7: bookmarked shas → dashed ring markers on graph nodes (optional;
     *  popouts omit it — display-only decorations, safe to be absent). */
    bookmarks?: ReadonlySet<string>;
    /** M7: refs decorations → branch color override (optional). */
    branchColors?: (refs: string[]) => string | null;
  } = $props();

  /** Must match the row height passed to (and defaulted by) GraphCanvas. */
  const ROW_HEIGHT = 24;
  const GRAPH_WIDTH = 220;
  const OVERSCAN = 10;

  const store = new HistoryStore();

  // -- selection + detail state ------------------------------------------------

  let selectedSha = $state<string | null>(null);
  /** Last known CommitInfo for the selection (survives filter restarts). */
  let selectedInfo = $state<CommitInfo | null>(null);
  let detailOpen = $state(false);
  let showCompare = $state(false);
  let compare = $state<{ files: FileDiff[]; base: string; target: string } | null>(null);
  let blameTarget = $state<{ path: string; from: string } | null>(null);
  let detailFiles = $state<FileDiff[] | null>(null);
  let detailLoading = $state(false);
  let diffToken = 0;
  let describeToken = 0;

  // -- resizable panes (SplitPane ratios, persisted as global prefs) --------

  /** Commit list fraction of the history panel (detail takes the rest). */
  const DETAIL_RATIO_KEY = "history-detail";
  const META_RATIO_KEY = "history-meta";
  let detailRatio = $state(readSplitRatio(DETAIL_RATIO_KEY, 0.62));
  /** Commit-metadata fraction of the detail pane (diff takes the rest). */
  let metaRatio = $state(readSplitRatio(META_RATIO_KEY, 0.34));

  function setDetailRatio(ratio: number): void {
    detailRatio = ratio;
    writeSplitRatio(DETAIL_RATIO_KEY, ratio);
  }

  function setMetaRatio(ratio: number): void {
    metaRatio = ratio;
    writeSplitRatio(META_RATIO_KEY, ratio);
  }

  // -- commit actions (M3 E2) -------------------------------------------------

  /** Multi-selection as row indices (list order); shift-click range. */
  let multiIdx = $state<number[]>([]);
  /** Anchor row for shift-click ranges (last plain click). */
  let anchorIdx: number | null = null;
  /** True while a cherry-pick/revert IPC is in flight. */
  let opBusy = $state(false);
  let plannerOpen = $state(false);
  let plannerBaseSha = $state<string | null>(null);

  // M12 commit-menu dialogs (sha the dialog operates on, null = closed).
  /** "Branch from here…" PromptDialog open for this sha. */
  let branchFromSha = $state<string | null>(null);
  /** "Tag here…" TagAtDialog open for this sha. */
  let tagAtSha = $state<string | null>(null);
  /** "Reset current branch to here…" ResetDialog open for this sha. */
  let resetToSha = $state<string | null>(null);

  // -- virtualizer + canvas scroll sync ------------------------------------------

  let scrollerEl = $state<HTMLDivElement | undefined>(undefined);
  let graphColEl = $state<HTMLDivElement | undefined>(undefined);
  let canvasRef = $state<unknown>(null);
  /** B2's internal scroll element (`.gc-scroll`) — exact sync fast path. */
  let canvasScrollEl: HTMLDivElement | null = null;
  let ownRange = $state({ start: 0, end: 0 });
  let canvasWindow = $state<{ first: number; last: number } | null>(null);

  $effect(() => {
    // Grab (or drop) the canvas's internal scroller when its column mounts.
    canvasScrollEl = graphColEl
      ? (graphColEl.querySelector<HTMLDivElement>(":scope .gc-scroll") ?? null)
      : null;
  });

  /** Render window: the canvas's reported window when live, else our own. */
  const range = $derived(
    canvasWindow && canvasWindow.last > canvasWindow.first
      ? { start: Math.max(0, canvasWindow.first), end: canvasWindow.last }
      : ownRange,
  );

  function updateOwnRange(): void {
    const el = scrollerEl;
    if (!el || el.clientHeight === 0) return;
    const first = Math.floor(el.scrollTop / ROW_HEIGHT);
    const count = Math.ceil(el.clientHeight / ROW_HEIGHT);
    const start = Math.max(0, first - OVERSCAN);
    const end = Math.min(total, first + count + OVERSCAN);
    if (start !== ownRange.start || end !== ownRange.end) ownRange = { start, end };
  }

  /** Exported-handle fallback (B2 contract: `scrollToRow(index)` centers). */
  function syncCanvasToRow(index: number): void {
    const exports = canvasRef as { scrollToRow?: (i: number) => void } | null;
    if (exports && typeof exports.scrollToRow === "function") exports.scrollToRow(index);
  }

  /** Mirrors our scrollTop into the canvas (exact when possible). */
  function pushScrollToCanvas(): void {
    const mine = scrollerEl;
    if (!mine) return;
    if (
      canvasScrollEl &&
      Math.abs(canvasScrollEl.scrollTop - mine.scrollTop) >= 1
    ) {
      canvasScrollEl.scrollTop = mine.scrollTop;
    } else if (!canvasScrollEl) {
      syncCanvasToRow(
        Math.floor((mine.scrollTop + mine.clientHeight / 2) / ROW_HEIGHT),
      );
    }
  }

  function onScroll(): void {
    updateOwnRange();
    pushScrollToCanvas();
    if (range.end >= total) store.loadMore();
  }

  $effect(() => {
    // Splitter drags resize the scroller without a scroll event: re-window
    // the virtualizer as the box changes (guarded — ResizeObserver is
    // absent in some test environments).
    const el = scrollerEl;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => updateOwnRange());
    observer.observe(el);
    return () => observer.disconnect();
  });

  /** Canvas reports its drawn window (incl. its overscan) — adopt + follow. */
  function onCanvasVisible(first: number, last: number): void {
    canvasWindow = { first, last };
    const mine = scrollerEl;
    if (
      canvasScrollEl &&
      mine &&
      Math.abs(mine.scrollTop - canvasScrollEl.scrollTop) >= 1
    ) {
      // The user scrolled the canvas directly — follow it exactly. Our
      // resulting scroll event pushes the same value back (no-op).
      mine.scrollTop = canvasScrollEl.scrollTop;
    }
  }

  $effect(() => {
    // Fill the viewport when data first lands (or grows past the window).
    void total;
    updateOwnRange();
    if (range.end >= total && total > 0) store.loadMore();
  });

  function scrollToIndex(index: number): void {
    const el = scrollerEl;
    if (!el) {
      syncCanvasToRow(index);
      return;
    }
    const top = index * ROW_HEIGHT;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW_HEIGHT > el.scrollTop + el.clientHeight) {
      el.scrollTop = top + ROW_HEIGHT - el.clientHeight;
    }
    pushScrollToCanvas();
  }

  // -- lifecycle ------------------------------------------------------------------

  $effect(() => {
    store.start(repoId);
    return () => store.destroy();
  });

  let lastRepoId: string | null = null;

  // Reset view state when switching repos (scroll position + range).
  $effect(() => {
    if (lastRepoId !== null && repoId !== lastRepoId) {
      untrack(() => {
        selectedSha = null;
        selectedInfo = null;
        detailOpen = false;
        compare = null;
        blameTarget = null;
        multiIdx = [];
        anchorIdx = null;
        plannerOpen = false;
        branchFromSha = null;
        tagAtSha = null;
        resetToSha = null;
        ownRange = { start: 0, end: 0 };
        canvasWindow = null;
        if (scrollerEl) scrollerEl.scrollTop = 0;
        if (canvasScrollEl) canvasScrollEl.scrollTop = 0;
      });
    }
    lastRepoId = repoId;
  });

  // -- derived ----------------------------------------------------------------------

  const commits = $derived(store.flat.commits);
  const total = $derived(commits.length);
  const selectedIdx = $derived.by(() => {
    void store.flat; // re-resolve after stream restarts reset the index
    return selectedSha ? store.indexOfSha(selectedSha) : -1;
  });

  /** Shas of the multi-selection (cherry-pick/revert input), list order. */
  const selectedShas = $derived.by(() => {
    const out: string[] = [];
    for (const i of multiIdx) {
      const commit = commits[i];
      if (commit) out.push(commit.sha);
    }
    return out;
  });
  const filterActive = $derived(store.filterActive);

  const activeDescendant = $derived(
    selectedIdx >= range.start && selectedIdx < range.end
      ? `commit-row-${selectedIdx}`
      : undefined,
  );

  const visible = $derived.by(() => {
    const start = Math.max(0, Math.min(range.start, total));
    const end = Math.max(start, Math.min(range.end, total));
    const out: Array<{ commit: CommitInfo; idx: number }> = [];
    for (let i = start; i < end; i++) {
      const commit = commits[i];
      if (commit) out.push({ commit, idx: i });
    }
    return out;
  });

  // -- selection ----------------------------------------------------------------------

  function select(
    sha: string,
    opts: { scroll?: boolean; open?: boolean; keepMulti?: boolean } = {},
  ): void {
    selectedSha = sha;
    const idx = store.indexOfSha(sha);
    const commit = idx >= 0 ? commits[idx] : null;
    if (commit) selectedInfo = commit;
    if (!opts.keepMulti) {
      multiIdx = idx >= 0 ? [idx] : [];
      anchorIdx = idx >= 0 ? idx : null;
    }
    if (opts.open !== false) detailOpen = Boolean(selectedInfo);
    if (opts.scroll && idx >= 0) scrollToIndex(idx);
  }

  /**
   * List row click: plain click (re)selects one commit; shift-click extends
   * the multi-selection to the anchor..clicked range (cherry-pick input).
   */
  function onRowClick(sha: string, idx: number, event: MouseEvent): void {
    if (event.shiftKey && anchorIdx !== null) {
      const lo = Math.min(anchorIdx, idx);
      const hi = Math.max(anchorIdx, idx);
      const range: number[] = [];
      for (let i = lo; i <= hi; i++) range.push(i);
      multiIdx = range;
      select(sha, { keepMulti: true });
    } else {
      anchorIdx = idx;
      select(sha);
    }
  }

  function selectParent(sha: string): void {
    if (!store.revealSha(sha)) {
      toast("Parent commit is not in the loaded history");
      return;
    }
    select(sha, { scroll: true });
  }

  // -- M12: bookmark star on the detail header -----------------------------------

  /** Whether the selected commit is bookmarked (drives the star toggle). */
  const selectedBookmarked = $derived(
    root !== "" &&
      selectedInfo !== null &&
      bookmarkStore.has(root, selectedInfo.sha),
  );

  // Bookmark state is lazy per root — hydrate outside derived reads.
  $effect(() => {
    if (root !== "") bookmarkStore.ensure(root);
  });

  function onKeydown(event: KeyboardEvent): void {
    if (total === 0) return;
    const current = selectedIdx;
    let next: number;
    switch (event.key) {
      case "j":
      case "ArrowDown":
        next = Math.min(total - 1, current + 1);
        break;
      case "k":
      case "ArrowUp":
        next = Math.max(0, current < 0 ? 0 : current - 1);
        break;
      case "g":
      case "Home":
        next = 0;
        break;
      case "G":
      case "End":
        next = total - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    const sha = commits[next]?.sha;
    if (sha) select(sha, { scroll: true });
  }

  // -- commit detail diff (lazy, token-guarded) --------------------------------------

  $effect(() => {
    const info = selectedInfo;
    const sha = selectedSha;
    void repoId; // reload on repo switch
    blameTarget = null;
    const token = ++diffToken;
    if (!info || info.parents.length === 0 || !sha) {
      detailFiles = null;
      detailLoading = false;
      return;
    }
    detailLoading = true;
    detailFiles = null;
    repoDiff(repoId, { commit: info.parents[0] as string }, { commit: sha })
      .then((files) => {
        if (token !== diffToken) return;
        detailFiles = files;
        detailLoading = false;
      })
      .catch((err: unknown) => {
        if (token !== diffToken) return;
        detailLoading = false;
        detailFiles = null;
        toast(`Diff failed: ${err instanceof Error ? err.message : String(err)}`, {
          kind: "error",
        });
      });
  });

  // -- M10: describe line for the selected commit ------------------------------

  let describeText = $state<string | null>(null);

  $effect(() => {
    const sha = selectedSha;
    void repoId;
    describeText = null;
    if (!sha || !selectedInfo) return;
    const token = ++describeToken;
    describe(repoId, sha)
      .then((text) => {
        if (token === describeToken) describeText = text;
      })
      .catch(() => {
        /* describe is decorative */
      });
  });

  function openBlame(path: string): void {
    if (!selectedSha) return;
    blameTarget = { path, from: selectedSha };
  }

  function onCompare(files: FileDiff[], base: string, target: string): void {
    compare = { files, base, target };
    blameTarget = null;
    detailOpen = true;
  }

  // -- commit actions (M3 E2) ---------------------------------------------------

  function describeError(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  /** Toasts a mutation result and refreshes the tab status. The history
   *  store restarts on its own via the backend `repo-changed` event. */
  function reportMergeResult(op: string, count: number, result: MergeResult): void {
    if (result.outcome === "conflicted" || result.conflicts.length > 0) {
      toast(`${op} conflicts — resolve in the status panel`, { kind: "error" });
    } else {
      toast(`${op} ${count === 1 ? "1 commit" : `${count} commits`} — done`, {
        kind: "success",
      });
    }
    void tabStore.refreshStatus(repoId);
  }

  async function doCherryPick(): Promise<void> {
    const shas = orderForCherryPick(selectedShas, commits);
    if (opBusy || shas.length === 0) return;
    opBusy = true;
    try {
      const result = await cherryPick(repoId, shas);
      reportMergeResult("Cherry-picked", shas.length, result);
    } catch (err) {
      toast(`Cherry-pick failed: ${describeError(err)}`, { kind: "error" });
    } finally {
      opBusy = false;
    }
  }

  async function doRevert(): Promise<void> {
    const shas = orderForCherryPick(selectedShas, commits);
    if (opBusy || shas.length === 0) return;
    opBusy = true;
    try {
      const result = await revertCommits(repoId, shas);
      reportMergeResult("Reverted", shas.length, result);
    } catch (err) {
      toast(`Revert failed: ${describeError(err)}`, { kind: "error" });
    } finally {
      opBusy = false;
    }
  }

  /** Opens the planner for `HEAD..selected` (the selected commit is the
   *  exclusive base — it is NOT part of the rebase). */
  function rebaseFromHere(): void {
    if (!selectedInfo || opBusy) return;
    plannerBaseSha = selectedInfo.sha;
    plannerOpen = true;
  }

  /**
   * Writes `text` to the clipboard with success/error toasts (the shared
   * helper behind "Copy sha" and M12's "Copy describe").
   */
  async function copyToClipboard(
    text: string,
    okMessage: string,
    errMessage: string,
  ): Promise<boolean> {
    try {
      await navigator.clipboard.writeText(text);
      toast(okMessage, { kind: "success" });
      return true;
    } catch {
      toast(errMessage, { kind: "error" });
      return false;
    }
  }

  async function copySha(): Promise<void> {
    if (!selectedInfo) return;
    await copyToClipboard(
      selectedInfo.sha,
      "SHA copied to clipboard",
      "Could not copy the SHA",
    );
  }

  /** M12: `git describe` for a commit → clipboard (menu + detail action). */
  async function copyDescribe(sha: string): Promise<void> {
    try {
      const text = await describe(repoId, sha);
      await copyToClipboard(
        text,
        `Describe copied: ${text}`,
        "Could not copy the describe",
      );
    } catch (err) {
      toast(`Describe failed: ${describeError(err)}`, { kind: "error" });
    }
  }

  // -- M12: branch / tag / reset "from here" (commit menu) ----------------------

  /** Creates a branch at `sha` (not checked out — HEAD stays put). */
  async function createBranchFrom(sha: string, name: string): Promise<void> {
    branchFromSha = null;
    try {
      await branchCreate(repoId, name, false, sha);
      toast(`Created branch ${name} at ${sha.slice(0, 7)}`, { kind: "success" });
      void tabStore.refreshStatus(repoId);
    } catch (err) {
      toast(
        `Create branch failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  /** Creates a (lightweight or annotated) tag at `sha`. */
  async function createTagAt(
    sha: string,
    name: string,
    annotated: boolean,
    message: string,
  ): Promise<void> {
    tagAtSha = null;
    try {
      // The backend makes a tag annotated by passing a message.
      await tagCreate(repoId, name, sha, annotated ? message : undefined);
      toast(`Created tag ${name} at ${sha.slice(0, 7)}`, { kind: "success" });
    } catch (err) {
      toast(
        `Tag create failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  /** ResetDialog finished: the history restarts via repo-changed. */
  function onResetFromHereDone(): void {
    resetToSha = null;
    void tabStore.refreshStatus(repoId);
  }

  function onRebaseFinished(): void {
    // History restarts itself via repo-changed (HEAD moved); refresh status.
    void tabStore.refreshStatus(repoId);
  }

  // -- M9 F1: palette event wiring + context menus ---------------------------

  let filterTextEl = $state<HTMLInputElement | undefined>(undefined);

  // Palette commands ride the typed event bus (palette/events.ts).
  $effect(() => {
    const unlisteners = [
      onUiEvent("history-focus-filter", () => filterTextEl?.focus()),
      onUiEvent("history-clear-filter", () => store.clearFilter()),
      onUiEvent("history-refresh", () => store.restart()),
      onUiEvent("history-toggle-blame", () => {
        if (blameTarget) {
          blameTarget = null;
        } else if (selectedSha && detailFiles && detailFiles.length > 0) {
          openBlame(detailFiles[0]!.path);
        } else {
          toast("Select a commit with file changes to blame");
        }
      }),
      onUiEvent("history-select-commit", ({ sha }) => {
        if (store.revealSha(sha)) {
          select(sha, { scroll: true });
        }
      }),
      onUiEvent("commit-action", ({ sha, action }) => {
        if (repoId === "") return;
        select(sha);
        if (action === "cherry-pick") void doCherryPick();
        else if (action === "revert") void doRevert();
        else if (action === "bookmark") toggleBookmark(sha);
      }),
    ];
    return () => {
      for (const unlisten of unlisteners) unlisten();
    };
  });

  /** Flips the commit's bookmark (persisted per repo root). */
  function toggleBookmark(sha: string): void {
    if (root === "") return;
    bookmarkStore.ensure(root);
    const added = bookmarkStore.toggle(root, sha);
    toast(added ? "Commit bookmarked" : "Bookmark removed", {
      kind: "success",
    });
  }

  /** Row right-click: the commit action menu. */
  function commitMenu(event: MouseEvent, sha: string, idx: number): void {
    onRowClick(sha, idx, event);
    const bookmarked = root !== "" && bookmarkStore.has(root, sha);
    const entries: MenuEntry[] = [
      { id: "pick", label: "Cherry-pick…", run: () => void doCherryPick() },
      { id: "revert", label: "Revert…", run: () => void doRevert() },
      { id: "rebase", label: "Rebase from here…", run: rebaseFromHere },
      { id: "bookmark", label: bookmarked ? "Remove bookmark" : "Bookmark commit", run: () => toggleBookmark(sha) },
      { id: "branch-from", label: "Branch from here…", run: () => (branchFromSha = sha) },
      { id: "tag-at", label: "Tag here…", run: () => (tagAtSha = sha) },
      { id: "reset-to", label: "Reset current branch to here…", run: () => (resetToSha = sha) },
      { id: "copy", label: "Copy sha", run: copySha },
      { id: "copy-describe", label: "Copy describe", run: () => void copyDescribe(sha) },
      { id: "archive", label: "Export archive…", run: () => void exportArchive(sha) },
    ];
    showMenuAt(event, entries);
  }

  /** M11: zip/tar export of the commit's tree via the save picker. */
  async function exportArchive(spec: string): Promise<void> {
    const destination = await pickSaveFile("Export archive", `archive-${spec.slice(0, 8)}.zip`, [
      { name: "Zip archive", extensions: ["zip"] },
      { name: "Tar archive", extensions: ["tar", "gz"] },
    ]);
    if (!destination) return;
    const format = destination.endsWith(".zip")
      ? "zip"
      : destination.endsWith(".tar")
        ? "tar"
        : "tar.gz";
    try {
      await archiveSpec(repoId, spec, format, destination);
      toast(`Archive written to ${destination}`, { kind: "success" });
    } catch (err) {
      toast(
        `Archive failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  /** Changed-file right-click: per-file actions. */
  function fileMenu(event: MouseEvent, path: string): void {
    const entries: MenuEntry[] = [
      { id: "history", label: "File history", run: () => void openPanelPopout("filehistory", repoId, `History: ${path}`, { path }) },
      { id: "blame", label: "Blame", run: () => openBlame(path) },
    ];
    showMenuAt(event, entries);
  }
</script>

<section class="history" aria-label="Commit history">
  <!-- 1. filter bar -->
  <div class="filterbar" role="search" aria-label="Filter commits">
    <input
      class="f"
      bind:this={filterTextEl}
      type="search"
      placeholder="Search commits"
      aria-label="Search commit text"
      value={store.filter.text}
      oninput={(e) => store.setFilter({ text: e.currentTarget.value })}
    />
    <input
      class="f"
      placeholder="Author"
      aria-label="Filter by author"
      value={store.filter.author}
      oninput={(e) => store.setFilter({ author: e.currentTarget.value })}
    />
    <input
      class="f wide"
      placeholder="Path"
      aria-label="Filter by path"
      value={store.filter.path}
      oninput={(e) => store.setFilter({ path: e.currentTarget.value })}
    />
    <label class="date">
      <span class="dl">after</span>
      <input
        type="date"
        aria-label="Commits after this date"
        value={store.filter.after}
        onchange={(e) => store.setFilter({ after: e.currentTarget.value })}
      />
    </label>
    <label class="date">
      <span class="dl">before</span>
      <input
        type="date"
        aria-label="Commits before this date"
        value={store.filter.before}
        onchange={(e) => store.setFilter({ before: e.currentTarget.value })}
      />
    </label>
    <!-- M9: regex mode is live (backend `LogFilter.regex`). -->
    <label class="regex" title="Treat the search text as a regular expression">
      <input
        type="checkbox"
        checked={store.filter.regex}
        onchange={(e) => store.setFilter({ regex: e.currentTarget.checked })}
      />
      <span>Regex</span>
    </label>
    <!-- M10: pickaxe -S — commits whose patch adds/removes the string. -->
    <input
      class="f"
      placeholder="Changes containing… (-S)"
      aria-label="Pickaxe: patches adding or removing this string"
      value={store.filter.pickaxe}
      oninput={(e) => store.setFilter({ pickaxe: e.currentTarget.value })}
    />
    <!-- M12: pickaxe -G — commits whose patch adds/removes lines matching
         this regex. -S and -G may BOTH be set: the values are forwarded
         verbatim and the backend intersects them (simplest contract — no
         cross-clearing UI to get out of sync). -->
    <input
      class="f"
      placeholder="patch regex (-G)"
      aria-label="Pickaxe: patches adding or removing lines matching this regex"
      value={store.filter.pickaxeRegex ?? ""}
      oninput={(e) => store.setFilter({ pickaxeRegex: e.currentTarget.value })}
    />

    {#if filterActive}
      <button class="clear" onclick={() => store.clearFilter()}>Clear</button>
    {/if}

    <button
      class="compare-toggle"
      aria-expanded={showCompare}
      onclick={() => (showCompare = !showCompare)}
    >
      Compare…
    </button>

    {#if onPopout}
      <!-- M4 F1: pop the history out into its own window (desktop only). -->
      <button class="compare-toggle" title="Pop out history" onclick={onPopout}>
        Pop out
      </button>
    {/if}

    <span class="status" role="status" aria-live="polite">
      {#if store.error}
        <span class="err" role="alert">{store.error}</span>
        <button class="retry" onclick={() => store.restart()}>Retry</button>
      {:else if store.loading}
        Streaming…
      {:else}
        {total}
        {total === 1 ? "commit" : "commits"}
        {#if store.hasMore}({store.pendingCount} pages buffered — scroll to load){/if}
      {/if}
    </span>
  </div>

  {#if showCompare}
    <CompareBar {repoId} {onCompare} />
  {/if}

  <!-- 2. log: DOM rows (left) + GraphCanvas (right), scroll-synced.
       Declared as a snippet: it renders standalone (detail closed) or as
       pane `a` of the detail SplitPane (detail open) — one copy of the
       virtualizer markup. -->
  {#snippet logPane()}
  <div class="listwrap">
    {#if !store.loading && !store.error && total === 0}
      <p class="empty">No commits match the current filters.</p>
    {:else}
      <div
        class="scroller"
        bind:this={scrollerEl}
        onscroll={onScroll}
        role="listbox"
        aria-label="Commits"
        tabindex="0"
        aria-activedescendant={activeDescendant}
        onkeydown={onKeydown}
      >
        <div class="spacer" style:height={`${total * ROW_HEIGHT}px`}>
          {#each visible as v (v.commit.sha)}
            <div
              id={`commit-row-${v.idx}`}
              class="row"
              class:selected={v.idx === selectedIdx}
              class:multisel={v.idx !== selectedIdx && multiIdx.includes(v.idx)}
              role="option"
              tabindex="-1"
              aria-selected={v.idx === selectedIdx || multiIdx.includes(v.idx)}
              style:top={`${v.idx * ROW_HEIGHT}px`}
              style:height={`${ROW_HEIGHT}px`}
              onclick={(e) => onRowClick(v.commit.sha, v.idx, e)}
              oncontextmenu={(e) => commitMenu(e, v.commit.sha, v.idx)}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  select(v.commit.sha);
                }
              }}
            >
              <span class="sha">{v.commit.sha.slice(0, 7)}</span>
              {#each v.commit.refs as ref (ref)}
                <span class="ref ref-{classifyRef(ref)}">{shortRefName(ref)}</span>
              {/each}
              <span class="summary" title={v.commit.summary}>{v.commit.summary}</span>
              <span class="author">{v.commit.author.name}</span>
              <span class="date">{formatRelativeTime(v.commit.author.time)}</span>
            </div>
          {/each}
        </div>
      </div>
      <div class="graphcol" bind:this={graphColEl} style:width={`${GRAPH_WIDTH}px`}>
        <GraphCanvas
          pages={store.pages}
          rowHeight={ROW_HEIGHT}
          onCommitClick={(sha) => select(sha)}
          onReachEnd={() => store.loadMore()}
          onVisibleRowsChange={onCanvasVisible}
          {bookmarks}
          {branchColors}
          bind:this={canvasRef}
        />
      </div>
    {/if}
  </div>
  {/snippet}

  <!-- 3. commit detail — open: the log and the detail pane share a stacked
       SplitPane (the list fraction is user-resizable + persisted); closed:
       a slim reopen strip. -->
  {#if detailOpen && (selectedInfo || compare)}
    <SplitPane
      axis="y"
      ratio={detailRatio}
      onRatio={setDetailRatio}
      label="Resize commit list and detail"
    >
      {#snippet a()}
        {@render logPane()}
      {/snippet}
      {#snippet b()}
    <div class="detail" aria-label="Commit detail">
      <header class="dhead">
        {#if selectedInfo}
          <span class="dsha">{selectedInfo.sha.slice(0, 7)}</span>
          <span class="dsummary" title={selectedInfo.summary}>{selectedInfo.summary}</span>
          {#if root !== ""}
            <!-- M12: bookmark star (same store mutation as the menu item). -->
            <button
              class="star"
              aria-pressed={selectedBookmarked}
              aria-label={selectedBookmarked
                ? "Remove bookmark from this commit"
                : "Bookmark this commit"}
              title={selectedBookmarked ? "Remove bookmark" : "Bookmark commit"}
              onclick={() => {
                // Snippet narrowing doesn't reach the callback closure.
                const sha = selectedInfo?.sha;
                if (sha) toggleBookmark(sha);
              }}
            >
              {selectedBookmarked ? "★" : "☆"}
            </button>
          {/if}
        {:else}
          <span class="dsha">diff</span>
          <span class="dsummary">{compare?.base} → {compare?.target}</span>
        {/if}
        <button class="dclose" onclick={() => (detailOpen = false)} aria-label="Close detail">
          ×
        </button>
      </header>
      {#if selectedInfo}
        <div class="cactions" role="toolbar" aria-label="Commit actions">
          <button class="act" onclick={doCherryPick} disabled={opBusy}>
            Cherry-pick{selectedShas.length > 1 ? ` (${selectedShas.length})` : ""}
          </button>
          <button class="act" onclick={doRevert} disabled={opBusy}>
            Revert{selectedShas.length > 1 ? ` (${selectedShas.length})` : ""}
          </button>
          <button
            class="act"
            onclick={rebaseFromHere}
            disabled={opBusy}
            title={`Rebase HEAD..${selectedInfo.sha.slice(0, 7)} (selected commit is the exclusive base)`}
          >
            Rebase from here
          </button>
          <button class="act" onclick={copySha}>Copy sha</button>
          <span class="cinfo" role="status">
            {#if selectedShas.length > 1}
              {selectedShas.length} commits selected (shift-click to extend)
            {/if}
          </span>
        </div>
      {/if}
      <div class="dbody">
        {#snippet filesPane()}
        <div class="dfiles">
          {#if blameTarget}
            <div class="dtab">
              <span>Blame: {blameTarget.path}</span>
              <button
                onclick={() => (blameTarget = null)}
                aria-label="Close blame">×
              </button>
            </div>
            <div class="dtabbody">
              <BlameView {repoId} path={blameTarget.path} from={blameTarget.from} />
            </div>
          {:else if compare}
            <div class="dtab">
              <span>Compare: {compare.base} → {compare.target} ({compare.files.length} files)</span>
              <button onclick={() => (compare = null)} aria-label="Close compare">×</button>
            </div>
            <div class="dtabbody">
              <DiffViewer files={compare.files} />
            </div>
          {:else if detailLoading}
            <p class="dstate" role="status">Loading diff…</p>
          {:else if !selectedInfo || selectedInfo.parents.length === 0}
            <p class="dstate">Root commit — nothing to diff against.</p>
          {:else if detailFiles === null}
            <p class="dstate">Diff unavailable.</p>
          {:else if detailFiles.length === 0}
            <p class="dstate">No changes against first parent.</p>
          {:else}
            <ul class="filelist" aria-label="Changed files">
              {#each detailFiles as file (file.path)}
                <li
                  oncontextmenu={(e) => fileMenu(e, file.path)}
                >
                  <span class="fpath" title={file.path}>
                    {#if file.old_path}{file.old_path} → {/if}{file.path}
                  </span>
                  <span class="stats">
                    {#if !file.binary}
                      <span class="add">+{file.additions}</span>
                      <span class="del">−{file.deletions}</span>
                    {:else}
                      <span class="muted">binary</span>
                    {/if}
                  </span>
                  <button class="blame" onclick={() => openBlame(file.path)}>Blame</button>
                  <button
                    class="blame"
                    title="History of {file.path} (popout)"
                    onclick={() =>
                      void openPanelPopout("filehistory", repoId, `History: ${file.path}`, { path: file.path })}
                  >History</button>
                </li>
              {/each}
            </ul>
            <div class="ddiff">
              <DiffViewer files={detailFiles} />
            </div>
          {/if}
        </div>
        {/snippet}

        {#if selectedInfo}
          <!-- Meta ↔ files/diff: the detail's second resizable split. -->
          <SplitPane
            axis="x"
            ratio={metaRatio}
            onRatio={setMetaRatio}
            label="Resize commit metadata and diff"
          >
            {#snippet a()}
              <!-- Snippet bodies don't inherit the outer {#if selectedInfo}
                   narrowing — re-guard here (renders only while active). -->
              {#if selectedInfo}
        <aside class="dmeta">
            <dl>
              <dt>Author</dt>
              <dd>
                {selectedInfo.author.name}
                <span class="muted">&lt;{selectedInfo.author.email}&gt;</span><br />
                {formatDateTime(selectedInfo.author.time)}
                <span class="muted">({formatRelativeTime(selectedInfo.author.time)})</span>
              </dd>
              <dt>Committer</dt>
              <dd>
                {selectedInfo.committer.name}<br />
                {formatDateTime(selectedInfo.committer.time)}
                <span class="muted">({formatRelativeTime(selectedInfo.committer.time)})</span>
              </dd>
              <dt>Parents</dt>
              <dd>
                {#if selectedInfo.parents.length === 0}
                  <em>root</em>
                {:else}
                  {#each selectedInfo.parents as p (p)}
                    <button class="parent" onclick={() => selectParent(p)} title={"Go to " + p}>
                      {p.slice(0, 7)}
                    </button>
                  {/each}
                {/if}
              </dd>
              <dt>Describe</dt>
              <dd>
                {#if describeText}
                  <code>{describeText}</code>
                {:else}
                  <span class="muted">…</span>
                {/if}
              </dd>
              <!-- M12: commit-signature verification badge. -->
              <dt>Signature</dt>
              <dd><SignatureBadge {repoId} sha={selectedInfo.sha} /></dd>
              {#if selectedInfo.refs.length > 0}
                <dt>Refs</dt>
                <dd class="drefs">
                  {#each selectedInfo.refs as ref (ref)}
                    <span class="ref ref-{classifyRef(ref)}">{shortRefName(ref)}</span>
                  {/each}
                </dd>
              {/if}
            </dl>
            <pre class="msg">{selectedInfo.message}</pre>
          </aside>
              {/if}
            {/snippet}
            {#snippet b()}
              {@render filesPane()}
            {/snippet}
          </SplitPane>
        {:else}
          {@render filesPane()}
        {/if}
      </div>
    </div>
      {/snippet}
    </SplitPane>
  {:else}
    {@render logPane()}
    {#if selectedInfo}
      <button class="detail-open" onclick={() => (detailOpen = true)} aria-expanded="false">
        Show commit detail — {selectedInfo.summary}
      </button>
    {/if}
  {/if}

  <!-- 4. interactive rebase planner (M3 E2) -->
  {#if plannerOpen}
    <RebasePlanner
      {repoId}
      baseSha={plannerBaseSha}
      onClose={() => (plannerOpen = false)}
      onFinished={onRebaseFinished}
    />
  {/if}

  <!-- 5. M12 commit-menu dialogs (branch / tag / reset "from here") -->
  {#if branchFromSha}
    <PromptDialog
      open={true}
      title={`Branch from ${branchFromSha.slice(0, 7)}`}
      message="Creates the branch at this commit; your current branch stays checked out."
      placeholder="New branch name"
      confirmLabel="Create branch"
      onSubmit={(name) => void createBranchFrom(branchFromSha ?? "", name)}
      onCancel={() => (branchFromSha = null)}
    />
  {/if}

  {#if tagAtSha}
    <TagAtDialog
      sha={tagAtSha}
      onSubmit={(name, annotated, message) =>
        void createTagAt(tagAtSha ?? "", name, annotated, message)}
      onCancel={() => (tagAtSha = null)}
    />
  {/if}

  {#if resetToSha}
    <ResetDialog
      {repoId}
      open={true}
      defaultTarget={resetToSha}
      onClose={() => (resetToSha = null)}
      onDone={onResetFromHereDone}
    />
  {/if}
</section>

<style>
  .history {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--m3-surface);
  }

  /* The list ↔ detail SplitPane fills the rest of the panel (its divider
   * carries the user's ratio; the panes size both children). */
  .history > :global(.split) {
    flex: 1;
    min-height: 0;
  }

  /* -- filter bar ------------------------------------------------------------ */

  .filterbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.375rem;
    padding: 0.375rem 0.75rem;
    border-bottom: 1px solid var(--m3-outline-variant);
    background: var(--m3-surface-container, var(--m3-surface));
    font-size: 0.75rem;
  }

  input.f {
    width: 9rem;
    padding: 0.2rem 0.45rem;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface);
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
  }

  input.f.wide {
    width: 12rem;
  }

  input.f:focus-visible,
  input[type="date"]:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  label.date {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--m3-on-surface-variant);
  }

  input[type="date"] {
    padding: 0.15rem 0.3rem;
    font-size: 0.6875rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface);
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
  }

  label.regex {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--m3-on-surface-variant);
    cursor: not-allowed;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }

  button.clear,
  button.compare-toggle,
  button.retry {
    padding: 0.2rem 0.55rem;
    font-size: 0.6875rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-high, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
  }

  button.clear:focus-visible,
  button.compare-toggle:focus-visible,
  button.retry:focus-visible,
  button.dclose:focus-visible,
  button.parent:focus-visible,
  button.blame:focus-visible,
  .dtab button:focus-visible,
  .detail-open:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .status {
    margin-left: auto;
    color: var(--m3-on-surface-variant);
    font-size: 0.6875rem;
    display: flex;
    align-items: center;
    gap: 0.375rem;
  }

  .status .err {
    color: var(--m3-error);
  }

  /* -- log list + graph (scroll-synced panes) --------------------------------- */

  .listwrap {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .empty {
    margin: auto;
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
  }

  /* Rows pane: hides its own gutter — the canvas's scrollbar drives both. */
  .scroller {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    scrollbar-width: none;
  }

  .scroller::-webkit-scrollbar {
    display: none;
  }

  .scroller:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .spacer {
    position: relative;
  }

  .row {
    position: absolute;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0 0.6rem 0 0.75rem;
    font-size: 0.75rem;
    cursor: pointer;
    border-bottom: 1px solid color-mix(in srgb, var(--m3-outline-variant) 45%, transparent);
  }

  .row:hover {
    background: var(--m3-surface-container-low, var(--m3-surface));
  }

  .row.selected {
    background: var(--m3-secondary-container);
    color: var(--m3-on-secondary-container);
  }

  /* shift-click range members (primary selection stays `.selected`) */
  .row.multisel {
    background: color-mix(in srgb, var(--m3-secondary-container) 45%, var(--m3-surface));
  }

  .row .sha {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface-variant);
  }

  .row.selected .sha {
    color: inherit;
  }

  .row .summary {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row .author {
    flex: none;
    max-width: 10rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface-variant);
  }

  .row.selected .author {
    color: inherit;
  }

  .row .date {
    flex: none;
    color: var(--m3-on-surface-variant);
    font-size: 0.6875rem;
  }

  /* ref chips (shared with the detail pane) */
  .ref {
    flex: none;
    max-width: 9rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 0 0.35rem;
    border-radius: var(--m3-shape-extra-small, 4px);
    font-size: 0.6875rem;
    line-height: 1.4;
  }

  .ref-local {
    color: var(--m3-on-primary-container);
    background: var(--m3-primary-container);
  }

  .ref-remote {
    color: var(--m3-on-tertiary-container);
    background: var(--m3-tertiary-container);
  }

  .ref-tag {
    color: var(--m3-on-secondary-container);
    background: var(--m3-secondary-container);
  }

  .ref-head {
    color: var(--m3-inverse-on-surface);
    background: var(--m3-inverse-surface);
  }

  .graphcol {
    flex: none;
    height: 100%;
    min-height: 0;
    border-left: 1px solid var(--m3-outline-variant);
    background: var(--m3-surface);
  }

  /* -- detail pane -------------------------------------------------------------- */

  .detail-open {
    flex: none;
    padding: 0.25rem 0.75rem;
    text-align: left;
    font-size: 0.72rem;
    color: var(--m3-primary);
    background: var(--m3-surface-container, var(--m3-surface));
    border: none;
    border-top: 1px solid var(--m3-outline-variant);
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .detail {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--m3-surface);
  }

  .dhead {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.25rem 0.75rem;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--m3-outline-variant);
    font-size: 0.8125rem;
  }

  .dsha {
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-primary);
  }

  .dsummary {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 500;
    color: var(--m3-on-surface);
  }

  .dclose,
  .dtab button,
  button.blame {
    padding: 0.1rem 0.4rem;
    color: var(--m3-on-surface-variant);
    background: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
    font-size: 0.72rem;
  }

  /* M12: bookmark star in the detail header. */
  button.star {
    flex: none;
    padding: 0.05rem 0.45rem;
    color: var(--m3-on-surface-variant);
    background: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
    font-size: 0.8125rem;
    line-height: 1.3;
  }

  button.star[aria-pressed="true"] {
    color: var(--m3-primary);
    border-color: var(--m3-primary);
  }

  button.star:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .dbody {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  /* SplitPane (meta ↔ files) or the bare files pane fills the body; the
   * child needs :global — the splitter owns its own scope. */
  .dbody > :global(*) {
    flex: 1;
    min-width: 0;
  }

  /* -- commit action bar (M3 E2) ------------------------------------------------ */

  .cactions {
    flex: none;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.35rem;
    padding: 0.25rem 0.75rem;
    border-bottom: 1px solid var(--m3-outline-variant);
    background: var(--m3-surface-container-low, var(--m3-surface));
    font-size: 0.72rem;
  }

  button.act {
    padding: 0.18rem 0.6rem;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-high, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-full, 9999px);
    cursor: pointer;
  }

  button.act:hover:not(:disabled) {
    background: var(--m3-secondary-container);
    color: var(--m3-on-secondary-container);
  }

  button.act:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  button.act:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .cinfo {
    margin-left: auto;
    color: var(--m3-on-surface-variant);
    font-size: 0.6875rem;
  }

  .dmeta {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 0.5rem 0.75rem;
    font-size: 0.75rem;
  }

  .dmeta dl {
    margin: 0;
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.25rem 0.75rem;
  }

  .dmeta dt {
    color: var(--m3-on-surface-variant);
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.02em;
    padding-top: 0.1rem;
  }

  .dmeta dd {
    margin: 0;
    color: var(--m3-on-surface);
  }

  .muted {
    color: var(--m3-on-surface-variant);
  }

  button.parent {
    margin-right: 0.25rem;
    padding: 0.05rem 0.3rem;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    color: var(--m3-primary);
    background: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
  }

  .drefs {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
  }

  .msg {
    margin: 0.5rem 0 0;
    padding: 0.5rem;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-low, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
  }

  .dfiles {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .dtab {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.2rem 0.5rem 0.2rem 0.75rem;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-low, var(--m3-surface));
    border-bottom: 1px solid var(--m3-outline-variant);
  }

  .dtab span {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, Consolas, monospace;
  }

  .dtabbody {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .dstate {
    margin: auto;
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
  }

  .filelist {
    flex: none;
    max-height: 30%;
    margin: 0;
    padding: 0.15rem 0;
    list-style: none;
    overflow-y: auto;
    border-bottom: 1px solid var(--m3-outline-variant);
    font-size: 0.72rem;
  }

  .filelist li {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.05rem 0.75rem;
  }

  .fpath {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-on-surface);
  }

  .stats {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
  }

  .add {
    color: var(--m3-tertiary);
  }

  .del {
    color: var(--m3-error);
  }

  .ddiff {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
</style>
