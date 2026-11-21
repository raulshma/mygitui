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
 * (`historyPrefs`). The branch-scope dropdown's feed comes from the shared
 * refs cache (`$lib/stores/refs.svelte`) and follows external ref moves
 * through the debounced repo-change effect below (the log restarts via the
 * store's own subscription).
   *
   * Keyboard: the list is a role="listbox" — j/k or arrows move the
   * selection (g/G/Home/End jump), and aria-activedescendant tracks it.
   */
  import { untrack } from "svelte";
  import {
    archiveSpec,
    branchCreate,
    branchSwitch,
    cherryPick,
    commitCheckout,
    commitMessage,
    describe,
    guardCheckpoint,
    pickSaveFile,
    repoDiff,
    repoRefs,
    revertCommits,
    stashPush,
    tagCreate,
  } from "$lib/ipc/client";
  import type { BranchInfo, CommitInfo, DiffSide, FileDiff, MergeResult } from "$lib/ipc/types";
  import { closedDialog, openDialog, type DialogState } from "$lib/state/dialog";
  import { describeError } from "$lib/errors";
  import { isSignificant, onRepoChange } from "$lib/events/repoEvents";
  import { branchesFor, invalidateRefs } from "$lib/stores/refs.svelte";
  import GraphCanvas from "$lib/components/graph/GraphCanvas.svelte";
  import EmptyState from "../layout/EmptyState.svelte";
  import CompareBar from "$lib/components/panels/CompareBar.svelte";
  import type { CompareResult } from "$lib/components/panels/compare";
  import CommitDetail from "$lib/components/panels/CommitDetail.svelte";
  import RebasePlanner from "$lib/components/rebase/RebasePlanner.svelte";
  import { orderForCherryPick } from "$lib/components/rebase/plannerModel";
  import { HistoryStore } from "$lib/stores/history.svelte";
  import { createHistoryScrollSync } from "$lib/stores/historyScrollSync";
  import { parsePathsFilter } from "$lib/stores/history-logic";
  import { tabStore } from "$lib/stores/tabs.svelte";
  import {
    loadHistoryPrefs,
    saveHistoryPrefs,
  } from "$lib/stores/historyPrefs";
  import { bookmarks as bookmarkStore } from "$lib/components/graph/bookmarks.svelte";
  import { onUiEvent, emitUiEvent } from "$lib/palette/events";
  import { ai } from "$lib/ai/ai.svelte";
  import { budgetedDiffText } from "$lib/ai/diffBudget";
  import { aiResultMeta } from "$lib/ai/usage";
  import {
    contextMenu,
    showMenuAt,
    type MenuEntry,
  } from "$lib/components/menu/contextMenuStore.svelte";
  import { openPanelPopout } from "$lib/layout/popout";
  import SplitPane from "$lib/components/layout/SplitPane.svelte";
  import {
    readSplitModeOpen,
    readSplitRatio,
    writeSplitModeOpen,
    writeSplitRatio,
  } from "$lib/layout/splitPrefs";
  import { widthMode } from "$lib/layout/widthMode.svelte";
  import { createWidthTracker } from "$lib/layout/containerWidth.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import ResetDialog from "$lib/components/safety/ResetDialog.svelte";
  import TagAtDialog from "$lib/components/panels/TagAtDialog.svelte";
  import type { HistoryCommitActions } from "$lib/components/panels/historySidebar";
  import {
    classifyRef,
    formatDateTime,
    formatRelativeTime,
    isCheckpointSummary,
    shortRefName,
  } from "$lib/git/format";
  import { toast } from "$lib/toast";
  import { copyText } from "$lib/clipboard";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import AuthorAvatar from "$lib/components/panels/AuthorAvatar.svelte";
  import RefBadge from "$lib/components/panels/RefBadge.svelte";
  import { floating } from "$lib/components/overlay/floating";

  const iconSearch = $derived(appIcon("search")!);
  const iconPopout = $derived(appIcon("open-in-new")!);
  const iconCollapse = $derived(appIcon("right-panel-close")!);
  const iconFilters = $derived(appIcon("filter-list")!);
  const iconCompare = $derived(appIcon("compare-arrows")!);
  const iconClose = $derived(appIcon("close")!);
  const iconRetry = $derived(appIcon("restart-alt")!);
  const iconCheckout = $derived(appIcon("alt-route")!);
  const iconBranch = $derived(appIcon("fork-right")!);
  const iconGraph = $derived(appIcon("account-tree")!);

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
  const OVERSCAN = 10;

  // -- graph column (plan §8.3) ---------------------------------------------------

  /**
   * The graph column's persisted prefs key: a `splitPrefs` slot shared by
   * the list↔graph ratio and the per-width-mode collapsed flag (one key,
   * both dimensions — the envelope holds `ratio` + `modes`).
   */
  const GRAPH_PREFS_KEY = "history-graph";
  /** Container width below which the graph auto-collapses (plan §8.3). */
  const GRAPH_AUTO_COLLAPSE_PX = 560;
  /**
   * Graph width as the LIST pane's fraction: clamped tighter than the
   * global splitter range so the graph stays a column (12–45% of the row),
   * never a co-equal half. Default ≈ the old fixed 220px at typical panel
   * widths (list 78% / graph 22%).
   */
  const GRAPH_RATIO_DEFAULT = 0.78;
  const GRAPH_RATIO_MIN = 0.55;
  const GRAPH_RATIO_MAX = 0.88;

  /** List pane fraction (the graph takes the rest), persisted per app. */
  let graphRatio = $state(readSplitRatio(GRAPH_PREFS_KEY, GRAPH_RATIO_DEFAULT));

  function setGraphRatio(raw: number): void {
    const clamped = Math.min(GRAPH_RATIO_MAX, Math.max(GRAPH_RATIO_MIN, raw));
    graphRatio = clamped;
    writeSplitRatio(GRAPH_PREFS_KEY, clamped);
  }

  /**
   * User collapse state, persisted per width mode (the §4.2 overlay-state
   * pattern — the graph is an overlay-ish visibility, so mode-scoped like
   * the drawers). `false` default = visible.
   */
  let graphCollapsed = $state(
    readSplitModeOpen(GRAPH_PREFS_KEY, widthMode.current, false),
  );

  function toggleGraph(): void {
    graphCollapsed = !graphCollapsed;
    writeSplitModeOpen(GRAPH_PREFS_KEY, widthMode.current, graphCollapsed);
  }

  $effect(() => {
    // The collapse state is stored per width mode: when the window mode
    // changes, restore THAT mode's choice (the toggle above writes the
    // current mode's slot; this read-only re-arm makes the stored value
    // follow the mode ladder).
    const mode = widthMode.current;
    untrack(() => {
      graphCollapsed = readSplitModeOpen(GRAPH_PREFS_KEY, mode, false);
    });
  });

  /**
   * The panel's own width tracker: the graph auto-collapses when the
   * history container (not the window) is too narrow to show a column
   * beside the list. Bound to the section root below.
   */
  const historyWidth = createWidthTracker(GRAPH_AUTO_COLLAPSE_PX);

  /** Effective visibility: user toggle AND the container-width floor. */
  const graphVisible = $derived(!graphCollapsed && !historyWidth.narrow);

  // Capturing the initial `root` is intended: the prop is fixed per mounting
  // site (RepoView passes the worktree root; popouts pass nothing), so the
  // stream slot never changes for a live view.
  // svelte-ignore state_referenced_locally
  const store = new HistoryStore({
    // Per-consumer stream slot: the popout window (root "") gets its own so
    // it never cancels the workspace view's walk (and vice versa).
    consumerId: root === "" ? "popout-history" : "history",
  });

  // -- selection + detail state ------------------------------------------------

  let selectedSha = $state<string | null>(null);
  /** Last known CommitInfo for the selection (survives filter restarts). */
  let selectedInfo = $state<CommitInfo | null>(null);
  let detailOpen = $state(false);
  let showCompare = $state(false);
  let compare = $state<CompareResult | null>(null);

  /** Live `CommitDetail` instance (palette blame toggle; null when closed). */
  let detailRef = $state<ReturnType<typeof CommitDetail> | null>(null);

  // -- filter popover (plan §8.4: ONE search input + ONE filter menu) ----------

  /**
   * The advanced filters (author / path / date range / ref scope / regex /
   * pickaxe) collapsed into one icon-triggered popover. The text search
   * stays an always-visible input beside the icon; state and semantics are
   * the store's (`store.filter`), untouched by this consolidation.
   */
  let filtersOpen = $state(false);
  let filtersAnchor = $state<HTMLElement | null>(null);
  let filtersEl = $state<HTMLElement | null>(null);

  function toggleFilters(): void {
    if (filtersOpen) {
      filtersOpen = false;
      filtersAnchor?.focus();
    } else {
      filtersOpen = true;
    }
  }

  /** Active advanced-filter count (the icon's badge): every non-empty
   *  field plus the regex flag; the text search is the bar's own input. */
  const advancedFilterCount = $derived.by(() => {
    const f = store.filter;
    return (
      [
        f.author,
        f.path,
        f.after,
        f.before,
        f.pickaxe,
        f.pickaxeRegex ?? "",
        f.scopeRef ?? "",
      ].filter((v) => v !== "").length + (f.regex ? 1 : 0)
    );
  });

  // The popover is portaled to <body> (BranchSwitcher idiom): outside-click
  // closes, Escape closes and restores the trigger's focus.
  $effect(() => {
    if (!filtersOpen) return;
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target as Node | null;
      if (filtersEl?.contains(target) || filtersAnchor?.contains(target)) return;
      filtersOpen = false;
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.stopPropagation();
        filtersOpen = false;
        filtersAnchor?.focus();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  });

  // -- refs popover (plan §8.2: `+n` chip lists ALL refs w/ copy/checkout) ------

  /**
   * The open refs popover: the commit whose refs are listed plus the `+n`
   * chip anchoring it (null = closed). One at a time; rows offer Copy for
   * every ref and checkout through the commit's existing confirm dialog.
   */
  let refsPopover = $state<{ sha: string; refs: string[] } | null>(null);
  let refsAnchor = $state<HTMLElement | null>(null);
  let refsEl = $state<HTMLElement | null>(null);

  function openRefsPopover(sha: string, refs: string[], anchor: HTMLElement): void {
    refsPopover = { sha, refs };
    refsAnchor = anchor;
  }

  function closeRefsPopover(): void {
    refsPopover = null;
    refsAnchor?.focus();
  }

  // Same outside/Esc contract as the filter popover above.
  $effect(() => {
    if (refsPopover === null) return;
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target as Node | null;
      if (refsEl?.contains(target) || refsAnchor?.contains(target)) return;
      refsPopover = null;
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.stopPropagation();
        refsPopover = null;
        refsAnchor?.focus();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  });

  /** Copies a ref's display name (the refs popover row action). */
  async function copyRef(ref: string): Promise<void> {
    await copyText(shortRefName(ref), "branch name");
  }

  /**
   * Checkout through the row's existing confirm dialog, targeting THIS ref
   * when it names a local branch (switch to that branch — the dialog
   * resolves stash/dirty choices as usual) and the commit otherwise
   * (detach at sha, like the hover action).
   */
  function checkoutRef(sha: string, ref: string): void {
    const branch = classifyRef(ref) === "local" ? shortRefName(ref) : undefined;
    requestCheckout(sha, branch);
  }

  // -- resizable panes (SplitPane ratios, persisted as global prefs) --------

  /** Commit list fraction of the history panel (detail takes the rest). */
  const DETAIL_RATIO_KEY = "history-detail";
  let detailRatio = $state(readSplitRatio(DETAIL_RATIO_KEY, 0.62));

  function setDetailRatio(ratio: number): void {
    detailRatio = ratio;
    writeSplitRatio(DETAIL_RATIO_KEY, ratio);
  }

  // -- commit actions (M3 E2) -------------------------------------------------

  /**
   * Multi-selection as SHAS, list order (cherry-pick/revert input) — a
   * selection-owned machine like the single select: the pick names
   * commits, not rows, so a stream restart can never repoint it at rows
   * the user did not click (registry #11 — the old index-based list only
   * survived on an epoch-mirror effect faithfully clearing it).
   */
  let multiShas = $state<string[]>([]);
  /** Membership view of `multiShas` — the row template asks 3× per visible row. */
  const multiSet = $derived(new Set(multiShas));
  /** Anchor commit for shift-click ranges (last plain click). */
  let anchorSha: string | null = null;
  /** True while a cherry-pick/revert/checkout/archive IPC is in flight. */
  let opBusy = $state(false);
  /** Interactive rebase planner for `HEAD..baseSha` (one state, args ride along). */
  let planner = $state<DialogState<{ baseSha: string }>>(closedDialog());

  // M12 commit-menu dialogs — one DialogState each; the sha travels with
  // open, so no open/close site can leave a stale sha behind.
  /** "Branch from here…" PromptDialog. */
  let branchFrom = $state<DialogState<{ sha: string }>>(closedDialog());
  /** "Tag here…" TagAtDialog. */
  let tagAt = $state<DialogState<{ sha: string }>>(closedDialog());
  /** "Reset current branch to here…" ResetDialog. */
  let resetTo = $state<DialogState<{ sha: string }>>(closedDialog());
  /**
   * "Checkout commit" ConfirmDialog (row hover action, context menu, refs
   * popover). The sha travels with open like the dialogs above; the
   * summary only feeds the dialog text and may degrade to "" after a
   * restart. `branch` pins the switch target at open time — the refs
   * popover checks out a SPECIFIC local branch — and undefined resolves
   * to the commit's first local branch at open (hover-action semantics).
   */
  let checkoutAt = $state<
    DialogState<{ sha: string; summary: string; branch?: string }>
  >(closedDialog());

  /** HEAD sha from the tab status (null when unknown/unloaded). */
  const headSha = $derived(
    tabStore.tabs.find((t) => t.id === repoId)?.status?.head ?? null,
  );
  /** True when the worktree has entries (dirty) — checkout offers stash. */
  const worktreeDirty = $derived(
    (tabStore.tabs.find((t) => t.id === repoId)?.status?.entries.length ?? 0) > 0,
  );

  /**
   * A local branch pointing at `commit` (branch checkouts switch instead of
   * detaching, like VSCode): the first local branch decoration, where the
   * "HEAD -> main" label contributes its `main`.
   */
  function localBranchAt(commit: CommitInfo): string | null {
    for (const ref of commit.refs) {
      if (ref === "HEAD") continue;
      const name = ref.startsWith("HEAD -> ") ? ref.slice("HEAD -> ".length) : ref;
      if (classifyRef(name) === "local") return name;
    }
    return null;
  }

  /**
   * Opens the checkout confirmation for `sha` (hover action / menu entry /
   * refs popover). Without `branch`, the commit's first local branch is
   * resolved at open time (switch instead of detach — VSCode parity).
   */
  function requestCheckout(sha: string, branch?: string): void {
    const row = commits.find((c) => c.sha === sha);
    checkoutAt = openDialog({
      sha,
      summary: row?.summary ?? "",
      branch: branch ?? (row ? (localBranchAt(row) ?? undefined) : undefined),
    });
  }

  /**
   * ConfirmDialog result: optionally stash first, snapshot a guard
   * checkpoint (the worktree is forced), then branch-switch or detach.
   * The history restarts via repo-changed; status refresh renders the
   * "detached @ sha" state. Rides `opBusy` like its siblings (cherry-pick/
   * revert/archive): a long forced checkout keeps the detail toolbar and
   * the commit menu's op entries disabled instead of letting a second op
   * queue behind it.
   */
  async function runCheckout(sha: string, branch: string | null, stashFirst: boolean): Promise<void> {
    if (opBusy) return;
    checkoutAt = closedDialog();
    opBusy = true;
    try {
      if (stashFirst) {
        await stashPush(repoId, `pre-checkout stash (checkout ${sha.slice(0, 7)})`);
      }
      await guardCheckpoint(repoId, "pre-checkout");
      if (branch) {
        await branchSwitch(repoId, branch, stashFirst ? false : true);
        toast(`Switched to ${branch}`, { kind: "success" });
      } else {
        await commitCheckout(repoId, sha);
        toast(`Checked out ${sha.slice(0, 7)} — detached HEAD`, { kind: "success" });
      }
    } catch (err) {
      toast(
        `Checkout failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      opBusy = false;
    }
    void tabStore.refreshStatus(repoId);
  }

  // -- virtualizer + canvas scroll sync ------------------------------------------

  let scrollerEl = $state<HTMLDivElement | undefined>(undefined);
  /**
   * The view's section root. ResizeObserver watches it in addition to the
   * scroller: SplitContainer (collapsed sidebar) and TabGroup (keep-alive
   * tab host) hide this panel with `hidden` on an ANCESTOR, and an unhide
   * must re-window the virtualizer or the list stays empty.
   */
  let panelEl = $state<HTMLElement | undefined>(undefined);
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

  /**
   * The last user-scrolled position as the top row's commit sha + scrollTop —
   * the anchor a stream restart restores the viewport from (see
   * `pendingScrollRestore`). Captured at scroll time because `#beginStream`
   * wipes the pages before the restart effect runs: reading them there would
   * see an empty log. The `total` guard skips the scroll event a restart's
   * collapsed spacer fires (scrollTop clamping to 0), which would otherwise
   * overwrite the anchor with the wiped log.
   */
  let lastScroll = { sha: null as string | null, top: 0 };

  function onScroll(): void {
    if (total > 0) {
      const top = scrollerEl?.scrollTop ?? 0;
      lastScroll = {
        sha: commits[Math.floor(top / ROW_HEIGHT)]?.sha ?? null,
        top,
      };
    }
    if (sync.onListScroll().end >= total) store.loadMore();
  }

  $effect(() => {
    // Splitter drags resize the scroller without a scroll event: re-window
    // the virtualizer as the box changes (guarded — ResizeObserver is
    // absent in some test environments). The section root is observed too:
    // the panel is hidden by `hidden` on an ANCESTOR (SplitContainer
    // collapse / TabGroup keep-alive host), so the unhide is a 0→N box
    // change only visible on the section — without it a restart that ran
    // while the panel was hidden would leave an empty list.
    const scroller = scrollerEl;
    const panel = panelEl;
    if ((!scroller && !panel) || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => sync.updateOwnRange());
    if (scroller) observer.observe(scroller);
    if (panel) observer.observe(panel);
    return () => observer.disconnect();
  });

  $effect(() => {
    // Fill the viewport when data first lands (or grows past the window).
    void total;
    sync.updateOwnRange();
    if (sync.renderWindow().end >= total && total > 0) store.loadMore();
  });

  $effect(() => {
    // Graph collapse (user toggle or the <560px container floor): the
    // canvas unmounts, so its last reported window would go stale —
    // renderWindow() prefers it over the local window and a scrolled list
    // would keep rendering the pre-collapse rows. Drop it and re-window
    // from the list scroller (the same fallback the sidebar variant always
    // runs); on re-expand the canvas remounts and reports a fresh window.
    // The bound scroll element / handle are cleared explicitly too: the
    // sync's ports select strategies by their ABSENCE, and a detached
    // leftover element would be mistaken for a live fast path.
    const hidden = !graphVisible;
    if (!hidden) return;
    canvasWindow = null;
    untrack(() => {
      canvasScrollEl = undefined;
      canvasRef = null;
      sync.updateOwnRange();
    });
  });

  $effect(() => {
    // Feed the panel root to the width tracker (one bind:this per element —
    // the section root is already bound to `panelEl` for the virtualizer's
    // ResizeObserver above).
    historyWidth.el = panelEl ?? undefined;
  });

  $effect(() => {
    // Graph (re)mount — expand from a collapse, or the first paint of the
    // full variant: seed the canvas's scroll offset from the list BEFORE
    // the canvas reports its drawn window. A fresh canvas sits at scrollTop
    // 0; unseeded, `sync.onCanvasVisible` would see list≠canvas and drag a
    // deeply scrolled list back to the top (§14's scroll-sync risk).
    // Deliberately NOT re-run on scroll: scrollTop writes are DOM-only and
    // untracked, so direct canvas scrolling never fights this.
    const canvas = canvasScrollEl;
    const list = scrollerEl;
    if (!canvas || !list) return;
    if (canvas.scrollTop !== list.scrollTop) canvas.scrollTop = list.scrollTop;
  });

  // -- lifecycle ------------------------------------------------------------------

  /** Scroll to the restored selection once its commit lands in the log. */
  let restoreScroll = false;

  /**
   * Armed by the restart effect with the pre-restart top row's commit (see
   * `lastScroll`); the effect below jumps back to it once the re-walked log
   * delivers that commit. Null when there is nothing to restore.
   */
  let pendingScrollRestore = $state<{ sha: string } | null>(null);

  /**
   * Parked `history-select-commit` request whose commit has not reached the
   * loaded log yet ("Show in History" from BranchPanel/StashPanel). It rides
   * the same restore machine as a persisted selection: buffered pages release
   * while the commit can still arrive; once the stream exhausts without it,
   * the request is dropped — with an info toast when the sender asked for
   * one (the sha may sit outside the restored filter's window).
   */
  let pendingReveal = $state<{ sha: string; toastOnMiss: boolean } | null>(null);

  // -- jump-to-SHA (palette `history.jump-to-sha`) ------------------------------
  //
  // A small inline input (not the QuickOpen component — this is one field
  // with one resolution rule): accepts a sha / short sha / branch / tag.
  // Success routes through the same `history-select-commit` machine as the
  // cross-panel reveals (parked retry while the stream delivers; a miss
  // with toastOnMiss is ONE info toast). Failures surface INLINE — never
  // as a toast storm.

  let jumpOpen = $state(false);
  let jumpError = $state<string | null>(null);
  let jumpInputEl = $state<HTMLInputElement | null>(null);
  /** Full ref list ([name, sha] pairs), fetched once per open. */
  let jumpRefs: Array<[string, string]> | null = null;

  function openJump(): void {
    jumpOpen = true;
    jumpError = null;
    jumpRefs = null;
    requestAnimationFrame(() => jumpInputEl?.focus());
  }

  function closeJump(): void {
    jumpOpen = false;
    jumpError = null;
    jumpRefs = null;
  }

  /** True when `value` looks like a (full or short) hex sha. */
  function isShaLike(value: string): boolean {
    return /^[0-9a-f]{4,40}$/i.test(value);
  }

  /** Selects a resolved commit through the parked-reveal machine + closes. */
  function revealJumpSha(sha: string): void {
    jumpOpen = false;
    jumpError = null;
    jumpRefs = null;
    if (store.revealSha(sha)) {
      select(sha, { scroll: true });
    } else {
      // Parked: selects when the stream delivers the commit; a miss costs
      // one info toast (the sha may sit outside the loaded window).
      pendingReveal = { sha, toastOnMiss: true };
    }
  }

  /**
   * Resolution order: exact loaded sha → unique short-sha prefix among the
   * loaded commits → ref name (branch/tag, short or fully qualified) →
   * a 40-char sha the stream has not reached yet (parked) → inline error.
   */
  async function jumpSubmit(): Promise<void> {
    const raw = (jumpInputEl?.value ?? "").trim();
    if (raw === "") {
      jumpError = "Enter a sha, branch, or tag";
      return;
    }
    const needle = raw.toLowerCase();
    const exact = commits.find((c) => c.sha === needle);
    if (exact) {
      revealJumpSha(exact.sha);
      return;
    }
    if (isShaLike(needle)) {
      const prefixMatches = commits.filter((c) => c.sha.startsWith(needle));
      if (prefixMatches.length === 1) {
        revealJumpSha(prefixMatches[0].sha);
        return;
      }
      if (prefixMatches.length > 1) {
        jumpError = `${prefixMatches.length} commits share this short sha — enter more characters`;
        return;
      }
    }
    // Refs: branches and tags by short or fully qualified name (cached per
    // open — the list is small and the jump bar is transient).
    if (jumpRefs === null) {
      try {
        jumpRefs = await repoRefs(repoId);
      } catch {
        jumpRefs = []; // degrade: sha resolution still works
      }
    }
    const wanted = needle;
    const refHit =
      jumpRefs.find(([name]) => name.toLowerCase() === wanted) ??
      jumpRefs.find(
        ([name]) =>
          name.toLowerCase() === `refs/heads/${wanted}` ||
          name.toLowerCase() === `refs/tags/${wanted}` ||
          name.toLowerCase() === `refs/remotes/${wanted}`,
      );
    if (refHit) {
      revealJumpSha(refHit[1]);
      return;
    }
    // A full sha the stream has not delivered yet: park it on the same
    // reveal machine (it selects on delivery, one info toast on exhaust).
    if (needle.length === 40 && isShaLike(needle)) {
      revealJumpSha(needle);
      return;
    }
    jumpError = `No commit, branch, or tag matches “${raw}”`;
  }

  function jumpKeydown(event: KeyboardEvent): void {
    if (event.key === "Enter") {
      event.preventDefault();
      void jumpSubmit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeJump();
    } else if (jumpError !== null) {
      jumpError = null;
    }
  }

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
        multiShas = [];
        anchorSha = null;
        planner = closedDialog();
        branchFrom = closedDialog();
        tagAt = closedDialog();
        resetTo = closedDialog();
        checkoutAt = closedDialog();
        pendingReveal = null;
        // A different repo's scroll anchor is meaningless: drop it before
        // the restart effect below arms a restore from it (same flush,
        // declaration order).
        lastScroll = { sha: null, top: 0 };
        pendingScrollRestore = null;
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

  $effect(() => {
    // Reset the scroll windows on EVERY stream restart, not only repo
    // re-targets: filter edits, watcher-driven refreshes and manual retries
    // all bump the store's epoch in #beginStream, and a render window
    // indexes into ONE stream's rows — inheriting the previous stream's
    // window (or its scrollTop) renders nothing until the first scroll
    // event. The repo re-target effect above resets too (root-only
    // re-targets keep the same repoId, so no epoch bump there); a persisted
    // selection re-resolves and re-scrolls as its commit lands (the
    // restore effects below). Untracked reset: the ports read the bound
    // elements, which must not enter this effect's dependencies.
    //
    // The reset also collapses the spacer (the store wiped its pages), so
    // the browser clamps scrollTop to 0 — arm the viewport restore from the
    // pre-restart anchor (`lastScroll`, captured at scroll time) so a
    // background refresh lands the user back where they were instead of at
    // the top. At the very top there is nothing to preserve, and a pending
    // persisted selection owns the scroll instead.
    void store.restartEpoch;
    untrack(() => {
      sync.reset();
      pendingScrollRestore =
        lastScroll.sha !== null && lastScroll.top > 0 && !restoreScroll
          ? { sha: lastScroll.sha }
          : null;
    });
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
      multiShas = [sha];
      anchorSha = sha;
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

  /**
   * Restores the viewport across a stream restart. `#beginStream` wipes the
   * pages on every restart (watcher refresh, abort-restart, filter edit) —
   * the spacer collapses, scrollTop clamps to 0 and the user is thrown to
   * the top mid-browse, which on an active repo (background fetches,
   * worktree churn re-triggering the walk) reads as "the list keeps
   * resetting itself". The anchor commit usually sits on the re-walked
   * log's first pages (the walk is newest-first and the tip rarely moves),
   * so the jump lands within the already-released window.
   *
   * Deliberately NO eager page release (`resolveRestoredSelection`'s
   * reveal loop): a commit the natural window never reaches — deep in a
   * huge log, or one the new filter excludes — just drops the restore when
   * the stream exhausts (the pre-fix behavior, top) instead of pulling the
   * whole history into the index for it. A pending parent navigation
   * (`revealSha`) owns the scroll instead, as does any scroll the user
   * managed during the gap; a scroller measuring 0px (panel hidden behind
   * a keep-alive host) drops it too — the value would not stick anyway.
   */
  $effect(() => {
    const pending = pendingScrollRestore;
    if (!pending) return;
    if (revealSha) {
      pendingScrollRestore = null;
      return;
    }
    const el = scrollerEl;
    if (!el || el.clientHeight === 0) {
      pendingScrollRestore = null;
      return;
    }
    // A non-zero scrollTop with rows on screen means the user scrolled
    // after the data landed — their position wins. During the wipe gap
    // (total === 0) the collapsed spacer pins scrollTop to 0, so this
    // cannot misfire on the arming flush.
    if (total > 0 && el.scrollTop > 0) {
      pendingScrollRestore = null;
      return;
    }
    void store.flat; // re-run as pages release into the index
    const idx = store.indexOfSha(pending.sha);
    if (idx >= 0) {
      pendingScrollRestore = null;
      el.scrollTop = idx * ROW_HEIGHT;
      sync.onListScroll();
    } else if (!store.loading && !store.hasMore) {
      pendingScrollRestore = null;
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
  /** How many pathspecs the path filter currently carries (2+ shows the
   *  "rename-following off" hint; the backend ignores `follow` there). */
  const pathFilterCount = $derived(parsePathsFilter(store.filter.path).length);
  // `total`/`visible` read THROUGH the flat wrapper, not through `commits`:
  // the wrapper object changes identity on every release, while the inner
  // array does not (LogIndex mutates in place) — a derived re-exposing the
  // same array is equality-skipped by the graph, which left the list empty
  // after a stream restart whose render window landed back on the same
  // value (the header updated; the rows never re-rendered).
  const total = $derived.by(() => store.flat.commits.length);
  const selectedIdx = $derived.by(() => {
    void store.flat; // re-resolve after stream restarts reset the index
    return selectedSha ? store.indexOfSha(selectedSha) : -1;
  });

  /** Shas of the multi-selection (cherry-pick/revert input), list order. */
  const selectedShas = $derived(multiShas);
  const filterActive = $derived(store.filterActive);

  const activeDescendant = $derived(
    selectedIdx >= range.start && selectedIdx < range.end
      ? `commit-row-${selectedIdx}`
      : undefined,
  );

  const visible = $derived.by(() => {
    const current = store.flat.commits;
    const start = Math.max(0, Math.min(range.start, total));
    const end = Math.max(start, Math.min(range.end, total));
    const out: Array<{ commit: CommitInfo; idx: number }> = [];
    for (let i = start; i < end; i++) {
      const commit = current[i];
      if (commit) out.push({ commit, idx: i });
    }
    return out;
  });

  /**
   * Cheap safety net for an empty render window while data exists: the
   * scroll, ResizeObserver and total-effect re-window paths can all miss
   * when a restart ran while the panel was hidden, so schedule one
   * `updateOwnRange()` on the next animation frame. The frame callback is
   * deliberately fire-and-forget (canceling it on effect re-run would
   * starve the one shot whenever another dep changes in the same frame);
   * the latch guards against loops — it re-arms only once the window
   * actually changes (rows render again), so a hopeless case (scroller
   * permanently absent) never reschedules every frame.
   */
  let emptyWindowRetried = false;
  $effect(() => {
    if (total === 0 || visible.length > 0) {
      emptyWindowRetried = false;
      return;
    }
    if (emptyWindowRetried) return;
    emptyWindowRetried = true;
    const rewindow = () => {
      sync.updateOwnRange();
      // A canvas window left over from a previous stream shadows the fresh
      // local window (renderWindow prefers it) — drop it when it still
      // renders nothing, so the view falls back to the local window.
      const w = sync.renderWindow();
      if (total > 0 && w.end <= w.start) canvasWindow = null;
    };
    if (typeof requestAnimationFrame !== "function") {
      setTimeout(rewindow, 16);
    } else {
      requestAnimationFrame(rewindow);
    }
  });

  // -- branch scope (backend LogFilter.refs) ------------------------------------

  /** Local branches feeding the scope dropdown (best-effort; never blocks). */
  let branchList = $state<BranchInfo[]>([]);

  /**
   * Stale-response guard (the store's `#token` pattern): every fetch bumps
   * the token and a response is dropped when a newer fetch started — repo
   * A's slow branch list must not land after the view re-targeted to B
   * (the re-target effect resets the list and fetches again, bumping it).
   */
  let branchToken = 0;

  async function refreshBranches(): Promise<void> {
    if (repoId === "") return;
    const token = ++branchToken;
    const target = repoId;
    try {
      // The dropdown's feed rides the shared refs cache
      // (`$lib/stores/refs.svelte`) — the four panel surfaces use the same
      // cache, so a repo open scans once, not five times.
      const list = await branchesFor(target);
      if (token !== branchToken) return;
      branchList = list;
    } catch {
      if (token !== branchToken) return;
      branchList = []; // dropdown degrades to All/Current only
    }
  }

  $effect(() => {
    void repoId; // re-fetch when the repo switches
    branchList = [];
    void refreshBranches();
  });

  // The dropdown never refreshed after external ref moves (terminal,
  // outside editors, the header switcher) or after this view's OWN
  // "Branch from here…" — a branch created mid-session was missing from
  // the scope list until a repo re-switch. Refresh on significant repo
  // changes for this repo, debounced 500 ms like FilesPanel's tree refresh
  // so a watcher burst coalesces into one fetch; the LOG itself restarts
  // through the store's own repo-changed subscription — this effect only
  // feeds the scope dropdown. The view is keep-alive mounted (TabGroup),
  // so this also refetches while hidden — accepted: the read is cheap and
  // shared (the refs cache was already dropped by its own repoEvents
  // subscription; every surface's debounced refresh then shares ONE
  // rescan). StatusPanel's cleanup pattern: the pending debounce dies with
  // the view, or it would fire a branchesFor() IPC after the repo
  // re-targeted.
  $effect(() => {
    let refreshTimer: ReturnType<typeof setTimeout> | null = null;
    const unregister = onRepoChange({
      filter: (change) => change.repo_id === repoId && isSignificant(change),
      handle: () => {
        if (refreshTimer) clearTimeout(refreshTimer);
        refreshTimer = setTimeout(() => {
          refreshTimer = null;
          void refreshBranches();
        }, 500);
      },
    });
    return () => {
      unregister();
      if (refreshTimer) {
        clearTimeout(refreshTimer);
        refreshTimer = null;
      }
    };
  });

  /**
   * Branch names offered by the scope dropdown — the loaded list, plus the
   * active scope when it is a branch the list no longer contains (deleted
   * after being persisted): the select keeps showing the live value and the
   * chip stays clearable instead of silently snapping to "All branches".
   */
  const scopeOptions = $derived.by(() => {
    const scope = store.filter.scopeRef ?? "";
    const names = branchList.map((b) => b.name);
    if (scope !== "" && scope !== "HEAD" && !names.includes(scope)) {
      names.push(scope);
    }
    return names;
  });

  /** Human label of the active scope (chip); null when unscoped. */
  const scopeLabel = $derived.by(() => {
    const scope = store.filter.scopeRef ?? "";
    if (scope === "") return null;
    if (scope === "HEAD") return tabStore.active?.status?.branch ?? "current branch";
    return scope;
  });

  /**
   * The scope dropdown's value is applied by effect, not a `value` prop: the
   * branch options stream in asynchronously (and may include the persisted
   * scope only after `refreshBranches` lands), so re-apply whenever the
   * option list or the filter field changes — a one-way `value` attribute
   * can otherwise strand on "All branches" while the store still holds the
   * restored scope.
   */
  let scopeSelectEl = $state<HTMLSelectElement | null>(null);
  $effect(() => {
    const el = scopeSelectEl;
    if (!el) return;
    void scopeOptions; // re-apply after the option list (re)renders
    el.value = store.filter.scopeRef ?? "";
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
      multiShas = [sha];
      anchorSha = sha;
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

  // Parked cross-panel reveals ("Show in History" from BranchPanel /
  // StashPanel via `history-select-commit`): the same resolve machine as the
  // persisted-selection restore — select + scroll once the commit lands,
  // info-toast when the exhausted stream never delivers it.
  $effect(() => {
    const pending = pendingReveal;
    if (!pending) return;
    void store.flat; // re-run as pages release into the index
    const outcome = store.resolveRestoredSelection(pending.sha);
    if (outcome.kind === "resolved") {
      pendingReveal = null;
      select(pending.sha, { scroll: true });
    } else if (outcome.kind === "dropped") {
      pendingReveal = null;
      if (pending.toastOnMiss) {
        toast("Commit is outside the loaded history window", { kind: "info" });
      }
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
      checkout: () => requestCheckout(info.sha),
    });
  });

  /**
   * List row click: plain click (re)selects one commit; shift-click extends
   * the multi-selection to the anchor..clicked range (cherry-pick input).
   */
  function onRowClick(sha: string, idx: number, event: MouseEvent): void {
    if (event.shiftKey && anchorSha !== null) {
      // Shift-click extends over the anchor..clicked ROW range (resolved
      // in the current stream — the selection itself stays sha-owned).
      const anchorIdx = store.indexOfSha(anchorSha);
      if (anchorIdx >= 0) {
        const lo = Math.min(anchorIdx, idx);
        const hi = Math.max(anchorIdx, idx);
        const range: string[] = [];
        for (let i = lo; i <= hi; i++) {
          const commit = commits[i];
          if (commit) range.push(commit.sha);
        }
        multiShas = range;
        select(sha, { keepMulti: true });
        return;
      }
      // The anchor row is not in the loaded stream anymore: degrade to a
      // plain single pick instead of silently re-pointing the range.
    }
    anchorSha = sha;
    select(sha);
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
    // Shift + movement extends the multi-selection over the anchor..target
    // row range (the shift-click machine, driven by keys; Home/End reach
    // the list ends). The anchor stays put, so repeated shifts grow the
    // same range; a plain click/arrow resets it (select()).
    if (event.shiftKey) {
      const anchorIdx =
        anchorSha !== null
          ? store.indexOfSha(anchorSha)
          : current >= 0
            ? current
            : next;
      const lo = Math.min(Math.max(anchorIdx, 0), next);
      const hi = Math.max(Math.max(anchorIdx, 0), next);
      const range: string[] = [];
      for (let i = lo; i <= hi; i++) {
        const commit = commits[i];
        if (commit) range.push(commit.sha);
      }
      if (range.length > 0) {
        multiShas = range;
        const sha = commits[next]?.sha;
        if (sha) select(sha, { keepMulti: true, scroll: true });
        return;
      }
      // The anchor row is no longer in the loaded stream: fall through to
      // the plain move (which re-anchors), like shift-click's degradation.
    }
    const sha = commits[next]?.sha;
    if (sha) select(sha, { scroll: true });
  }

  // -- commit detail: diff/describe/blame live in CommitDetail ------------------

  function onCompare(
    files: FileDiff[],
    base: string,
    target: string,
    sides?: { old: DiffSide; new: DiffSide },
  ): void {
    // Attach the multi-selection's commits (when the compare came from
    // selected rows) so the center detail can offer AI release notes.
    const selected = rangeCommits();
    const commits = selected.length > 0 ? selected : undefined;
    if (sidebar) {
      // The workspace renders the result in the center editor; this panel
      // stays a pure list (no detail pane of its own).
      onCompareResult?.({ files, base, target, commits, sides });
      return;
    }
    compare = { files, base, target, commits, sides };
    detailOpen = true;
  }

  /** The multi-selection as prompt data, newest first (log row order). */
  function rangeCommits(): Array<{ summary: string; sha: string }> {
    const out: Array<{ summary: string; sha: string }> = [];
    for (const sha of multiShas) {
      const row = commits.find((c) => c.sha === sha);
      if (row) out.push({ summary: row.summary, sha: row.sha });
    }
    return out;
  }

  // -- commit actions (M3 E2) ---------------------------------------------------

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
    planner = openDialog({ baseSha: selectedInfo.sha });
  }

  async function copySha(): Promise<void> {
    if (!selectedInfo) return;
    await copyText(selectedInfo.sha, "SHA");
  }

  /** M12: `git describe` for a commit → clipboard (menu + detail action). */
  async function copyDescribe(sha: string): Promise<void> {
    try {
      const text = await describe(repoId, sha);
      await copyText(text);
    } catch (err) {
      toast(`Describe failed: ${describeError(err)}`, { kind: "error" });
    }
  }

  // -- M12: branch / tag / reset "from here" (commit menu) ----------------------

  /** Creates a branch at `sha` (not checked out — HEAD stays put). */
  async function createBranchFrom(sha: string, name: string): Promise<void> {
    branchFrom = closedDialog();
    try {
      await branchCreate(repoId, name, false, sha);
      toast(`Created branch ${name} at ${sha.slice(0, 7)}`, { kind: "success" });
      // The new ref must reach the scope dropdown NOW, not on the next
      // watcher batch: drop the shared refs cache first (its TTL slot
      // still holds the pre-mutation list), then refetch — the debounced
      // repo-change refresh below re-serves the repopulated slot, so the
      // follow-up watcher burst costs no extra scan.
      invalidateRefs(repoId);
      void refreshBranches();
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
    tagAt = closedDialog();
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
    resetTo = closedDialog();
    void tabStore.refreshStatus(repoId);
  }

  function onRebaseFinished(): void {
    // History restarts itself via repo-changed (HEAD moved); refresh status.
    void tabStore.refreshStatus(repoId);
  }

  // -- M9 F1: palette event wiring + context menus ---------------------------

  // null, not undefined: bound via bind:ref on ui Input, whose ref prop has
  // a fallback — Svelte throws props_invalid_value on a undefined binding.
  let filterTextEl = $state<HTMLInputElement | null>(null);

  // Palette commands ride the typed event bus (palette/events.ts).
  $effect(() => {
    const unlisteners = [
      onUiEvent("history-focus-filter", () => filterTextEl?.focus()),
      // Ctrl+F follows focus (the keybinds engine owns the combo globally —
      // see the `diff.find` command): inside this panel it focuses the
      // search input. Keep-alive mounting means several instances are live,
      // so the focus guard picks the one the user is working in.
      onUiEvent("history-filter", () => {
        if (!panelEl?.contains(document.activeElement)) return;
        filterTextEl?.focus();
      }),
      onUiEvent("history-clear-filter", () => store.clearFilter()),
      onUiEvent("history-refresh", () => {
        store.restart();
        void refreshBranches(); // the scope dropdown follows branch mutations
      }),
      onUiEvent("history-toggle-blame", () => {
        // The file list lives in CommitDetail — delegate to its exported
        // handler (null when the detail pane is closed).
        if (!detailRef?.toggleBlameFirstFile()) {
          toast("Select a commit with file changes to blame");
        }
      }),
      onUiEvent("history-select-commit", ({ sha, repoId: target, toastOnMiss }) => {
        if (target !== undefined && target !== repoId) return;
        if (store.revealSha(sha)) {
          select(sha, { scroll: true });
        } else {
          // Beyond the loaded window (and nothing buffered to release):
          // park the request — it selects when the stream delivers the
          // commit, or toasts once the stream exhausts without it.
          pendingReveal = { sha, toastOnMiss: toastOnMiss ?? false };
        }
      }),
      onUiEvent("history-jump-to-sha", () => openJump()),
      onUiEvent("graph-open-context-menu", ({ sha, clientX, clientY }) => {
        // Right-click on the graph canvas: same behavior as the DOM list's
        // row menu — select the commit, then open the commit menu at the
        // pointer (the row is already on screen; no scroll).
        select(sha);
        contextMenu.show(clientX, clientY, commitMenuEntries(sha));
      }),
      onUiEvent("commit-action", ({ sha, action }) => {
        if (repoId === "") return;
        select(sha);
        if (action === "cherry-pick") void doCherryPick();
        else if (action === "revert") void doRevert();
        else if (action === "bookmark") toggleBookmark(sha);
        else if (action === "checkout") requestCheckout(sha);
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

  /**
   * The commit action menu (branch/tag/reset/cherry-pick/revert/copy) for
   * `sha` — shared by the DOM list's row right-click and the graph canvas's
   * `graph-open-context-menu`. Entries close over this view's handlers, so
   * the selection done by the caller is what the actions operate on.
   */
  function commitMenuEntries(sha: string): MenuEntry[] {
    const bookmarked = root !== "" && bookmarkStore.has(root, sha);
    // The opBusy-gated handlers (cherry-pick/revert/rebase/archive) bail
    // silently on their guard; a disabled entry says why instead of a
    // click doing nothing mid-op.
    const entries: MenuEntry[] = [
      { id: "pick", label: "Cherry-pick…", disabled: opBusy, run: () => void doCherryPick() },
      { id: "revert", label: "Revert…", disabled: opBusy, run: () => void doRevert() },
      { id: "rebase", label: "Rebase from here…", disabled: opBusy, run: rebaseFromHere },
      { id: "bookmark", label: bookmarked ? "Remove bookmark" : "Bookmark commit", run: () => toggleBookmark(sha) },
      { id: "branch-from", label: "Branch from here…", run: () => (branchFrom = openDialog({ sha })) },
      { id: "tag-at", label: "Tag here…", run: () => (tagAt = openDialog({ sha })) },
      { id: "reset-to", label: "Reset current branch to here…", run: () => (resetTo = openDialog({ sha })) },
      { id: "copy", label: "Copy sha", run: copySha },
      { id: "copy-describe", label: "Copy describe", run: () => void copyDescribe(sha) },
      { id: "archive", label: "Export archive…", disabled: opBusy, run: () => void exportArchive(sha) },
    ];
    // VSCode-parity checkout: hidden at HEAD; a branch tip switches to the
    // branch instead of detaching (labels travel with the target).
    if (sha !== headSha) {
      const row = commits.find((c) => c.sha === sha);
      const branch = row ? localBranchAt(row) : null;
      entries.unshift({
        id: "checkout",
        label: branch ? `Switch to ${branch}` : "Checkout commit…",
        run: () => requestCheckout(sha),
      });
    }
    // M13: AI explain, gated like every AI affordance (master + feature).
    if (ai.isEnabled() && ai.isFeatureEnabled("explain-commit")) {
      entries.push({ id: "ai-explain", label: "Explain with AI…", run: () => void explainCommit(sha) });
    }
    return entries;
  }

  /**
   * M13: headless explain-commit for one commit — gathers the parent0→sha
   * diff (the same shape CommitDetail shows), runs the feature, and routes
   * the answer to the shared result dialog. Root commits (no parent) toast
   * instead of failing opaquely.
   */
  async function explainCommit(sha: string): Promise<void> {
    const row = commits.find((c) => c.sha === sha);
    const parent = row?.parents[0];
    if (!row || !parent) {
      toast("Root commit — nothing to diff against", { kind: "info" });
      return;
    }
    try {
      const files = await repoDiff(repoId, { commit: parent }, { commit: sha });
      // Log pages carry only the summary; the AI prompt gets the full body.
      const message = await commitMessage(repoId, sha).catch(() => null);
      const outcome = await ai.run("explain-commit", {
        repoId,
        sha,
        description: message ?? row.summary,
        diff: budgetedDiffText(files),
      });
      emitUiEvent("ai-show-result", {
        title: `Commit ${sha.slice(0, 7)}`,
        text: outcome.text ?? outcome.result.text,
        meta: aiResultMeta(outcome.result),
      });
    } catch (err) {
      toast(`AI explain failed: ${describeError(err)}`, { kind: "error" });
    }
  }

  /** Row right-click: the commit action menu. */
  function commitMenu(event: MouseEvent, sha: string, idx: number): void {
    onRowClick(sha, idx, event);
    showMenuAt(event, commitMenuEntries(sha));
  }

  /** M11: zip/tar export of the commit's tree via the save picker. */
  async function exportArchive(spec: string): Promise<void> {
    // A large archive can take a while; ride the opBusy flag so concurrent
    // ops stay gated and the existing busy surfaces (sidebar actions,
    // CommitDetail) reflect it.
    if (opBusy) return;
    opBusy = true;
    try {
      const destination = await pickSaveFile(
        "Export archive",
        `archive-${spec.slice(0, 8)}.zip`,
        [
          { name: "Zip archive", extensions: ["zip"] },
          { name: "Tar archive", extensions: ["tar", "gz"] },
        ],
      );
      if (!destination) return;
      const format = destination.endsWith(".zip")
        ? "zip"
        : destination.endsWith(".tar")
          ? "tar"
          : "tar.gz";
      await archiveSpec(repoId, spec, format, destination);
      toast(`Archive written to ${destination}`, { kind: "success" });
    } catch (err) {
      toast(
        `Archive failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      opBusy = false;
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
  class="history @container flex h-full min-h-0 flex-col bg-card [&>.split]:flex-1 [&>.split]:min-h-0"
  class:sidebar
  aria-label="Commit history"
  bind:this={panelEl}
>
  {#if sidebar}
    <!-- 1a. sidebar header: the panel is a bare layout leaf, so it owns its
         own title row (full mode lives in a tab/popout and has one). -->
    <header class="sidebar-head flex flex-none items-center gap-1 border-b border-border bg-popover px-2.5 py-1">
      <span class="sh-title min-w-0 flex-1 truncate text-ui-micro font-semibold uppercase tracking-[0.04em] text-muted-foreground">History</span>
      {#if onPopout}
        <Hint label="Pop out history">
          {#snippet child({ props })}
            <Button
              {...props}
              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
              variant="ghost"
              size="icon-xs"
              type="button"
              aria-label="Pop out history"
              onclick={onPopout}
            >
              <Icon icon={iconPopout} size={16} />
            </Button>
          {/snippet}
        </Hint>
      {/if}
      {#if onCollapse}
        <Hint label="Hide the history sidebar">
          {#snippet child({ props })}
            <Button
              {...props}
              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
              variant="ghost"
              size="icon-xs"
              type="button"
              aria-label="Hide history sidebar"
              onclick={onCollapse}
            >
              <Icon icon={iconCollapse} size={16} />
            </Button>
          {/snippet}
        </Hint>
      {/if}
    </header>
  {/if}

  <!-- 1b. filter bar (sidebar: search + a disclosure for the rest) -->
  <div
    class="filterbar flex flex-wrap items-center border-b border-border bg-popover text-xs {sidebar ? 'compact gap-1 px-1.5 py-1' : 'gap-1.5 px-3 py-1.5'}"
    class:compact={sidebar}
    role="search"
    aria-label="Filter commits"
  >
    <!-- Popout: flexible with a placeholder-fitting floor — a fixed w-36 can
         be flex-crushed below the "Search commits" placeholder width and
         truncate it (inputs are w-full min-w-0, so the wrapper's min-content
         is 0 and the row squeezes it first). -->
    <span class="relative inline-flex items-center text-muted-foreground {sidebar
      ? 'w-auto min-w-0 flex-1 basis-28'
      : 'w-auto min-w-40 flex-1 basis-40'}">
      <span class="absolute left-[0.45rem] inline-flex pointer-events-none opacity-75">
        <Icon icon={iconSearch} size={12} />
      </span>
      <Input
        class="f h-auto rounded-sm bg-card dark:bg-card px-6 py-0.5 text-xs md:text-xs placeholder:text-muted-foreground/80"
        bind:ref={filterTextEl}
        type="search"
        placeholder="Search commits"
        aria-label="Search commit text"
        value={store.filter.text}
        oninput={(e) => store.setFilter({ text: e.currentTarget.value })}
      />
    </span>

    <!-- Workstream D §8.4: the advanced filters (author / path / date range /
         ref scope / regex / pickaxe) collapse into ONE icon-triggered
         popover — the fields render in the portaled panel at the bottom of
         the view. The dot counts the active advanced fields; the text
         search stays an always-visible input beside it. -->
    <Hint
      label={advancedFilterCount > 0
        ? `Filters — ${advancedFilterCount} active`
        : "Filters (author, path, dates, ref)"}
    >
      {#snippet child({ props })}
        <Button
          bind:ref={filtersAnchor}
          {...props}
          class="relative flex-none after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground {filtersOpen ||
          advancedFilterCount > 0
            ? 'text-primary'
            : 'text-muted-foreground'} [&_svg:not([class*='size-'])]:size-4"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label={advancedFilterCount > 0
            ? `Filters — ${advancedFilterCount} active`
            : "Filters (author, path, dates, ref)"}
          aria-haspopup="dialog"
          aria-expanded={filtersOpen}
          onclick={toggleFilters}
        >
          <Icon icon={iconFilters} size={16} />
          {#if advancedFilterCount > 0}
            <span
              class="absolute -top-1 -right-1 inline-flex min-w-4 items-center justify-center rounded-full bg-primary px-1 text-ui-micro leading-[1.3] font-semibold text-primary-foreground tabular-nums"
              aria-hidden="true"
            >
              {advancedFilterCount > 9 ? "9+" : advancedFilterCount}
            </span>
          {/if}
        </Button>
      {/snippet}
    </Hint>

    {#if scopeLabel}
      <!-- Scope chip stays visible even with the advanced filters collapsed
           (sidebar mode): the log may be narrower than it looks, so the
           active scope is always shown and one click clears it. -->
      <Hint label="The log is scoped to this branch's history">
        {#snippet child({ props })}
          <span
            {...props}
            class="scope-chip inline-flex flex-none items-center gap-ui-15 rounded-full bg-primary/10 py-0 pr-ui-3 pl-2 text-ui-micro text-primary"
          >
            <span class="max-w-36 truncate">scoped to {scopeLabel}</span>
            <Hint label="Back to all branches">
              {#snippet child({ props })}
                <button
                  {...props}
                  class="inline-flex items-center rounded-full p-0.5 text-primary/80 hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:outline-ring [&_svg:not([class*='size-'])]:size-2.5"
                  type="button"
                  aria-label="Clear branch scope"
                  onclick={() => store.setFilter({ scopeRef: "" })}
                >
                  <Icon icon={iconClose} size={10} />
                </button>
              {/snippet}
            </Hint>
          </span>
        {/snippet}
      </Hint>
    {/if}

    {#if !sidebar}
      <!-- Workstream D §8.3: graph column toggle (collapse to 0; the column
           ALSO auto-collapses below a 560px container — see
           GRAPH_AUTO_COLLAPSE_PX — so the toggle reflects the user state
           and the tooltip explains the width rule). -->
      <Hint
        label={graphCollapsed
          ? "Show the commit graph"
          : historyWidth.narrow
            ? "Hide the commit graph (stays hidden while the panel is narrower than 560px)"
            : "Hide the commit graph"}
      >
        {#snippet child({ props })}
          <Button
            {...props}
            class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
            variant="ghost"
            size="icon-xs"
            type="button"
            aria-label={graphCollapsed ? "Show the commit graph" : "Hide the commit graph"}
            aria-pressed={graphCollapsed}
            onclick={toggleGraph}
          >
            <Icon icon={iconGraph} size={16} />
          </Button>
        {/snippet}
      </Hint>
    {/if}

    <Hint label="Compare commits">
      {#snippet child({ props })}
        <Button
          {...props}
          class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label="Compare commits"
          aria-pressed={showCompare}
          aria-expanded={showCompare}
          onclick={() => (showCompare = !showCompare)}
        >
          <Icon icon={iconCompare} size={16} />
        </Button>
      {/snippet}
    </Hint>

    {#if onPopout && !sidebar}
      <!-- M4 F1: pop the history out into its own window (desktop only). -->
      <Hint label="Pop out history into its own window">
        {#snippet child({ props })}
          <Button
            {...props}
            class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
            variant="ghost"
            size="icon-xs"
            type="button"
            aria-label="Pop out history"
            onclick={onPopout}
          >
            <Icon icon={iconPopout} size={16} />
          </Button>
        {/snippet}
      </Hint>
    {/if}

    <span
      class="status flex items-center gap-1.5 text-ui-micro text-muted-foreground {sidebar ? 'basis-full' : 'ml-auto'}"
      role="status"
      aria-live="polite"
    >
      {#if store.error}
        <span class="err text-destructive" role="alert">{store.error}</span>
        <Hint label="Retry loading history">
          {#snippet child({ props })}
            <Button
              {...props}
              class="relative text-muted-foreground after:absolute after:-inset-1 after:content-[''] hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-4"
              variant="ghost"
              size="icon-xs"
              type="button"
              aria-label="Retry loading history"
              onclick={() => store.restart()}
            >
              <Icon icon={iconRetry} size={16} />
            </Button>
          {/snippet}
        </Hint>
      {:else if store.loading}
        <Spinner class="size-3" role="presentation" aria-hidden="true" />
        Streaming…
      {:else}
        <!-- Workstream D §8.1: the count line is "N commits" — the buffered-
             pages debug detail ("N pages buffered…") moved into the tooltip
             (kept one hover away, off the primary surface). -->
        {#if store.hasMore}
          <Hint
            label={`${total.toLocaleString()} ${total === 1 ? "commit" : "commits"} loaded · ${store.pendingCount} pages buffered — scroll to the bottom to load more`}
          >
            {#snippet child({ props })}
              <span {...props} class="cursor-help">
                {total.toLocaleString()}
                {total === 1 ? "commit" : "commits"}
              </span>
            {/snippet}
          </Hint>
        {:else}
          {total.toLocaleString()}
          {total === 1 ? "commit" : "commits"}
        {/if}
      {/if}
    </span>
  </div>

  {#if jumpOpen}
    <!-- Jump-to-SHA quick open (palette `history.jump-to-sha`): one input,
         sha / short sha / branch / tag; errors surface inline, never as
         toasts. -->
    <div class="jumpbar flex flex-wrap items-center gap-1.5 border-b border-border bg-popover px-2.5 py-1 text-xs" role="search" aria-label="Jump to commit">
      <span class="relative inline-flex min-w-0 flex-1 basis-40 items-center text-muted-foreground">
        <span class="pointer-events-none absolute left-[0.45rem] inline-flex opacity-75">
          <Icon icon={iconSearch} size={12} />
        </span>
        <Input
          class="h-auto rounded-sm bg-card px-6 py-0.5 text-xs placeholder:text-muted-foreground/80"
          bind:ref={jumpInputEl}
          type="text"
          placeholder="Jump to sha, branch, or tag…"
          aria-label="Jump to commit"
          aria-invalid={jumpError !== null}
          onkeydown={jumpKeydown}
        />
      </span>
      <Button
        class="h-6 shrink-0 rounded-sm px-2.5 text-ui-meta leading-[1.5] font-normal"
        variant="outline"
        type="button"
        onclick={() => void jumpSubmit()}
      >Go</Button>
      <Button
        class="h-6 shrink-0 rounded-sm px-2 text-ui-meta leading-[1.5] font-normal text-muted-foreground"
        variant="ghost"
        type="button"
        aria-label="Close jump-to-commit"
        onclick={closeJump}
      >✕</Button>
      {#if jumpError !== null}
        <span class="jump-error w-full text-ui-meta text-destructive" role="alert">{jumpError}</span>
      {/if}
    </div>
  {/if}

  {#if showCompare}
    <CompareBar {repoId} {root} {onCompare} />
  {/if}

  <!-- 2. log: DOM rows (left) + GraphCanvas (right), scroll-synced.
       Declared as a snippet: it renders standalone (detail closed) or as
       pane `a` of the detail SplitPane (detail open) — one copy of the
       virtualizer markup. -->
  {#snippet listOnly()}
  <div class="listwrap relative flex min-h-0 flex-1">
    <!-- The scroller stays MOUNTED across loading/empty states: swapping it
         out per state destroyed `scrollerEl` on every stream restart, and a
         restart while the panel was hidden left the recreated scroller
         unmeasured (0px) with the render window stuck empty until the user
         scrolled. Loading / empty render as overlays instead (the spacer is
         0px tall while total === 0, so the scroller itself shows nothing). -->
    <div
      class="scroller min-w-0 flex-1 overflow-y-auto focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring {sidebar
        ? ""
        : "[scrollbar-width:none] [&::-webkit-scrollbar]:hidden"}"
      bind:this={scrollerEl}
      onscroll={onScroll}
      role="listbox"
      aria-label="Commits"
      aria-multiselectable="true"
      tabindex="0"
      aria-activedescendant={activeDescendant}
      onkeydown={onKeydown}
    >
      <div class="spacer relative" style:height={`${total * ROW_HEIGHT}px`}>
        {#each visible as v (v.commit.sha)}
          <div
            id={`commit-row-${v.idx}`}
            class="row group absolute inset-x-0 flex cursor-pointer items-center border-b border-border/45 py-0 text-ui-base {sidebar ? "gap-1 pr-2 pl-2" : "gap-1.5 pr-2.5 pl-3"} {v.idx === selectedIdx
              ? "bg-secondary text-secondary-foreground"
              : multiSet.has(v.commit.sha)
                ? "bg-secondary/45 hover:bg-secondary/60"
                : "hover:bg-accent"}"
            class:selected={v.idx === selectedIdx}
            class:multisel={v.idx !== selectedIdx && multiSet.has(v.commit.sha)}
            role="option"
            tabindex="-1"
            aria-selected={v.idx === selectedIdx || multiSet.has(v.commit.sha)}
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
            <!-- VSCode Source Control Graph row: avatar → subject →
                 branch labels → hover actions → relative time. The hash
                 lives in the summary tooltip, the copy chip, and the
                 detail pane. §8.5: the avatar is a hover/wide affordance —
                 hidden by default, shown on row hover, always shown once
                 the container is ≥40rem (the narrow rows need the width
                 for the message). -->
            <Hint label={`${v.commit.author.name}${v.commit.author.email ? ` <${v.commit.author.email}>` : ""}`}>
              {#snippet child({ props })}
                <span
                  {...props}
                  class="avatar-cell hidden flex-none cursor-help group-hover:flex @[40rem]:flex"
                >
                  <AuthorAvatar name={v.commit.author.name} email={v.commit.author.email} />
                </span>
              {/snippet}
            </Hint>
            <!-- Log pages carry only the summary — the hover hint shows the
                 same first line (the full body lives in the detail pane). -->
            <Hint label={`${v.commit.sha}\n${v.commit.summary}`}>
              {#snippet child({ props })}
                <span {...props} class="summary inline-flex min-w-0 flex-1 cursor-help items-center gap-1 truncate">
                  <span class="min-w-0 truncate">{v.commit.summary}</span>
                  {#if isCheckpointSummary(v.commit.summary)}
                    <span
                      class="checkpoint-badge flex-none rounded-sm border border-border bg-muted px-1 text-ui-micro leading-[1.4] text-muted-foreground"
                      title="mygitui checkpoint snapshot"
                      >checkpoint</span
                    >
                  {/if}
                </span>
              {/snippet}
            </Hint>
            <!-- §8.2: first 2 refs + a `+n` chip opening the refs popover
                 (all refs, copy/checkout). One tint language for every
                 kind: branch=blue, tag=purple, remote=muted, HEAD=accent
                 (RefBadge). -->
            {#each v.commit.refs.slice(0, 2) as ref, i (`${v.commit.sha}:${i}:${ref}`)}
              <RefBadge {ref} compact={sidebar} maxWidthClass={sidebar ? "max-w-24" : "max-w-28"} />
            {/each}
            {#if v.commit.refs.length > 2}
              <button
                type="button"
                class="ref ref-more inline-flex flex-none cursor-pointer items-center whitespace-nowrap rounded-sm border border-transparent bg-muted px-1 text-ui-micro leading-[1.4] tabular-nums text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                aria-label={`${v.commit.refs.length - 2} more refs: ${v.commit.refs.slice(2).map(shortRefName).join(", ")}`}
                aria-haspopup="dialog"
                aria-expanded={refsPopover?.sha === v.commit.sha}
                onclick={(e) => {
                  e.stopPropagation();
                  openRefsPopover(v.commit.sha, v.commit.refs, e.currentTarget);
                }}
              >
                +{v.commit.refs.length - 2}
              </button>
            {/if}
            <!-- Hover actions: copy hash, checkout, create branch. `hidden`
                 (not opacity) keeps the cluster out of hit-testing and the
                 a11y tree until hover — an invisible button must never eat a
                 row click. It overlays the row's right edge over the date
                 (hidden while hovering), matching the row tint via bg-inherit;
                 buttons stop propagation so the row click stays a pure
                 selection. -->
            <span class="row-actions absolute inset-y-0 right-1.5 hidden items-center gap-0.5 bg-inherit pl-1.5 group-hover:flex">
              <Hint label="Copy commit hash" side="top">
                {#snippet child({ props })}
                  <button
                    {...props}
                    type="button"
                    class="sha-copy flex-none cursor-pointer rounded-sm border border-border bg-muted px-1 font-mono text-ui-micro leading-[1.4] tabular-nums text-muted-foreground hover:text-foreground"
                    aria-label="Copy commit hash"
                    onclick={(e) => {
                      e.stopPropagation();
                      void copyText(v.commit.sha, "SHA");
                    }}
                  >{v.commit.sha.slice(0, 7)}</button>
                {/snippet}
              </Hint>
              {#if v.commit.sha !== headSha}
                {@const branch = localBranchAt(v.commit)}
                <Hint label={branch ? `Switch to ${branch}` : "Checkout commit (detached HEAD)"} side="top">
                  {#snippet child({ props })}
                    <button
                      {...props}
                      type="button"
                      class="checkout-action inline-flex size-5 flex-none cursor-pointer items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
                      aria-label={branch ? `Switch to ${branch}` : "Checkout commit (detached HEAD)"}
                      onclick={(e) => {
                        e.stopPropagation();
                        requestCheckout(v.commit.sha);
                      }}
                    >
                      <Icon icon={iconCheckout} size={14} />
                    </button>
                  {/snippet}
                </Hint>
              {/if}
              <Hint label="Create branch here" side="top">
                {#snippet child({ props })}
                  <button
                    {...props}
                    type="button"
                    class="branch-action inline-flex size-5 flex-none cursor-pointer items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
                    aria-label="Create branch here"
                    onclick={(e) => {
                      e.stopPropagation();
                      branchFrom = openDialog({ sha: v.commit.sha });
                    }}
                  >
                    <Icon icon={iconBranch} size={14} />
                  </button>
                {/snippet}
              </Hint>
            </span>
            <Hint label={formatDateTime(v.commit.author.time)}>
              {#snippet child({ props })}
                <!-- Relative label stays compact in the row; the exact local
                     timestamp rides on hover ("3 weeks ago" alone goes stale).
                     Hidden while the hover-actions overlay covers it. -->
                <span
                  {...props}
                  class="date flex-none cursor-help text-ui-micro text-muted-foreground tabular-nums group-hover:invisible"
                  >{formatRelativeTime(v.commit.author.time)}</span
                >
              {/snippet}
            </Hint>
          </div>
        {/each}
      </div>
    </div>
    {#if store.loading && total > 0}
      <!-- Workstream D §8.1: load-on-scroll is a subtle spinner row pinned
           to the list's bottom edge — no layout shift, no modal overlay
           (the status line beside the filters still carries the streaming
           state; deliberately NOT role="status" so the two never
           double-announce). -->
      <div
        class="load-more pointer-events-none absolute inset-x-0 bottom-1 z-10 flex justify-center"
        aria-hidden="true"
      >
        <span
          class="inline-flex items-center gap-ui-4 rounded-full border border-border bg-popover px-2 py-0.5 text-ui-micro text-muted-foreground shadow-sm"
        >
          <Spinner class="size-3" role="presentation" aria-hidden="true" />
          Loading more…
        </span>
      </div>
    {/if}
    {#if store.loading && total === 0}
      <!-- Loading overlay: pointer-events-none so the (empty) scroller keeps
           focus/scroll handling while the first page streams in. -->
      <div
        class="absolute inset-0 z-10 flex items-center justify-center bg-card pointer-events-none"
      >
        <p class="empty m-auto text-ui-base text-muted-foreground" role="status">
          <span class="empty-loading inline-flex items-center gap-ui-4">
            <Spinner class="size-4" role="presentation" aria-hidden="true" />
            Loading history…
          </span>
        </p>
      </div>
    {:else if !store.loading && !store.error && total === 0}
      <!-- Empty overlay: keeps the EmptyState interactive (action buttons)
           with its own stacking above the still-mounted scroller. An empty
           log without an active filter is just a freshly initialized repo. -->
      <div class="absolute inset-0 z-10 flex items-center justify-center bg-card">
        <EmptyState
          title={filterActive
            ? "No commits match the current filters."
            : "No commits yet."}
        />
      </div>
    {/if}
  </div>
  {/snippet}

  {#snippet logPane()}
    <!-- Workstream D §8.3: the graph column is a resizable SplitPane pane
         (ratio persisted under the `history-graph` splitPrefs key, clamped
         to 12–45% of the row) and disappears entirely when collapsed or
         when the container is narrower than 560px — the list reflows to the
         full width and the scroll sync degrades to its local-window mode
         (the sidebar variant's normal path). -->
    {#if !sidebar && graphVisible}
      <div class="graphsplit flex min-h-0 min-w-0 flex-1">
        <SplitPane
          axis="x"
          ratio={graphRatio}
          onRatio={setGraphRatio}
          label="Resize commit list and graph"
        >
          {#snippet a()}
            {@render listOnly()}
          {/snippet}
          {#snippet b()}
            <div class="graphcol h-full min-h-0 border-l border-border bg-card">
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
          {/snippet}
        </SplitPane>
      </div>
    {:else}
      {@render listOnly()}
    {/if}
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
          onCheckout={() => {
            if (selectedInfo) requestCheckout(selectedInfo.sha);
          }}
          onPopout={selectedInfo ? popOutDetail : undefined}
        />
      {/snippet}
    </SplitPane>
  {:else}
    {@render logPane()}
    {#if selectedInfo}
      <Button
        variant="ghost"
        class="detail-open flex-none h-auto w-full justify-start truncate rounded-none border-t border-border bg-popover px-3 py-1 text-left text-ui-meta font-normal text-primary hover:bg-accent hover:text-primary focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring dark:hover:bg-accent"
        onclick={() => (detailOpen = true)}
        aria-expanded="false"
      >
        Show commit detail — {selectedInfo.summary}
      </Button>
    {/if}
  {/if}

  <!-- 4. interactive rebase planner (M3 E2) -->
  {#if planner.open}
    <RebasePlanner
      {repoId}
      baseSha={planner.baseSha}
      onClose={() => (planner = closedDialog())}
      onFinished={onRebaseFinished}
    />
  {/if}

  <!-- 5. M12 commit-menu dialogs (branch / tag / reset "from here") -->
  {#if branchFrom.open}
    <PromptDialog
      open={true}
      title={`Branch from ${branchFrom.sha.slice(0, 7)}`}
      message="Creates the branch at this commit; your current branch stays checked out."
      placeholder="New branch name"
      confirmLabel="Create branch"
      onSubmit={(name) => {
        if (branchFrom.open) void createBranchFrom(branchFrom.sha, name);
      }}
      onCancel={() => (branchFrom = closedDialog())}
    />
  {/if}

  {#if tagAt.open}
    <TagAtDialog
      sha={tagAt.sha}
      onSubmit={(name, annotated, message) => {
        if (tagAt.open) void createTagAt(tagAt.sha, name, annotated, message);
      }}
      onCancel={() => (tagAt = closedDialog())}
    />
  {/if}

  {#if resetTo.open}
    <ResetDialog
      {repoId}
      open={true}
      defaultTarget={resetTo.sha}
      onClose={() => (resetTo = closedDialog())}
      onDone={onResetFromHereDone}
    />
  {/if}

  {#if checkoutAt.open}
    {@const sha = checkoutAt.sha}
    {@const summary = checkoutAt.summary}
    {@const branch = checkoutAt.branch ?? null}
    <ConfirmDialog
      open={true}
      title={branch ? `Switch to ${branch}` : `Checkout ${sha.slice(0, 7)}`}
      danger={worktreeDirty}
      confirmLabel={
        branch
          ? worktreeDirty ? "Discard changes and switch" : "Switch"
          : worktreeDirty ? "Discard changes and checkout" : "Checkout"
      }
      alternateLabel={worktreeDirty ? "Stash changes first" : undefined}
      onConfirm={() => void runCheckout(sha, branch, false)}
      onAlternate={() => void runCheckout(sha, branch, true)}
      onCancel={() => (checkoutAt = closedDialog())}
    >
      {#snippet message()}
        {#if summary}
          <p class="mb-2 font-medium">{summary}</p>
        {/if}
        {#if branch}
          <p>
            Checks out the branch{worktreeDirty
              ? " — conflicting local modifications are discarded"
              : ""}.
          </p>
        {:else}
          <p>
            HEAD will detach at <span class="font-mono">{sha.slice(0, 7)}</span> —
            you are leaving the branch. Create a branch from here to keep new work.
          </p>
        {/if}
        {#if worktreeDirty}
          <p class="mt-2">
            The worktree has uncommitted changes: stash them first, or proceed and
            discard conflicting modifications (a checkpoint snapshot is taken either way).
          </p>
        {/if}
      {/snippet}
    </ConfirmDialog>
  {/if}

  {#if filtersOpen}
    <!-- Workstream D §8.4: the ONE filter menu (author / path / date range /
         ref / regex / pickaxe) behind the filter icon. Portaled to <body>
         (the bar clips in narrow panels); every input keeps its original
         aria-label — only the container moved, the filter semantics are the
         store's and unchanged. -->
    <div
      bind:this={filtersEl}
      class="filters-popover fixed z-60 flex max-h-[min(32rem,calc(100vh-2rem))] w-80 max-w-[calc(100vw-1rem)] flex-col gap-2.5 overflow-y-auto rounded-lg border border-border bg-popover p-3 text-ui-base text-popover-foreground shadow-md"
      role="dialog"
      aria-label="Commit filters"
      use:floating={{ anchor: filtersAnchor ?? undefined, alignEnd: true }}
    >
      <div class="flex flex-none items-center gap-2">
        <span class="min-w-0 flex-1 text-ui-meta font-semibold">Filters</span>
        {#if filterActive}
          <Button
            class="h-6 flex-none rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
            variant="ghost"
            type="button"
            onclick={() => store.clearFilter()}
          >Clear all</Button>
        {/if}
        <Button
          class="size-6 flex-none rounded-sm p-0 text-muted-foreground"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label="Close filters"
          onclick={toggleFilters}
        >
          <Icon icon={iconClose} size={12} />
        </Button>
      </div>
      <label class="flex flex-col gap-1 text-ui-micro text-muted-foreground">
        Author
        <Input
          class="h-auto rounded-sm bg-card dark:bg-card px-2 py-1 text-ui-meta md:text-ui-meta placeholder:text-muted-foreground/80"
          placeholder="Author"
          aria-label="Filter by author"
          value={store.filter.author}
          oninput={(e) => store.setFilter({ author: e.currentTarget.value })}
        />
      </label>
      <label class="flex flex-col gap-1 text-ui-micro text-muted-foreground">
        Path
        <Input
          class="h-auto rounded-sm bg-card dark:bg-card px-2 py-1 text-ui-meta md:text-ui-meta placeholder:text-muted-foreground/80"
          placeholder="Path (comma-separates)"
          aria-label="Filter by path (comma or newline separated for multiple)"
          value={store.filter.path}
          oninput={(e) => store.setFilter({ path: e.currentTarget.value })}
        />
        {#if pathFilterCount > 1}
          <!-- Multi-pathspec walk: the backend ignores rename-following there
               (no single name to shift through renames) — say so up front. -->
          <Hint label="Rename-following is off while filtering multiple paths (a multi-path walk has no single name to follow through renames)">
            {#snippet child({ props })}
              <span
                {...props}
                class="inline-flex flex-none items-center self-start rounded-full bg-muted px-1.5 py-0 text-ui-micro leading-[1.4] text-muted-foreground"
              >{pathFilterCount} paths · follow off</span>
            {/snippet}
          </Hint>
        {/if}
      </label>
      <div class="flex gap-2">
        <label class="flex min-w-0 flex-1 flex-col gap-1 text-ui-micro text-muted-foreground">
          After
          <Input
            type="date"
            class="h-auto rounded-sm bg-card dark:bg-card px-2 py-1 text-ui-meta md:text-ui-meta"
            aria-label="Commits after this date"
            value={store.filter.after}
            onchange={(e) => store.setFilter({ after: e.currentTarget.value })}
          />
        </label>
        <label class="flex min-w-0 flex-1 flex-col gap-1 text-ui-micro text-muted-foreground">
          Before
          <Input
            type="date"
            class="h-auto rounded-sm bg-card dark:bg-card px-2 py-1 text-ui-meta md:text-ui-meta"
            aria-label="Commits before this date"
            value={store.filter.before}
            onchange={(e) => store.setFilter({ before: e.currentTarget.value })}
          />
        </label>
      </div>
      <label class="flex flex-col gap-1 text-ui-micro text-muted-foreground">
        Ref scope
        <select
          class="h-auto rounded-sm border border-input bg-card px-2 py-1 text-ui-meta text-foreground focus-visible:outline-2 focus-visible:outline-ring focus-visible:-outline-offset-1"
          aria-label="Branch scope"
          bind:this={scopeSelectEl}
          onchange={(e) => store.setFilter({ scopeRef: e.currentTarget.value })}
        >
          <option value="">All branches</option>
          <option value="HEAD">Current branch</option>
          {#each scopeOptions as name (name)}
            <option value={name}>{name}</option>
          {/each}
        </select>
      </label>
      <div class="flex flex-col gap-2 border-t border-border pt-2.5">
        <!-- M9: regex mode is live (backend `LogFilter.regex`). -->
        <Hint label="Treat the search text as a regular expression">
          {#snippet child({ props })}
            <label
              {...props}
              class="regex flex cursor-pointer items-center gap-2 text-ui-meta text-muted-foreground"
            >
              <Checkbox
                checked={store.filter.regex}
                onCheckedChange={(checked) => store.setFilter({ regex: checked === true })}
                aria-label="Treat the search text as a regular expression"
              />
              <span>Regex</span>
            </label>
          {/snippet}
        </Hint>
        <!-- M10: pickaxe -S — commits whose patch adds/removes the string. -->
        <Input
          class="f h-auto rounded-sm bg-card dark:bg-card px-2 py-1 text-ui-meta md:text-ui-meta placeholder:text-muted-foreground/80"
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
          class="f h-auto rounded-sm bg-card dark:bg-card px-2 py-1 text-ui-meta md:text-ui-meta placeholder:text-muted-foreground/80"
          placeholder="patch regex (-G)"
          aria-label="Pickaxe: patches adding or removing lines matching this regex"
          value={store.filter.pickaxeRegex ?? ""}
          oninput={(e) => store.setFilter({ pickaxeRegex: e.currentTarget.value })}
        />
      </div>
    </div>
  {/if}

  {#if refsPopover !== null}
    {@const pop = refsPopover}
    <!-- Workstream D §8.2: the `+n` chip's popover — ALL of the commit's
         refs, each with Copy and (for commits HEAD is not on) checkout
         through the same confirm dialog as the row hover action. -->
    <div
      bind:this={refsEl}
      class="refs-popover fixed z-60 flex w-72 max-w-[calc(100vw-1rem)] flex-col rounded-lg border border-border bg-popover text-ui-base text-popover-foreground shadow-md"
      role="dialog"
      aria-label={`Refs — commit ${pop.sha.slice(0, 7)}`}
      use:floating={{ anchor: refsAnchor ?? undefined, alignEnd: true, above: true, gap: 6 }}
    >
      <div class="flex flex-none items-center gap-2 border-b border-border px-3 py-1.5">
        <span class="min-w-0 flex-1 truncate text-ui-meta font-semibold">Refs — {pop.sha.slice(0, 7)}</span>
        <Button
          class="size-6 flex-none rounded-sm p-0 text-muted-foreground"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label="Close refs"
          onclick={closeRefsPopover}
        >
          <Icon icon={iconClose} size={12} />
        </Button>
      </div>
      <ul class="refs-list max-h-56 overflow-y-auto p-1" aria-label="All refs">
        {#each pop.refs as ref (ref)}
          <li class="flex items-center gap-1 rounded-sm px-1.5 py-1 hover:bg-accent">
            <RefBadge {ref} compact maxWidthClass="max-w-36" />
            <span class="min-w-0 flex-1"></span>
            <Hint label={`Copy ${shortRefName(ref)}`} side="left">
              {#snippet child({ props })}
                <button
                  {...props}
                  type="button"
                  class="inline-flex size-5 flex-none cursor-pointer items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                  aria-label={`Copy ${shortRefName(ref)}`}
                  onclick={() => void copyRef(ref)}
                >
                  <span class="text-ui-micro font-semibold" aria-hidden="true">⧉</span>
                </button>
              {/snippet}
            </Hint>
            {#if pop.sha !== headSha && classifyRef(ref) !== "head"}
              {@const isLocal = classifyRef(ref) === "local"}
              {@const refName = shortRefName(ref)}
              <Hint
                label={isLocal ? `Switch to ${refName}` : "Checkout commit (detached HEAD)"}
                side="left"
              >
                {#snippet child({ props })}
                  <button
                    {...props}
                    type="button"
                    class="inline-flex size-5 flex-none cursor-pointer items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                    aria-label={isLocal ? `Switch to ${refName}` : "Checkout commit (detached HEAD)"}
                    onclick={() => {
                      refsPopover = null;
                      checkoutRef(pop.sha, ref);
                    }}
                  >
                    <Icon icon={iconCheckout} size={14} />
                  </button>
                {/snippet}
              </Hint>
            {/if}
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</section>
