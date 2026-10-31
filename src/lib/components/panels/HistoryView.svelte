<script lang="ts">
  /**
   * HistoryView (B3 lane) — filter bar + streamed commit log + commit detail.
   *
   * Layout (top to bottom):
   *   1. filter bar (text / author / path / after / before; regex shown but
   *      disabled until M1.1) with an optional CompareBar strip,
   *   2. the log: DOM commit rows (left) beside GraphCanvas (right, 220px).
   *      GraphCanvas owns its native scroll container (B2 contract), exposed
   *      to this view via `bind:scrollEl`, so the two panes keep their
   *      scrollTops in lockstep instead of sharing one scroller: the visible
   *      DOM-row window IS the canvas's reported window
   *      (`onVisibleRowsChange`), the DOM pane hides its own gutter, and the
   *      canvas's scrollbar effectively drives both. Wheel over either pane,
   *      keyboard nav, and `scrollToRow`/`scrollToIndex` all converge
   *      (equality-guarded pushes — no feedback loop). The whole sync lives
   *      in `$lib/stores/historyScrollSync` (unit-tested with fake
   *      elements); this view only wires the real ports. Without the
   *      canvas's scroll element it degrades to the exported
   *      `scrollToRow(index)` + a locally computed window.
   *   3. commit detail pane (bottom, `CommitDetail`): full message,
   *      author/committer dates, clickable parents, and the commit's diff;
   *      per-file "Blame" swaps in a BlameView, and CompareBar results
   *      render there too. The list↔detail boundary is user-resizable via
   *      `SplitPane` (ratio persisted as a global pref, `splitPrefs`); the
   *      detail's own meta↔diff split lives inside `CommitDetail`.
   *
   * Data: one HistoryStore per mounted view (see `$lib/stores/history.svelte`)
   * started/destroyed in an `$effect` keyed on `repoId`. The store applies
   * the persisted per-root filter itself INSIDE `start()` before the first
   * stream request (restore ordering is store-internal code order now); this
   * view hydrates only the non-filter UI state (selection + detail
   * visibility) on mount/re-target and saves everything on change
   * (`historyPrefs`).
   *
   * Keyboard: the list is a role="listbox" — j/k or arrows move the
   * selection (g/G/Home/End jump), and aria-activedescendant tracks it.
   */
  import { untrack } from "svelte";
  import {
    archiveSpec,
    branchCreate,
    cherryPick,
    describe,
    pickSaveFile,
    revertCommits,
    tagCreate,
  } from "$lib/ipc/client";
  import type { CommitInfo, FileDiff, MergeResult } from "$lib/ipc/types";
  import GraphCanvas from "$lib/components/graph/GraphCanvas.svelte";
  import CompareBar from "$lib/components/panels/CompareBar.svelte";
  import type { CompareResult } from "$lib/components/panels/compare";
  import CommitDetail from "$lib/components/panels/CommitDetail.svelte";
  import RebasePlanner from "$lib/components/rebase/RebasePlanner.svelte";
  import { orderForCherryPick } from "$lib/components/rebase/plannerModel";
  import { HistoryStore } from "$lib/stores/history.svelte";
  import { createHistoryScrollSync } from "$lib/stores/historyScrollSync";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import {
    loadHistoryPrefs,
    saveHistoryPrefs,
  } from "$lib/stores/historyPrefs";
  import { bookmarks as bookmarkStore } from "$lib/components/graph/bookmarks.svelte";
  import { onUiEvent } from "$lib/palette/events";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { openPanelPopout } from "$lib/layout/popout";
  import SplitPane from "$lib/components/layout/SplitPane.svelte";
  import { readSplitRatio, writeSplitRatio } from "$lib/layout/splitPrefs";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import ResetDialog from "$lib/components/safety/ResetDialog.svelte";
  import TagAtDialog from "$lib/components/panels/TagAtDialog.svelte";
  import type { HistoryCommitActions } from "$lib/components/panels/historySidebar";
  import {
    classifyRef,
    formatRelativeTime,
    shortRefName,
  } from "$lib/git/format";
  import { toast } from "$lib/toast";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import iconSearch from "@ktibow/iconset-material-symbols/search";
  import iconPopout from "@ktibow/iconset-material-symbols/open-in-new";
  import iconCollapse from "@ktibow/iconset-material-symbols/right-panel-close";
  import iconFilters from "@ktibow/iconset-material-symbols/filter-list";
  import iconClearFilters from "@ktibow/iconset-material-symbols/filter-list-off";
  import iconCompare from "@ktibow/iconset-material-symbols/compare-arrows";
  import iconRetry from "@ktibow/iconset-material-symbols/restart-alt";

  let {
    repoId,
    root = "",
    onPopout,
    bookmarks,
    branchColors,
    variant = "full",
    onSelectCommit,
    onCompareResult,
    onActionsChange,
    onCollapse,
    revealSha = null,
    onRevealHandled,
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
    /**
     * `full` (popouts) renders the list and the detail pane exactly as
     * before. `sidebar` is the workspace's collapsible history list: the
     * detail opens in the main editor pane, so only the list is rendered.
     */
    variant?: "full" | "sidebar";
    /**
     * Sidebar variant: the selected commit is reported upward. `source` is
     * `"restore"` for a persisted selection resolving on mount — the
     * workspace only opens it when the editor is empty (it must not steal
     * an open working-copy diff after history is collapsed and reopened).
     */
    onSelectCommit?: (info: CommitInfo | null, source?: "restore") => void;
    /** Sidebar variant: compare results open in the main editor pane. */
    onCompareResult?: (result: CompareResult) => void;
    /** Sidebar variant: action toolbar state for the center commit detail. */
    onActionsChange?: (actions: HistoryCommitActions | null) => void;
    /** Sidebar variant: collapse control in the panel header. */
    onCollapse?: () => void;
    /**
     * Sidebar variant: reveal + select this commit once it is loaded (parent
     * navigation from the center editor, including when history was just
     * re-opened and this view is mounting for the first time).
     */
    revealSha?: string | null;
    /** Sidebar variant: called after `revealSha` was selected (owner clears it). */
    onRevealHandled?: () => void;
  } = $props();

  /** Sidebar mode owns no detail pane; the center editor does. */
  const sidebar = $derived(variant === "sidebar");

  /** Must match the row height passed to (and defaulted by) GraphCanvas. */
  const ROW_HEIGHT = 28;
  const GRAPH_WIDTH = 220;
  const OVERSCAN = 10;

  const store = new HistoryStore();

  // -- selection + detail state ------------------------------------------------

  let selectedSha = $state<string | null>(null);
  /** Last known CommitInfo for the selection (survives filter restarts). */
  let selectedInfo = $state<CommitInfo | null>(null);
  let detailOpen = $state(false);
  let showCompare = $state(false);
  /** Sidebar variant: reveal author/path/date/pickaxe filters. */
  let showAdvanced = $state(false);
  let compare = $state<CompareResult | null>(null);

  /** Live `CommitDetail` instance (palette blame toggle; null when closed). */
  let detailRef = $state<ReturnType<typeof CommitDetail> | null>(null);

  // -- resizable panes (SplitPane ratios, persisted as global prefs) --------

  /** Commit list fraction of the history panel (detail takes the rest). */
  const DETAIL_RATIO_KEY = "history-detail";
  let detailRatio = $state(readSplitRatio(DETAIL_RATIO_KEY, 0.62));

  function setDetailRatio(ratio: number): void {
    detailRatio = ratio;
    writeSplitRatio(DETAIL_RATIO_KEY, ratio);
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
  /** GraphCanvas's native scroll container (`bind:scrollEl`) — fast path. */
  let canvasScrollEl = $state<HTMLDivElement | undefined>(undefined);
  let canvasRef = $state<{ scrollToRow?: (i: number) => void } | null>(null);
  /** Locally computed render window (degraded mode + fallback). */
  let ownRange = $state({ start: 0, end: 0 });
  /** The canvas's reported drawn window (null until it reports one). */
  let canvasWindow = $state<{ first: number; last: number } | null>(null);

  /**
   * Two-way list↔canvas scroll sync (see `$lib/stores/historyScrollSync`).
   * The ports close over the bound elements; an absent element (null) is
   * what selects the fallback (`scrollToRow`) / degraded (local window)
   * strategies. Window state stays HERE as `$state` — read/written through
   * the ports — so the derived render window below stays reactive.
   */
  const sync = createHistoryScrollSync({
    list: {
      getScrollTop: () => scrollerEl?.scrollTop ?? null,
      setScrollTop: (top) => {
        if (scrollerEl) scrollerEl.scrollTop = top;
      },
      getClientHeight: () => scrollerEl?.clientHeight ?? 0,
    },
    graph: {
      getScrollTop: () => canvasScrollEl?.scrollTop ?? null,
      setScrollTop: (top) => {
        if (canvasScrollEl) canvasScrollEl.scrollTop = top;
      },
      getClientHeight: () => canvasScrollEl?.clientHeight ?? 0,
      // Exported-handle fallback (B2 contract: `scrollToRow(index)` centers).
      scrollToRow: (index) => canvasRef?.scrollToRow?.(index),
    },
    rowHeight: ROW_HEIGHT,
    overscan: OVERSCAN,
    totalRows: () => total,
    ownRange: () => ownRange,
    setOwnRange: (r) => (ownRange = r),
    canvasWindow: () => canvasWindow,
    setCanvasWindow: (w) => (canvasWindow = w),
  });

  /** Render window: the canvas's reported window when live, else our own. */
  const range = $derived(sync.renderWindow());

  function onScroll(): void {
    if (sync.onListScroll().end >= total) store.loadMore();
  }

  $effect(() => {
    // Splitter drags resize the scroller without a scroll event: re-window
    // the virtualizer as the box changes (guarded — ResizeObserver is
    // absent in some test environments).
    const el = scrollerEl;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => sync.updateOwnRange());
    observer.observe(el);
    return () => observer.disconnect();
  });

  $effect(() => {
    // Fill the viewport when data first lands (or grows past the window).
    void total;
    sync.updateOwnRange();
    if (sync.renderWindow().end >= total && total > 0) store.loadMore();
  });

  // -- lifecycle ------------------------------------------------------------------

  /** Scroll to the restored selection once its commit lands in the log. */
  let restoreScroll = false;

  /**
   * Repo re-targeting — on mount and on every repo switch it resets
   * transient view state (dialogs, compare results, scroll/range) and
   * hydrates the non-filter UI state (selection + detail visibility) from
   * the persisted per-root prefs. The FILTER is deliberately not touched
   * here: the store applies the persisted filter itself inside
   * `start()` below, before the first stream request — the ordering
   * invariant is store-internal code order, not effect order.
   * Popouts (root === "") are display-only: nothing is restored or saved.
   */
  let lastTarget: string | null = null;
  $effect(() => {
    const target = `${repoId}\u0000${root}`;
    if (target === lastTarget) return;
    const switching = lastTarget !== null;
    lastTarget = target;
    if (switching) {
      untrack(() => {
        compare = null;
        multiIdx = [];
        anchorIdx = null;
        plannerOpen = false;
        branchFromSha = null;
        tagAtSha = null;
        resetToSha = null;
        sync.reset();
      });
    }
    const prefs = root === "" ? null : loadHistoryPrefs(root);
    selectedSha = prefs?.selectedSha ?? null;
    selectedInfo = null; // the effect below resolves it once commits land
    detailOpen = prefs?.detailOpen ?? false;
    restoreScroll = selectedSha !== null;
  });

  $effect(() => {
    store.start(repoId, { root });
    return () => store.destroy();
  });

  /**
   * Resolves a hydrated selection once its commit reaches the loaded log:
   * fills `selectedInfo` (the detail pane renders from it), mirrors a
   * click's multi-select bookkeeping, and scrolls to it once. The
   * resolve/pending/drop decision itself lives in the store
   * (`resolveRestoredSelection`, pure core in `history-logic`): a selection
   * that never appears is dropped when the stream exhausts, so no phantom
   * detail state lingers.
   */
  $effect(() => {
    const sha = selectedSha;
    if (!sha || selectedInfo) return;
    void store.flat; // re-run as pages release into the index
    const restored = store.resolveRestoredSelection(sha);
    if (restored.kind === "resolved") {
      const commit = commits[restored.index] ?? null;
      if (!commit) return;
      selectedInfo = commit;
      multiIdx = [restored.index];
      anchorIdx = restored.index;
      if (restoreScroll) {
        restoreScroll = false;
        sync.scrollToIndex(restored.index);
      }
      if (sidebar) onSelectCommit?.(commit, "restore");
    } else if (restored.kind === "dropped") {
      selectedSha = null;
      detailOpen = false;
      restoreScroll = false;
      // The center editor may still show the vanished commit's detail.
      if (sidebar) onSelectCommit?.(null);
    }
  });

  // Persist per-root history state on every change (debounced in the prefs
  // module); the store's restore writes identical filter data back — harmless.
  $effect(() => {
    if (root === "") return;
    const filter = { ...store.filter };
    saveHistoryPrefs(root, {
      filter,
      selectedSha,
      detailOpen: detailOpen && selectedSha !== null,
    });
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
    if (!sidebar && opts.open !== false) detailOpen = Boolean(selectedInfo);
    if (opts.scroll && idx >= 0) sync.scrollToIndex(idx);
    // Report explicitly as well as through the effect below: re-clicking the
    // already-selected row must reopen a detail the user closed in between
    // (the effect only re-runs when `selectedInfo` changes).
    if (sidebar) onSelectCommit?.(commit ?? null);
  }

  // Sidebar variant: selection is reported by `select()` only — a restored
  // selection (panel re-mounted after a collapse) updates the list highlight
  // without stealing the editor away from an open working-copy diff.

  // Sidebar variant: parent navigation from the center editor. The commit
  // may still be beyond the released window (or the stream may not have
  // reached it yet), so retry as pages land; a commit the active filter
  // excludes is dropped with the usual explanation.
  $effect(() => {
    if (!sidebar) return;
    const sha = revealSha;
    if (!sha) return;
    void store.flat;
    const outcome = store.resolveRestoredSelection(sha);
    if (outcome.kind === "resolved") {
      select(sha, { scroll: true });
      onRevealHandled?.();
    } else if (outcome.kind === "dropped") {
      onRevealHandled?.();
      toast("Parent commit is not in the loaded history");
    }
  });

  // Sidebar variant: expose the multi-selection action toolbar state. The
  // functions close over this view's live selection, so the center detail's
  // Cherry-pick / Revert / Rebase buttons keep operating on all selected
  // commits.
  // Sidebar variant: drop the center detail's action toolbar when this list
  // unmounts (history collapsed) so it never calls into a destroyed store.
  $effect(() => {
    if (!sidebar) return;
    return () => onActionsChange?.(null);
  });

  $effect(() => {
    if (!sidebar) return;
    const info = selectedInfo;
    if (!info) {
      onActionsChange?.(null);
      return;
    }
    // Read the reactive inputs so the effect re-runs when they change.
    void selectedShas;
    void opBusy;
    onActionsChange?.({
      count: selectedShas.length,
      busy: opBusy,
      cherryPick: () => void doCherryPick(),
      revert: () => void doRevert(),
      rebaseFromHere,
    });
  });

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

  // Bookmark state is lazy per root — hydrate outside derived reads (the
  // row context menu and CommitDetail's star read the store).
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

  // -- commit detail: diff/describe/blame live in CommitDetail ------------------

  function onCompare(files: FileDiff[], base: string, target: string): void {
    if (sidebar) {
      // The workspace renders the result in the center editor; this panel
      // stays a pure list (no detail pane of its own).
      onCompareResult?.({ files, base, target });
      return;
    }
    compare = { files, base, target };
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
        // The file list lives in CommitDetail — delegate to its exported
        // handler (null when the detail pane is closed).
        if (!detailRef?.toggleBlameFirstFile()) {
          toast("Select a commit with file changes to blame");
        }
      }),
      onUiEvent("history-select-commit", ({ sha, repoId: target }) => {
        if (target !== undefined && target !== repoId) return;
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

  /** Pops the selected commit's detail out into its own window. */
  function popOutDetail(): void {
    const sha = selectedInfo?.sha;
    if (!sha) return;
    void openPanelPopout(
      "commitdetail",
      repoId,
      `Commit ${sha.slice(0, 7)}`,
      { sha },
    );
  }
</script>

<section
  class="history flex h-full min-h-0 flex-col bg-card [&>.split]:flex-1 [&>.split]:min-h-0"
  class:sidebar
  aria-label="Commit history"
>
  {#if sidebar}
    <!-- 1a. sidebar header: the panel is a bare layout leaf, so it owns its
         own title row (full mode lives in a tab/popout and has one). -->
    <header class="sidebar-head flex flex-none items-center gap-1 border-b border-border bg-popover py-[0.3rem] pr-[0.4rem] pl-[0.6rem]">
      <span class="sh-title min-w-0 flex-1 truncate text-[0.6875rem] font-semibold uppercase tracking-[0.04em] text-muted-foreground">History</span>
      {#if onPopout}
        <Button
          class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label="Pop out history"
          title="Pop out history"
          onclick={onPopout}
        >
          <Icon icon={iconPopout} size={16} />
        </Button>
      {/if}
      {#if onCollapse}
        <Button
          class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label="Hide history sidebar"
          title="Hide the history sidebar"
          onclick={onCollapse}
        >
          <Icon icon={iconCollapse} size={16} />
        </Button>
      {/if}
    </header>
  {/if}

  <!-- 1b. filter bar (sidebar: search + a disclosure for the rest) -->
  <div
    class="filterbar flex flex-wrap items-center border-b border-border bg-popover text-xs {sidebar ? 'compact gap-1 px-1.5 py-[0.3rem]' : 'gap-1.5 px-3 py-1.5'}"
    class:compact={sidebar}
    role="search"
    aria-label="Filter commits"
  >
    <span class="relative inline-flex items-center text-muted-foreground {sidebar ? 'w-auto min-w-0 flex-1 basis-28' : 'w-36'}">
      <span class="absolute left-[0.45rem] inline-flex pointer-events-none opacity-75">
        <Icon icon={iconSearch} size={12} />
      </span>
      <Input
        class="f h-auto rounded-sm bg-card dark:bg-card px-[1.5rem] py-[0.2rem] text-xs md:text-xs placeholder:text-muted-foreground/80"
        bind:ref={filterTextEl}
        type="search"
        placeholder="Search commits"
        aria-label="Search commit text"
        value={store.filter.text}
        oninput={(e) => store.setFilter({ text: e.currentTarget.value })}
      />
    </span>

    {#if sidebar}
      <Button
        class="relative after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground {filterActive
          ? 'text-primary'
          : 'text-muted-foreground'} [&_svg:not([class*='size-'])]:size-4"
        variant="ghost"
        size="icon-xs"
        type="button"
        aria-label="More filters"
        aria-pressed={showAdvanced}
        aria-expanded={showAdvanced}
        title="More filters"
        onclick={() => (showAdvanced = !showAdvanced)}
      >
        <Icon icon={iconFilters} size={16} />
      </Button>
    {/if}

    {#if !sidebar || showAdvanced}
      <Input
        class="f h-auto w-36 rounded-sm bg-card dark:bg-card px-[0.45rem] py-[0.2rem] text-xs md:text-xs placeholder:text-muted-foreground/80"
        placeholder="Author"
        aria-label="Filter by author"
        value={store.filter.author}
        oninput={(e) => store.setFilter({ author: e.currentTarget.value })}
      />
      <Input
        class="f h-auto w-48 rounded-sm bg-card dark:bg-card px-[0.45rem] py-[0.2rem] text-xs md:text-xs placeholder:text-muted-foreground/80"
        placeholder="Path"
        aria-label="Filter by path"
        value={store.filter.path}
        oninput={(e) => store.setFilter({ path: e.currentTarget.value })}
      />
      <label class="date flex items-center gap-1 text-muted-foreground">
        <span class="dl">after</span>
        <Input
          type="date"
          class="h-auto rounded-sm bg-card dark:bg-card px-[0.3rem] py-[0.15rem] text-[0.6875rem] md:text-[0.6875rem] placeholder:text-muted-foreground/80"
          aria-label="Commits after this date"
          value={store.filter.after}
          onchange={(e) => store.setFilter({ after: e.currentTarget.value })}
        />
      </label>
      <label class="date flex items-center gap-1 text-muted-foreground">
        <span class="dl">before</span>
        <Input
          type="date"
          class="h-auto rounded-sm bg-card dark:bg-card px-[0.3rem] py-[0.15rem] text-[0.6875rem] md:text-[0.6875rem] placeholder:text-muted-foreground/80"
          aria-label="Commits before this date"
          value={store.filter.before}
          onchange={(e) => store.setFilter({ before: e.currentTarget.value })}
        />
      </label>
      <!-- M9: regex mode is live (backend `LogFilter.regex`). -->
      <label class="regex flex items-center gap-1 text-muted-foreground" title="Treat the search text as a regular expression">
        <Checkbox
          checked={store.filter.regex}
          onCheckedChange={(checked) => store.setFilter({ regex: checked === true })}
          aria-label="Treat the search text as a regular expression"
        />
        <span>Regex</span>
      </label>
      <!-- M10: pickaxe -S — commits whose patch adds/removes the string. -->
      <Input
        class="f h-auto w-36 rounded-sm bg-card dark:bg-card px-[0.45rem] py-[0.2rem] text-xs md:text-xs placeholder:text-muted-foreground/80"
        placeholder="Changes containing… (-S)"
        aria-label="Pickaxe: patches adding or removing this string"
        value={store.filter.pickaxe}
        oninput={(e) => store.setFilter({ pickaxe: e.currentTarget.value })}
      />
      <!-- M12: pickaxe -G — commits whose patch adds/removes lines matching
           this regex. -S and -G may BOTH be set: the values are forwarded
           verbatim and the backend intersects them (simplest contract — no
           cross-clearing UI to get out of sync). -->
      <Input
        class="f h-auto w-36 rounded-sm bg-card dark:bg-card px-[0.45rem] py-[0.2rem] text-xs md:text-xs placeholder:text-muted-foreground/80"
        placeholder="patch regex (-G)"
        aria-label="Pickaxe: patches adding or removing lines matching this regex"
        value={store.filter.pickaxeRegex ?? ""}
        oninput={(e) => store.setFilter({ pickaxeRegex: e.currentTarget.value })}
      />
    {/if}

    {#if filterActive}
      <Button
        class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
        variant="ghost"
        size="icon-xs"
        type="button"
        aria-label="Clear filters"
        title="Clear all filters"
        onclick={() => store.clearFilter()}
      >
        <Icon icon={iconClearFilters} size={16} />
      </Button>
    {/if}

    <Button
      class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
      variant="ghost"
      size="icon-xs"
      type="button"
      aria-label="Compare commits"
      aria-pressed={showCompare}
      aria-expanded={showCompare}
      title="Compare commits"
      onclick={() => (showCompare = !showCompare)}
    >
      <Icon icon={iconCompare} size={16} />
    </Button>

    {#if onPopout && !sidebar}
      <!-- M4 F1: pop the history out into its own window (desktop only). -->
      <Button
        class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
        variant="ghost"
        size="icon-xs"
        type="button"
        aria-label="Pop out history"
        title="Pop out history into its own window"
        onclick={onPopout}
      >
        <Icon icon={iconPopout} size={16} />
      </Button>
    {/if}

    <span
      class="status flex items-center gap-1.5 text-[0.6875rem] text-muted-foreground {sidebar ? 'basis-full' : 'ml-auto'}"
      role="status"
      aria-live="polite"
    >
      {#if store.error}
        <span class="err text-destructive" role="alert">{store.error}</span>
        <Button
          class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label="Retry loading history"
          title="Retry loading history"
          onclick={() => store.restart()}
        >
          <Icon icon={iconRetry} size={16} />
        </Button>
      {:else if store.loading}
        <Spinner class="size-[11px]" role="presentation" aria-hidden="true" />
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
  <div class="listwrap relative flex min-h-0 flex-1">
    {#if store.loading && total === 0}
      <p class="empty m-auto text-[0.8125rem] text-muted-foreground" role="status">
        <span class="empty-loading inline-flex items-center gap-[0.4rem]">
          <Spinner class="size-[14px]" role="presentation" aria-hidden="true" />
          Loading history…
        </span>
      </p>
    {:else if !store.loading && !store.error && total === 0}
      <p class="empty m-auto text-[0.8125rem] text-muted-foreground">No commits match the current filters.</p>
    {:else}
      <div
        class="scroller min-w-0 flex-1 overflow-y-auto focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring {sidebar
          ? ""
          : "[scrollbar-width:none] [&::-webkit-scrollbar]:hidden"}"
        bind:this={scrollerEl}
        onscroll={onScroll}
        role="listbox"
        aria-label="Commits"
        tabindex="0"
        aria-activedescendant={activeDescendant}
        onkeydown={onKeydown}
      >
        <div class="spacer relative" style:height={`${total * ROW_HEIGHT}px`}>
          {#each visible as v (v.commit.sha)}
            <div
              id={`commit-row-${v.idx}`}
              class="row absolute inset-x-0 flex cursor-pointer items-center border-b border-border/45 py-0 text-[0.8125rem] {sidebar ? "gap-1 pr-[0.45rem] pl-[0.55rem]" : "gap-1.5 pr-[0.6rem] pl-[0.75rem]"} {v.idx === selectedIdx
                ? "bg-secondary text-secondary-foreground"
                : multiIdx.includes(v.idx)
                  ? "bg-secondary/45 hover:bg-secondary/60"
                  : "hover:bg-accent"}"
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
              <span class="sha flex-none font-mono {v.idx === selectedIdx ? "" : "text-muted-foreground"}">{v.commit.sha.slice(0, 7)}</span>
              {#each v.commit.refs.slice(0, 3) as ref, i (`${v.commit.sha}:${i}:${ref}`)}
                {@const kind = classifyRef(ref)}
                {@const scope =
                  kind === "local"
                    ? "Local"
                    : kind === "remote"
                      ? "Remote"
                      : kind === "tag"
                        ? "Tag"
                        : "HEAD"}
                {@const refTint =
                  kind === "local"
                    ? "border-transparent bg-primary/10 text-primary"
                    : kind === "remote"
                      ? "border-border text-muted-foreground"
                      : kind === "tag"
                        ? "border-transparent bg-accent text-accent-foreground"
                        : "border-primary bg-primary/10 text-primary"}
                <span
                  class="ref ref-{kind} inline-flex flex-none items-center gap-1 whitespace-nowrap rounded-sm border px-[0.35rem] text-[0.6875rem] leading-[1.4] {sidebar ? 'max-w-24' : 'max-w-36'} {refTint}"
                  title={`${scope}: ${shortRefName(ref)}`}
                  aria-label={`${scope}: ${shortRefName(ref)}`}
                >
                  <span class="ref-scope flex-none text-[0.55rem] font-bold uppercase tracking-[0.025em] opacity-80 {sidebar ? 'hidden' : ''}" aria-hidden="true">{scope}</span>
                  <span class="ref-name min-w-0 truncate">{shortRefName(ref)}</span>
                </span>
              {/each}
              {#if v.commit.refs.length > 3}
                <span
                  class="ref ref-more inline-flex flex-none items-center whitespace-nowrap rounded-sm border border-transparent bg-muted px-[0.35rem] text-[0.6875rem] leading-[1.4] tabular-nums text-muted-foreground"
                  title={v.commit.refs.slice(3).map(shortRefName).join(", ")}
                  aria-label={`${v.commit.refs.length - 3} more refs`}
                >
                  +{v.commit.refs.length - 3}
                </span>
              {/if}
              <span class="summary min-w-0 flex-1 truncate" title={v.commit.summary}>{v.commit.summary}</span>
              <span class="author max-w-40 flex-none truncate {v.idx === selectedIdx ? "" : "text-muted-foreground"} {sidebar ? "hidden" : ""}">{v.commit.author.name}</span>
              <span class="date flex-none text-muted-foreground {sidebar ? "text-[0.65rem]" : "text-[0.6875rem]"}">{formatRelativeTime(v.commit.author.time)}</span>
            </div>
          {/each}
        </div>
      </div>
      {#if !sidebar}
        <div class="graphcol h-full min-h-0 flex-none border-l border-border bg-card" style:width={`${GRAPH_WIDTH}px`}>
          <GraphCanvas
            pages={store.pages}
            rowHeight={ROW_HEIGHT}
            onCommitClick={(sha) => select(sha)}
            onReachEnd={() => store.loadMore()}
            onVisibleRowsChange={sync.onCanvasVisible}
            {bookmarks}
            {branchColors}
            bind:scrollEl={canvasScrollEl}
            bind:this={canvasRef}
          />
        </div>
      {/if}
    {/if}
  </div>
  {/snippet}

  {#if sidebar}
    <!-- 3. sidebar: list only — the selected commit opens in the workspace's
         center editor (variant `full` keeps the inline detail below). -->
    {@render logPane()}
  {:else if detailOpen && (selectedInfo || compare)}
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
        <CommitDetail
          bind:this={detailRef}
          {repoId}
          info={selectedInfo}
          {compare}
          {root}
          actionCount={selectedShas.length}
          actionBusy={opBusy}
          onClose={() => (detailOpen = false)}
          onCloseCompare={() => (compare = null)}
          onSelectParent={selectParent}
          onCherryPick={() => void doCherryPick()}
          onRevert={() => void doRevert()}
          onRebaseFromHere={rebaseFromHere}
          onPopout={selectedInfo ? popOutDetail : undefined}
        />
      {/snippet}
    </SplitPane>
  {:else}
    {@render logPane()}
    {#if selectedInfo}
      <Button
        variant="ghost"
        class="detail-open flex-none h-auto w-full justify-start truncate rounded-none border-t border-border bg-popover px-3 py-1 text-left text-[0.72rem] font-normal text-primary hover:bg-accent hover:text-primary focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring dark:hover:bg-accent"
        onclick={() => (detailOpen = true)}
        aria-expanded="false"
      >
        Show commit detail — {selectedInfo.summary}
      </Button>
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
