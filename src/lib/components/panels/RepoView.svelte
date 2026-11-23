<script module lang="ts">
  /**
   * Panel chunks load when their snippet first renders, not at module eval —
   * a module-scope `import()` promise starts fetching every panel the moment
   * the RepoView chunk lands, which defeats the split (terminal/xterm, the
   * history graph and the forge would all load on first tab open).
   *
   * The cached-promise pattern stays: inside a reactive snippet a fresh
   * `import()` call creates a new promise on every evaluation, and the
   * surrounding {#await} tears the panel down and remounts it — the file
   * tree re-listed its directories on every status snapshot (each listing
   * used to open a fresh SFTP channel on remote repos). A stable promise
   * reference renders the await block once.
   */
function lazyImport<T>(load: () => Promise<T>): () => Promise<T> {
	let cached: Promise<T> | null = null;
	return () => (cached ??= load());
}
const filesPanelImport = lazyImport(() => import("$lib/components/panels/FilesPanel.svelte"));
const branchPanelImport = lazyImport(() => import("$lib/components/panels/BranchPanel.svelte"));
const remotePanelImport = lazyImport(() => import("$lib/components/panels/RemotePanel.svelte"));
const stashPanelImport = lazyImport(() => import("$lib/components/panels/StashPanel.svelte"));
const worktreePanelImport = lazyImport(() => import("$lib/components/panels/WorktreePanel.svelte"));
const repoPanelImport = lazyImport(() => import("$lib/components/panels/RepoPanel.svelte"));
const submodulePanelImport = lazyImport(() => import("$lib/components/panels/SubmodulePanel.svelte"));
const reflogPanelImport = lazyImport(() => import("$lib/components/panels/ReflogPanel.svelte"));
const undoPanelImport = lazyImport(() => import("$lib/components/safety/UndoPanel.svelte"));
const historyViewImport = lazyImport(() => import("$lib/components/panels/HistoryView.svelte"));
const terminalPanelImport = lazyImport(() => import("$lib/components/terminal/TerminalPanel.svelte"));
const forgePanelImport = lazyImport(() => import("$lib/components/forge/ForgePanel.svelte"));
const actionsPanelImport = lazyImport(() => import("$lib/components/actions/ActionsPanel.svelte"));
const statsPanelImport = lazyImport(() => import("$lib/components/panels/StatsPanel.svelte"));
const assistantPanelImport = lazyImport(() => import("$lib/components/ai/AssistantPanel.svelte"));
const quickOpenImport = lazyImport(() => import("$lib/components/files/QuickOpen.svelte"));
</script>

<script lang="ts">
  /**
   * Per-tab repo workspace (M2, extended in M3, layout-configurable in M4):
   * header facts + sync quick actions + the configurable main splitter tree
   * (SplitContainer).
   * The default tree is VSCode-like: a narrow source-control sidebar
   * (StatusPanel above a pinned CommitBar) on the left, the Diff panel as the
   * center editor, and a collapsible History sidebar on the right. The
   * VSCode-style activity bar at the far left switches primary-sidebar
   * panels; collapsing either sidebar keeps it mounted and replaces it with
   * a rail (the activity bar itself on the left, a vertical History tab on
   * the right), so state survives and the center content grows.
   *
   * M4 layout: the main area renders the active layout tree from the
   * `layouts` store (per-repo overlay > preset > default). Panels are
   * mapped PanelId → `{#snippet}` here — the only place that knows both
   * sides of the registry (see `$lib/layout/index.ts`). Tab groups get
   * context menus (move / new group / hide) and a "+ Add panel" menu; the
   * header exposes the preset select, "Save layout as…", "Add panel" and
   * per-repo reset.
   *
   * The center editor owns no state itself (the `diff` panel may be moved or
   * re-parented): selection + streamed working-copy diff live here, and the
   * history sidebar reports its commit selection upward.
   *
   * M3 conflict flow (unchanged): while `repo_status` reports a merge /
   * rebase / sequencer operation in progress, the per-repo conflicts cache
   * polls `conflicts(repoId)`; when files come back, a banner across the
   * top offers Resolve (ConflictEditor overlay) and Abort (rebase_abort /
   * merge_abort / sequencer_abort, chosen by the in-progress op).
   */
  import { untrack, type Snippet } from "svelte";
  import type {
    CommitInfo,
    ConflictFile,
    FileDiff,
    FileMeta,
    RepoStatus,
    StatusEntry,
  } from "$lib/ipc/types";
  import {
    diffScopeForEntry,
    diffScopeSides,
    type DiffScope,
  } from "$lib/components/panels/statusModel";
  import {
    activeDiffTab,
    diffPageForOpenPaths,
    neighborDiffPath,
    upsertDiffTab,
    withoutDiffTab,
    type DiffTab,
  } from "$lib/components/panels/diffTabs";
  import {
    discard,
    mergeAbort,
    rebaseAbort,
    sequencerAbort,
    stage,
    statFile,
    streamDiff,
  } from "$lib/ipc/client";
  import type {
    OpenFileInfo,
    WorkspaceCapture,
  } from "$lib/components/files/FileWorkspace.svelte";
  import StatusPanel from "$lib/components/panels/StatusPanel.svelte";
  import ConflictEditor from "$lib/components/merge/ConflictEditor.svelte";
  import BisectBanner from "$lib/components/bisect/BisectBanner.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import DialogShell from "$lib/components/safety/DialogShell.svelte";
  import { trapTab } from "$lib/components/safety/trapTab";
  import { forgeStore } from "$lib/stores/forge.svelte";
  import { forgeChips } from "$lib/forge/chips.svelte";
  import {
    hostOfRemoteUrl,
    loadForgeRemote,
    saveForgeRemote,
  } from "$lib/forge/remotePrefs";
  import { remotes as listRemotes } from "$lib/ipc/client";
  import type { RemoteInfo } from "$lib/ipc/types";
  import { rebaseMonitor } from "$lib/components/rebase/rebaseStore.svelte";
  import EditorPane from "$lib/components/panels/EditorPane.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import type { CompareResult } from "$lib/components/panels/compare";
  import type { HistoryCommitActions } from "$lib/components/panels/historySidebar";
  import AuthDialog from "$lib/components/AuthDialog.svelte";
  import CleanDialog from "$lib/components/actions/CleanDialog.svelte";
  import RepoHeader from "$lib/components/header/RepoHeader.svelte";
  // Startup: every panel that is not part of the first paint (status is the
  // default tab; diff is always visible) is dynamically
  // imported inside its snippet below — hidden panels (terminal/xterm,
  // forge, stats, history's graph, …) and their dependency trees only load
  // when first activated, which keeps the launch-critical module graph
  // small. Imports are cached, so later activations are synchronous-fast.
  import { ai } from "$lib/ai/ai.svelte";
  import SplitContainer from "$lib/components/layout/SplitContainer.svelte";
  import ActivityBar from "$lib/components/layout/ActivityBar.svelte";
  import { conflictsStore } from "$lib/components/panels/conflictsStore.svelte";
  import { emitUiEvent, onUiEvent } from "$lib/palette/events";
  import {
    conflictAbortCommand,
    conflictSource,
    conflictSourceLabel,
  } from "$lib/components/panels/panelModel";
  import {
    firstTabsId,
    findPanelNode,
    firstSidebarHolder,
    leftSiblingOf,
    moveToLeftGroup,
    moveToRightGroup,
    nodeById,
    nodeIds,
    PANEL_META,
    rightSiblingOf,
    setRatio,
    TREE_PANELS,
    visiblePanels,
    type LayoutNode,
    type PanelGroupAction,
    type PanelId,
  } from "$lib/layout/layoutModel";
  import { layouts } from "$lib/layout/layout.svelte";
  // §10 responsive matrix: per-mode overlay drawers + the reactive mode.
  import { widthMode } from "$lib/layout/widthMode.svelte";
  import {
    drawerPanelLabel,
    isDrawerPanel,
    planDrawerPanels,
    type DrawerBinding,
    type DrawerPanel,
    type DrawerSide,
  } from "$lib/layout/drawerPlan";
  import { workspaceDrawers } from "$lib/layout/workspaceDrawers.svelte";
  // §10 Stacked (<800): the full-screen-route fallback plan + its store.
  import {
    STACKED_DEFAULT_VIEW,
    STACKED_PRIMARY_VIEWS,
    stackedMorePanels,
    stackedViewLabel,
  } from "$lib/layout/stackedPlan";
  import { workspaceStacked } from "$lib/layout/workspaceStacked.svelte";
  import StackedTabBar from "$lib/components/layout/StackedTabBar.svelte";
  import { createWidthTracker } from "$lib/layout/containerWidth.svelte";
  import { tick } from "svelte";
  import { actionsStore } from "$lib/stores/actions.svelte";
  import { stashActionRun } from "$lib/components/actions/runPopout";
  import { terminals } from "$lib/terminal/terminalStore.svelte";
  import { openPanelPopout } from "$lib/layout/popout";
  import { bookmarks } from "$lib/components/graph/bookmarks.svelte";
  import {
    branchColorForRefs,
    branchColorStore,
  } from "$lib/stats/branchColors.svelte";
  import {
    refreshStatus,
    showRecentRepos,
    tabStore,
  } from "$lib/stores/tabs.svelte";
  import { loadHistoryPrefs } from "$lib/stores/historyPrefs";
  import {
    diffRowKey,
    diffScrollKey,
    fileScrollKey,
    workspaceSession,
    type SessionCenterKind,
    type StoredDiffRef,
  } from "$lib/stores/workspaceSession.svelte";
  import { diffPrefs } from "$lib/diff/diffPrefs.svelte";
  import { railPins } from "$lib/stores/railPins.svelte";
  import { startAuthEvents, startOpsEvents } from "$lib/stores/ops.svelte";
  import { autofetch } from "$lib/stores/autofetch.svelte";
  import { appSettings } from "$lib/state/appSettings.svelte";
  import { toast } from "$lib/toast";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import type { HeaderAction } from "$lib/components/header/headerActions";

  let {
    repoId,
    name,
    root,
    status,
    deviceId = null,
  }: {
    repoId: string;
    name: string;
    root: string;
    status: RepoStatus | null;
    /** Owning device for remote repositories (`null` = local). */
    deviceId?: string | null;
  } = $props();

  // Icons for the header overflow menu; the header row's own glyphs live in
  // `header/RepoHeader.svelte`.
  const iconBisect = $derived(appIcon("alt-route")!);
  const iconForgeRemote = $derived(
    appIcon("pull-request") ?? appIcon("merge-type")!,
  );
  const iconSaveLayout = $derived(appIcon("save")!);
  const iconClean = $derived(appIcon("cleaning-services")!);
  const iconResetLayout = $derived(appIcon("restart-alt")!);
  const iconHome = $derived(appIcon("home-rounded")!);

  let selected: string[] = $state([]);
  let hasSubmodules = $state(false);

  // --- Center editor state -------------------------------------------------

  /**
   * The center editor's mode — ONE state machine, ONE `$state`. Every mode
   * change is a single assignment of this union (impossible to forget a
   * sibling reset), and only one mode can be active at a time. It replaces
   * six parallel nullable fields whose combinations were resolved by
   * EditorPane's priority if-chain — the encoding that let repo A's file
   * tabs survive a switch to repo B (state-machine audit, 2026-11).
   */
  type CenterPane =
    | { kind: "empty" }
    | { kind: "diff"; tabs: DiffTab[]; activePath: string }
    | { kind: "commit"; commit: CommitInfo }
    | { kind: "compare"; result: CompareResult }
    | { kind: "workflowRun"; runId: number }
    | { kind: "actionRun"; runId: string }
    | { kind: "files" };
  const EMPTY_PANE: CenterPane = { kind: "empty" };

  /** The center editor's mode (see {@link CenterPane}). */
  let center = $state<CenterPane>({ kind: "empty" });

  // Per-mode projections. Reads keep the old names (the template and the
  // diff-stream plumbing below are unchanged); writes go through `center`.
  // The diff mode holds a TAB LIST (like the file workspace): `diffEntry` /
  // `diffScope` are the ACTIVE tab's entry + comparison, so every existing
  // consumer (entry refresh, prev/next, hunk actions, session sync) keeps
  // its single-diff semantics.
  const diffTabList = $derived(center.kind === "diff" ? center.tabs : []);
  const diffEntry = $derived.by<StatusEntry | null>(() => {
    if (center.kind !== "diff") return null;
    return activeDiffTab(center.tabs, center.activePath)?.entry ?? null;
  });
  /** Which working-copy comparison the open diff shows (staged or unstaged). */
  const diffScope = $derived.by<DiffScope>(() => {
    if (center.kind !== "diff") return "unstaged";
    return activeDiffTab(center.tabs, center.activePath)?.scope ?? "unstaged";
  });
  const selectedCommit = $derived(
    center.kind === "commit" ? center.commit : null,
  );
  const compareResult = $derived(
    center.kind === "compare" ? center.result : null,
  );
  const workflowRunId = $derived(
    center.kind === "workflowRun" ? center.runId : null,
  );
  const actionRunId = $derived(center.kind === "actionRun" ? center.runId : null);
  const filesMode = $derived(center.kind === "files");

  // Diff-stream plumbing (output state of the working-copy diff stream —
  // only meaningful while `center` is in the "diff" mode).
  let diffRepoId = $state<string | null>(null);
  let diffRoot = $state<string | null>(null);
  let diffRepoName = $state<string | null>(null);
  let diffFiles: FileDiff[] = $state([]);
  let diffLoading = $state(false);
  /** Failure message from the last diff load (null = none): a dropped
   *  backend handle must render as an error, not as "No changes". */
  let diffError = $state<string | null>(null);
  /** Multi-selection action toolbar state reported by the history sidebar. */
  let historyActions = $state<HistoryCommitActions | null>(null);
  /** Commit to reveal/select in history (parent navigation from the editor). */
  let pendingHistorySha = $state<string | null>(null);
  /** Guards the working-copy diff stream against stale pages. */
  let diffToken = 0;
  /**
   * The sides the current `diffFiles` batch was streamed for. Tabs carry
   * their own scope (a staged row next to an unstaged one opens both), but
   * one stream answers ONE sides pair — activating a tab whose scope
   * differs re-streams under its sides (see {@link activateDiffTab}).
   */
  let diffStreamScope = $state<DiffScope>("unstaged");

  // --- File workspace state (open workdir files, VS Code-style) -------------

  /** Open files: tab order = open order; each keeps its own dirty/meta. */
  let openFiles = $state<OpenFileInfo[]>([]);
  /** Active (visible) workspace tab. */
  let activeFile = $state<string | null>(null);
  /** Quick-open (Ctrl+P) overlay visibility. */
  let quickOpenVisible = $state(false);

  // --- Workspace session (per-root persistence across switches/relaunch) -----

  // The session store is the durable home of the workspace state below:
  // every mutation point mirrors into it (`syncSession`), the repo-switch
  // effect restores from it. The fields here stay the RUNTIME truth — the
  // session only ever observes them.

  /** Root the switch effect last saved/restored for (null = mount). */
  let lastSessionRoot: string | null = null;
  /** Guards a restore against a rapid A→B→A switch (stale continuation). */
  let restoreToken = 0;
  /** FileWorkspace capture API (dirty doc text), attached by the panel. */
  let workspaceCapture: WorkspaceCapture | null = null;
  /**
   * Editor seed for the restore window: dirty-buffer doc text (memory only)
   * + saved scroll positions, keyed `file:<path>`, consumed by the
   * FileEditors the restore mounts; cleared once they all loaded.
   */
  let editorSeed = $state<{
    docs: Record<string, string>;
    scrolls: Record<string, number>;
  } | null>(null);
  /** Stored diffs whose entries wait for the new repo's status to land. */
  let pendingDiffRestore = $state<{
    refs: StoredDiffRef[];
    activePath: string | null;
  } | null>(null);
  /** Scroll the next-mounted DiffViewer restores (null = top, as always). */
  let diffScrollSeed = $state<number | null>(null);
  /**
   * First-visible-row the next-mounted DiffViewer restores (null = top).
   * Row seeds survive wrap/mode height changes that drift px offsets; the
   * px seed stays for payloads saved before rows were recorded.
   */
  let diffRowSeed = $state<number | null>(null);

  /**
   * Mirrors the live workspace state into the current root's session. One
   * write-through for every mutation point, so the session can never drift
   * from the runtime state it restores. Transient center views (compare,
   * run logs) record as "empty": they need not persist.
   */
  function syncSession(): void {
    if (root === "") return;
    // Transient center views (compare, run logs) record as "empty": they
    // need not persist, and coming back lands on an empty editor instead
    // of resurrecting a stale one.
    const centerKind: SessionCenterKind =
      center.kind === "files"
        ? "files"
        : center.kind === "diff"
          ? "diff"
          : center.kind === "commit"
            ? "commit"
            : "empty";
    workspaceSession.patch(root, {
      openFiles: openFiles.map((f) => f.path),
      activeFile,
      centerKind,
      diffs:
        center.kind === "diff"
          ? center.tabs.map((t) => ({ path: t.entry.path, scope: t.scope }))
          : [],
      activeDiffPath: center.kind === "diff" ? center.activePath : null,
      commitSha: center.kind === "commit" ? center.commit.sha : null,
    });
    workspaceSession.setCommit(
      root,
      center.kind === "commit" ? center.commit : null,
    );
  }

  /** Scroll capture from the file workspace (event-driven, session write). */
  function onFileScrollCapture(path: string, top: number): void {
    if (root === "") return;
    workspaceSession.setScroll(root, fileScrollKey(path), top);
  }

  /** Scroll capture from the open working-copy diff (px + first row). */
  function onDiffScrollCapture(top: number, firstRow: number): void {
    const path = diffEntry?.path;
    if (path === undefined || root === "") return;
    workspaceSession.setScroll(root, diffScrollKey(path), top);
    workspaceSession.setScroll(root, diffRowKey(path), firstRow);
  }

  /** The saved session first-visible-row of a diff path (tab restore). */
  function rowSeedFor(path: string): number | null {
    if (root === "") return null;
    return workspaceSession.get(root)?.scrolls[diffRowKey(path)] ?? null;
  }

  /**
   * The ignore-whitespace preference flipped in a diff toolbar (typed bus):
   * re-stream the workspace diff so the backend query honors the new flag
   * (the viewer's row model drops the ws-only zero-hunk file entries once
   * the fresh pages land).
   */
  $effect(() => {
    return onUiEvent("diff-ignore-ws-changed", () => {
      if (center.kind !== "diff") return;
      void streamDiffTabs(
        center.tabs,
        center.activePath,
        diffRepoId ?? undefined,
        diffRoot ?? undefined,
        diffRepoName ?? undefined,
        scrollSeedFor(center.activePath),
        rowSeedFor(center.activePath),
      );
    });
  });

  /** Stable attach callback: the workspace registers its capture API once. */
  function attachCapture(api: WorkspaceCapture | null): void {
    workspaceCapture = api;
  }

  /** Open (or focus) a workdir file in the center workspace. */
  async function openWorkdirFile(path: string): Promise<void> {
    // The editor surfaces through the drawers' edges — yield both edge
    // overlays so the opened file is unobstructed (§10 Medium/Narrow).
    if (drawerPanelIds.has("files")) closeDrawer("files");
    if (drawerPanelIds.has("history")) closeDrawer("history");
    const existing = openFiles.find((f) => f.path === path);
    if (existing) {
      activeFile = path;
      center = { kind: "files" };
      syncSession();
      return;
    }
    const file: OpenFileInfo = { path, dirty: false, meta: null };
    openFiles.push(file);
    activeFile = path;
    // Single mode transition: the workspace replaces whatever was showing.
    center = { kind: "files" };
    syncSession();
    try {
      const meta: FileMeta = await statFile(repoId, path);
      // Write through the proxied array: the local `file` is the raw
      // pre-push object, and mutating it fires no signals (the pane
      // would stay on the "Reading…" spinner). Re-finding also skips
      // the update when the tab was closed during the stat round-trip.
      const target = openFiles.find((f) => f.path === path);
      if (target) target.meta = meta;
    } catch (err) {
      toast(String(err), { kind: "error" });
      closeWorkdirFile(path);
    }
  }

  function closeWorkdirFile(path: string): void {
    const idx = openFiles.findIndex((f) => f.path === path);
    if (idx !== -1) openFiles.splice(idx, 1);
    if (activeFile === path) {
      activeFile =
        openFiles[Math.min(idx, openFiles.length - 1)]?.path ?? null;
    }
    if (openFiles.length === 0 && center.kind === "files") {
      center = EMPTY_PANE;
    }
    // Drop the tab's saved scroll + buffer with it — a later restore must
    // not resurrect a file the user dismissed.
    if (root !== "") workspaceSession.forgetFile(root, path);
    syncSession();
  }

  function workdirDiffFor(path: string): void {
    // Surfacing the editor yields the edge drawer overlays (§10).
    if (drawerPanelIds.has("files")) closeDrawer("files");
    if (drawerPanelIds.has("history")) closeDrawer("history");
    const entry =
      status?.entries.find((e) => e.path === path || e.old_path === path) ??
      null;
    if (!entry) {
      toast(`No working-copy changes for ${path}`);
      return;
    }
    void openDiff(entry, undefined, undefined, undefined, diffScopeForEntry(entry));
  }

  // --- Layout ---------------------------------------------------------------

  // Hydrate this repo's persisted overlay (never inside `$derived` — the
  // store writes `$state`); the derived below re-runs once it lands.
  $effect(() => {
    layouts.ensureRepo(root);
  });

  // M7: hydrate the per-root bookmark + branch-color stores the same way
  // (the history graph consumes both — see the historyPanel snippet).
  $effect(() => {
    bookmarks.ensure(root);
    branchColorStore.ensure(root);
  });

  /** The active layout for this repo (overlay > preset > default). */
  const resolved = $derived(layouts.active(root));
  /** Resolved once per rule change, not once per painted graph row. */
  const branchRules = $derived(branchColorStore.rules(root));
  /** Panels not currently placed (candidates for "Add panel" menus). */
  const hiddenPanels = $derived(
    TREE_PANELS.filter((p) => !visiblePanels(resolved.layout.main).includes(p)),
  );

  /**
   * Primary sidebar holder: the leftmost leaf/tab group that is not just the
   * center editor. The activity bar switches and toggles this group.
   */
  const activityGroup = $derived(firstSidebarHolder(resolved.layout.main));

  /** Active panel in the activity-controlled sidebar. */
  const activityPanel = $derived.by<PanelId | null>(() => {
    const group = activityGroup;
    if (!group) return null;
    if (group.kind === "leaf") return group.panel;
    return (
      group.tabs[Math.min(Math.max(group.active, 0), group.tabs.length - 1)] ??
      null
    );
  });

  /**
   * The panel active before the current one (`undefined` until the first
   * effect run — the mount value must not count as a transition).
   */
  let lastActivityPanel: PanelId | null | undefined = undefined;

  // Switching TO the files panel reveals what the center shows: the open
  // diff's file, else the active workspace tab. Fires on the transition
  // only — rail clicks, tab-group activations and palette focus all land
  // here; re-renders while files is already active never re-emit, so the
  // user's manual tree selection is not fought.
  $effect(() => {
    const panel = activityPanel;
    const previous = lastActivityPanel;
    lastActivityPanel = panel;
    if (panel !== "files" || previous === undefined || previous === "files") {
      return;
    }
    const path = diffEntry?.path ?? activeFile;
    if (path !== null) emitUiEvent("files-reveal-path", { repoId, path });
  });

  /** History holder (the right sidebar in the default layout). */
  const historyNode = $derived(findPanelNode(resolved.layout.main, "history"));
  /** Whether history lives in the activity-controlled (primary) sidebar. */
  const historyInActivityGroup = $derived(
    historyNode !== null &&
      activityGroup !== null &&
      historyNode.id === activityGroup.id,
  );
  /** History outside the primary sidebar (normally the right column). */
  const historyInRightSidebar = $derived(
    historyNode !== null && !historyInActivityGroup,
  );
  const activityGroupPanels = $derived(
    activityGroup ? visiblePanels(activityGroup) : [],
  );
  const historyPanels = $derived(historyNode ? visiblePanels(historyNode) : []);

  // --- §10 responsive drawers (Medium/Narrow overlay conversion) ------------
  //
  // The matrix drawerizes FILES (Medium+Narrow, left), STATUS (Narrow,
  // right) and HISTORY (Medium+Narrow, right). A drawer is the SAME
  // keep-alive host element the docked pane renders, repositioned with
  // `position: absolute` (see drawerPlan.drawerPaneClass) — docking and
  // drawerizing only ever swap classes, never remount a panel, so mode
  // flips preserve editor/scroll/keep-alive state (§10 mechanics).
  //
  // Stacked (<800) rides the same host machinery (see the stacked section
  // below): the workspace shows ONE full-screen region at a time, selected
  // through the bottom tab bar.

  /** The reactive width mode (mirrors `<html data-width-mode>`). */
  const currentWidthMode = $derived(widthMode.current);

  // --- §10 Stacked (<800): full-screen routes over the same hosts ----------
  //
  // Below the floor the workspace stops splitting: ONE region is visible,
  // chosen by the bottom tab bar (the ActivityBar's stacked replacement).
  // Every visible panel is "stacked-bound" — its keep-alive host presents
  // as a full-screen route (`stacked: true` bindings riding the SAME
  // drawerPanes/drawerPanels channels as the Medium/Narrow drawers), so a
  // mode flip swaps classes only and panels never remount. Views behind
  // "More" are placed into the tree on selection, exactly like the rail
  // does in the wider modes.

  /**
   * The active stacked view while the mode is Stacked (`null` otherwise —
   * every stacked derivation keys off this one read of the store).
   */
  const stackedView = $derived(
    currentWidthMode === "stacked" ? workspaceStacked.active : null,
  );

  /** One full-screen binding for a stacked view's host. */
  function makeStackedBinding(panel: PanelId, key: string): DrawerBinding {
    return {
      key,
      open: workspaceStacked.active === panel,
      label: `${stackedViewLabel(panel)} view`,
      // Stacked hosts have no Esc/edge semantics — the no-op handlers
      // keep DrawerBinding's required shape without drawer behavior.
      onKeydown: () => {},
      onPointerdown: () => {},
      stacked: true,
    };
  }

  /** Pane bindings (leaf holders — the editor/history columns). */
  const stackedPaneBindings = $derived.by(() => {
    const out: DrawerBinding[] = [];
    if (stackedView === null) return out;
    for (const panel of visiblePanels(resolved.layout.main)) {
      const holder = findPanelNode(resolved.layout.main, panel);
      if (holder?.kind === "leaf") {
        out.push(makeStackedBinding(panel, holder.id));
      }
    }
    return out;
  });

  /** Panel bindings (tab-group members — the primary sidebar's group). */
  const stackedTabBindings = $derived.by(() => {
    const out: DrawerBinding[] = [];
    if (stackedView === null) return out;
    for (const panel of visiblePanels(resolved.layout.main)) {
      const holder = findPanelNode(resolved.layout.main, panel);
      if (holder?.kind === "tabs") {
        out.push(makeStackedBinding(panel, panel));
      }
    }
    return out;
  });

  /**
   * Stacked selection (bottom tab bar / More menu / focus events): places
   * the panel in the tree when the layout lacks it, keeps the underlying
   * group's active tab in step (widening later shows the same panel), and
   * switches the full-screen view.
   */
  function selectStackedView(panel: PanelId): void {
    const holder = findPanelNode(resolved.layout.main, panel);
    if (!holder) {
      layouts.addPanel(
        root,
        panel,
        activityGroup?.id ?? firstTabsId(resolved.layout.main) ?? undefined,
      );
    } else if (holder.kind === "tabs") {
      layouts.setActiveTab(root, holder.id, holder.tabs.indexOf(panel));
    }
    workspaceStacked.setActive(panel);
  }

  /**
   * The §10 matrix's bar inventory: the five primary views (fixed) plus
   * every other activity-bar panel behind "More" — the rail's whole
   * destination list stays reachable below the floor.
   */
  const stackedMorePanelsList = $derived.by(() =>
    stackedMorePanels(activityPanels),
  );

  /**
   * The mode's drawerized panels for the ACTIVE layout — the §10 plan
   * guarded so the workspace always keeps docked content (empty in Wide
   * and Stacked — Stacked drawerizes nothing, it stacks everything).
   */
  const drawerPanels = $derived(
    planDrawerPanels(currentWidthMode, visiblePanels(resolved.layout.main)),
  );

  /**
   * Overlay panel ids for membership checks (PanelId-keyed): the mode's
   * drawerized panels, plus — in Stacked — every visible panel (each one
   * presents as a full-screen route there).
   */
  const drawerPanelIds = $derived.by(() => {
    const ids = new Set<PanelId>(drawerPanels.map(({ panel }) => panel));
    if (stackedView !== null) {
      for (const panel of visiblePanels(resolved.layout.main)) ids.add(panel);
    }
    return ids;
  });

  /** Per-panel drawer open state (persisted per mode; see workspaceDrawers). */
  const drawerOpen = $derived.by(() => ({
    files: workspaceDrawers.isOpen("files"),
    status: workspaceDrawers.isOpen("status"),
    history: workspaceDrawers.isOpen("history"),
  }));

  /**
   * Holders of overlay-hosted panels. A collapsed pane hosting a drawer
   * must DRAIN (zero flex basis, still mounted) instead of `display: none`,
   * which would kill the absolutely-positioned drawer hosts inside. In
   * Stacked every panel's holder hosts a full-screen route — same rule.
   */
  const drawerHolderIds = $derived.by(() => {
    const ids = new Set<string>();
    for (const { panel } of drawerPanels) {
      const holder = findPanelNode(resolved.layout.main, panel);
      if (holder) ids.add(holder.id);
    }
    if (stackedView !== null) {
      for (const panel of visiblePanels(resolved.layout.main)) {
        const holder = findPanelNode(resolved.layout.main, panel);
        if (holder) ids.add(holder.id);
      }
    }
    return ids;
  });

  /**
   * Effective sidebar collapse state. The persisted flags express intent, but
   * a layout must never hide every pane: if both sidebars ask to collapse and
   * only the other one would remain, history stays visible. Keeping this in
   * one derived block avoids circular can-collapse checks.
   */
  const collapsed = $derived.by(() => {
    const panels = visiblePanels(resolved.layout.main);
    const leftPanels = activityGroupPanels;
    const rightPanels =
      historyInRightSidebar && historyNode ? historyPanels : [];
    let left =
      resolved.leftCollapsed &&
      activityGroup !== null &&
      panels.some((panel) => !leftPanels.includes(panel));
    let right =
      resolved.rightCollapsed &&
      rightPanels.length > 0 &&
      panels.some(
        (panel) =>
          !rightPanels.includes(panel) &&
          !(left && leftPanels.includes(panel)),
      );
    if (left && right) {
      const remaining = panels.some(
        (panel) =>
          !leftPanels.includes(panel) && !rightPanels.includes(panel),
      );
      // Keep the primary sidebar collapsed; history is the fallback content.
      if (!remaining) right = false;
    }
    return { left, right };
  });

  /** Whether the history panel is actually on screen (not behind a rail/tab).
   *  Drawerized history is "on screen" while its drawer is open (§10);
   *  in Stacked it is on screen while it is the active full-screen view. */
  const historyVisible = $derived.by(() => {
    if (stackedView !== null) return stackedView === "history";
    if (drawerPanelIds.has("history")) return drawerOpen.history;
    if (!historyNode) return false;
    if (historyInActivityGroup) {
      return !collapsed.left && activityPanel === "history";
    }
    if (collapsed.right) return false;
    if (historyNode.kind === "tabs") {
      const active = Math.min(
        Math.max(historyNode.active, 0),
        historyNode.tabs.length - 1,
      );
      return historyNode.tabs[active] === "history";
    }
    return true;
  });
  /**
   * Right-edge rail shown when the right sidebar is collapsed or absent.
   * With history drawerized, the rail becomes the drawer's reopen
   * affordance while the drawer is closed; while open the drawer overlays
   * the rail's column, so the rail hides (the ActivityBar item stays).
   * Stacked has no rail at all — the bottom tab bar owns every toggle.
   */
  const showHistoryRail = $derived.by(() => {
    if (stackedView !== null) return false;
    if (drawerPanelIds.has("history")) {
      return !drawerOpen.history && historyInRightSidebar;
    }
    return !historyNode || (historyInRightSidebar && collapsed.right);
  });

  /**
   * Can a sidebar collapse without emptying the workspace? Panels already
   * hidden behind the other rail do not count as remaining content.
   */
  const canCollapseLeft = $derived.by(() => {
    if (!activityGroup) return false;
    return visiblePanels(resolved.layout.main).some(
      (panel) =>
        !activityGroupPanels.includes(panel) &&
        !(collapsed.right && historyPanels.includes(panel)),
    );
  });

  const canCollapseRight = $derived.by(() => {
    if (!historyInRightSidebar || !historyNode) return false;
    return visiblePanels(resolved.layout.main).some(
      (panel) =>
        !historyPanels.includes(panel) &&
        !(collapsed.left && activityGroupPanels.includes(panel)),
    );
  });

  /** Hiding history must not empty the workspace either. Closing a drawer
   *  never can — drawerized history always hides safely (§10). */
  const canHideHistory = $derived(
    drawerPanelIds.has("history")
      ? true
      : historyInActivityGroup
        ? canCollapseLeft
        : canCollapseRight,
  );

  /**
   * Pane ids hidden by the two sidebar rails; mounted, just out of layout.
   * Holders of drawerized panels are EXCLUDED — `display: none` would kill
   * their absolutely-positioned drawer hosts — and drain instead (see
   * `drainedIds`).
   */
  const collapsedIds = $derived.by(() => {
    const ids: string[] = [];
    if (
      collapsed.left &&
      activityGroup &&
      !drawerHolderIds.has(activityGroup.id)
    ) {
      ids.push(activityGroup.id);
    }
    if (
      collapsed.right &&
      historyNode &&
      !ids.includes(historyNode.id) &&
      !drawerHolderIds.has(historyNode.id)
    ) {
      ids.push(historyNode.id);
    }
    return ids;
  });

  /**
   * Re-opens history (adding the panel for layouts that predate it) and
   * brings it to the front of whichever holder it lives in. Drawerized
   * history simply opens its drawer; Stacked makes it the active route.
   */
  function showHistory(): void {
    if (stackedView !== null) {
      selectStackedView("history");
      return;
    }
    if (drawerPanelIds.has("history")) {
      openDrawer("history");
      return;
    }
    if (!historyNode) {
      layouts.showRightPanel(root, "history", 0.68);
      layouts.setSidebarCollapsed(root, "right", false);
      return;
    }
    if (historyInActivityGroup) {
      layouts.setSidebarCollapsed(root, "left", false);
    } else {
      layouts.setSidebarCollapsed(root, "right", false);
    }
    if (historyNode.kind === "tabs") {
      layouts.setActiveTab(
        root,
        historyNode.id,
        historyNode.tabs.indexOf("history"),
      );
    }
  }

  function hideHistory(): void {
    if (stackedView !== null) {
      // No hidden state below the floor: "hiding" history switches to the
      // editor (§3 principle 4 — every view stays reachable via the bar).
      if (workspaceStacked.active === "history") selectStackedView("diff");
      return;
    }
    if (drawerPanelIds.has("history")) {
      closeDrawer("history");
      return;
    }
    if (!historyNode) return;
    if (historyInActivityGroup) {
      if (canCollapseLeft) layouts.setSidebarCollapsed(root, "left", true);
      return;
    }
    if (canCollapseRight) layouts.setSidebarCollapsed(root, "right", true);
  }

  function toggleHistory(): void {
    if (historyVisible) hideHistory();
    else showHistory();
  }

  /** Activity bar: toggle a drawerized panel's drawer (§10), else activate
   *  the panel in the primary sidebar, adding it when hidden. Stacked has
   *  no rail — the same intent selects the full-screen route. */
  function selectActivityPanel(panel: PanelId): void {
    if (stackedView !== null) {
      selectStackedView(panel);
      return;
    }
    if (isDrawerPanel(panel) && drawerPanelIds.has(panel)) {
      toggleDrawer(panel);
      return;
    }
    // Selecting a docked panel takes over the primary sidebar; when history
    // shares that group as a drawer, mirror the wide-mode behavior where
    // selecting another of the group's tabs hides it.
    if (
      drawerPanelIds.has("history") &&
      drawerOpen.history &&
      historyInActivityGroup
    ) {
      closeDrawer("history");
    }
    const group = activityGroup;
    if (!collapsed.left && activityPanel === panel) {
      if (canCollapseLeft) layouts.setSidebarCollapsed(root, "left", true);
      return;
    }
    layouts.setSidebarCollapsed(root, "left", false);
    const holder = findPanelNode(resolved.layout.main, panel);
    if (holder) {
      if (holder.kind === "tabs") {
        layouts.setActiveTab(root, holder.id, holder.tabs.indexOf(panel));
      }
      return;
    }
    layouts.addPanel(root, panel, group?.id);
  }

  /** History rail: toggle the drawer (§10), the right sidebar, or activate
   *  it in-pane. (Stacked has no rail; the guard keeps the intent honest.) */
  function selectHistoryActivity(): void {
    if (stackedView !== null) {
      selectStackedView("history");
      return;
    }
    if (drawerPanelIds.has("history")) {
      toggleDrawer("history");
      return;
    }
    if (historyInActivityGroup) {
      selectActivityPanel("history");
      return;
    }
    if (historyVisible) hideHistory();
    else showHistory();
  }

  function toggleLeftSidebar(): void {
    if (collapsed.left) {
      layouts.setSidebarCollapsed(root, "left", false);
      return;
    }
    if (canCollapseLeft) layouts.setSidebarCollapsed(root, "left", true);
  }

  /** Panels with an activity-bar icon (diff is the center editor). */
  const activityPanels = TREE_PANELS.filter(
    (panel) => panel !== "diff" && panel !== "history",
  );

  // --- Drawer open/close + focus contract (OverlayDrawer parity, §11) -------

  /** Elements a press inside the drawer may land on without stealing focus. */
  const DRAWER_FOCUSABLE =
    "a[href], button:not([disabled]), textarea, input, select, iframe, [tabindex]:not([tabindex='-1'])";

  /**
   * The element focus returns to on close (the invoking toggle), captured
   * at open time like OverlayDrawer does.
   */
  let drawerReturnFocus: Partial<Record<DrawerPanel, HTMLElement | null>> = {};

  function openDrawer(panel: DrawerPanel): void {
    if (drawerOpen[panel]) return;
    // One drawer per window edge: STATUS and HISTORY both anchor right in
    // Narrow — opening one closes the other (FILES owns the left edge
    // alone). The closed drawer's state persists like any toggle.
    const side = drawerPanels.find((d) => d.panel === panel)?.side;
    if (side !== undefined) {
      for (const { panel: other, side: otherSide } of drawerPanels) {
        if (other !== panel && otherSide === side && drawerOpen[other]) {
          closeDrawer(other);
        }
      }
    }
    const active = document.activeElement;
    drawerReturnFocus[panel] =
      active instanceof HTMLElement && active !== document.body ? active : null;
    workspaceDrawers.setOpen(panel, currentWidthMode, true);
    // Focus moves into the drawer on open. On the FIRST open the host may
    // not exist yet (the panel mounts in the same flush), so the move waits
    // a tick and re-checks that the drawer is still open.
    void tick().then(() => {
      if (!workspaceDrawers.isOpen(panel)) return;
      document
        .querySelector<HTMLElement>(`[data-overlay-drawer="${panel}"]`)
        ?.focus();
    });
  }

  function closeDrawer(panel: DrawerPanel): void {
    if (!drawerOpen[panel]) return;
    workspaceDrawers.setOpen(panel, currentWidthMode, false);
    const back = drawerReturnFocus[panel] ?? null;
    drawerReturnFocus[panel] = null;
    back?.focus();
  }

  function toggleDrawer(panel: DrawerPanel): void {
    if (drawerOpen[panel]) closeDrawer(panel);
    else openDrawer(panel);
  }

  /**
   * Host keydown: Esc closes (scoped — the event only reaches the host
   * while focus is inside it, and nested floating layers mark Esc as
   * handled); Tab wraps within the drawer via the shared trap.
   */
  function drawerHostKeydown(
    panel: DrawerPanel,
  ): (event: KeyboardEvent) => void {
    return (event) => {
      if (event.key === "Escape") {
        if (event.defaultPrevented) return;
        event.stopPropagation();
        closeDrawer(panel);
      } else if (event.key === "Tab") {
        trapTab(event.currentTarget as HTMLElement, event);
      }
    };
  }

  /** Presses on the drawer's non-focusable surface anchor focus to the
   *  host, so the Esc close can never strand on `<body>` (OverlayDrawer
   *  parity). */
  function drawerHostPointerdown(event: PointerEvent): void {
    const target = event.target;
    if (target instanceof Element && target.closest(DRAWER_FOCUSABLE)) return;
    (event.currentTarget as HTMLElement).focus();
  }

  function makeDrawerBinding(
    panel: DrawerPanel,
    side: DrawerSide | undefined,
    key: string,
  ): DrawerBinding {
    return {
      key,
      side,
      open: workspaceDrawers.isOpen(panel),
      label: drawerPanelLabel(panel),
      onKeydown: drawerHostKeydown(panel),
      onPointerdown: drawerHostPointerdown,
    };
  }

  /**
   * Drawer host bindings. A LEAF holder drawerizes its whole pane through
   * SplitContainer (`drawerPanes` — the default layout's history column);
   * a tab-group member drawerizes its keep-alive host through TabGroup
   * (`drawerPanels` — FILES/STATUS in the default layout). Stacked mode
   * substitutes its full-screen route bindings (see the stacked section).
   */
  const drawerPaneBindings = $derived.by(() => {
    if (stackedView !== null) return stackedPaneBindings;
    const out: DrawerBinding[] = [];
    for (const { panel, side } of drawerPanels) {
      const holder = findPanelNode(resolved.layout.main, panel);
      if (holder?.kind === "leaf") {
        out.push(makeDrawerBinding(panel, side, holder.id));
      }
    }
    return out;
  });

  const drawerTabBindings = $derived.by(() => {
    if (stackedView !== null) return stackedTabBindings;
    const out: DrawerBinding[] = [];
    for (const { panel, side } of drawerPanels) {
      const holder = findPanelNode(resolved.layout.main, panel);
      if (holder?.kind === "tabs") {
        out.push(makeDrawerBinding(panel, side, panel));
      }
    }
    return out;
  });

  /**
   * Tab groups drained to zero flex basis (still mounted): their every
   * panel drawerized, or they are collapsed while hosting a drawer —
   * `display: none` would kill the drawer hosts, so the pane drains and
   * the hosts escape through `position: absolute`. Stacked drains EVERY
   * non-root node (the split wrappers too): no docked content remains, so
   * no empty percentage-sized column may linger under the full-screen
   * hosts.
   */
  const drainedIds = $derived.by(() => {
    const ids: string[] = [];
    for (const id of drawerHolderIds) {
      const holder = nodeById(resolved.layout.main, id);
      if (holder?.kind !== "tabs") continue; // leaf holders ARE the drawer
      const everyDrawerized =
        holder.tabs.length > 0 &&
        holder.tabs.every((panel) => drawerPanelIds.has(panel));
      const collapsedHere =
        (collapsed.left && activityGroup?.id === id) ||
        (collapsed.right && historyInRightSidebar && historyNode?.id === id);
      if (everyDrawerized || collapsedHere) ids.push(id);
    }
    if (currentWidthMode === "stacked") {
      const rootId = resolved.layout.main.id;
      for (const id of nodeIds(resolved.layout.main)) {
        if (id !== rootId && !ids.includes(id)) ids.push(id);
      }
    }
    return ids;
  });

  /**
   * Re-resolves the drawer open states when the width mode flips (per-mode
   * persistence; the first run seeds the initial mode's states). Stacked
   * hydrates its active view once (persisted, else the panel the user was
   * just reading in the wider mode — a still-open drawer wins over the
   * docked active tab).
   */
  const stackedFallbackView = $derived.by<PanelId>(() => {
    if (drawerOpen.files) return "files";
    if (drawerOpen.history) return "history";
    return activityPanel ?? STACKED_DEFAULT_VIEW;
  });

  $effect(() => {
    workspaceDrawers.refreshMode(widthMode.current);
    workspaceStacked.refresh(stackedFallbackView);
  });

  /**
   * A stacked view's panel must exist in the tree: a persisted view (or a
   * "More" pick the layout lacks) is added to the primary sidebar group,
   * exactly like the rail/focus paths do in the wider modes.
   */
  $effect(() => {
    if (currentWidthMode !== "stacked") return;
    const view = workspaceStacked.active;
    if (findPanelNode(resolved.layout.main, view) !== null) return;
    layouts.addPanel(
      root,
      view,
      activityGroup?.id ?? firstTabsId(resolved.layout.main) ?? undefined,
    );
  });

  // --- Mode transitions: a drawerized panel never stays the active tab ------
  //
  // When a group's active panel converts to a drawer (mode flip into
  // Medium/Narrow), the docked pane would show an empty head for a panel
  // that now floats as an overlay. Medium switches to the nearest docked
  // sibling (STATUS stays docked there, so the sidebar keeps content);
  // Narrow collapses the sidebar once per entry instead — the editor
  // reclaims the width, mirroring the history auto-collapse below (a
  // manual re-open always wins afterwards).

  let narrowLeftAutoCollapsed = $state(false);

  $effect(() => {
    const mode = currentWidthMode;
    if (mode !== "narrow") narrowLeftAutoCollapsed = false;
    const group = activityGroup;
    const panel = activityPanel;
    if (
      !group ||
      group.kind !== "tabs" ||
      panel === null ||
      !drawerPanelIds.has(panel)
    ) {
      return;
    }
    if (mode === "narrow") {
      if (!narrowLeftAutoCollapsed) {
        narrowLeftAutoCollapsed = true;
        layouts.setSidebarCollapsed(root, "left", true);
      }
      return;
    }
    // Medium: nearest docked sibling in tab order (after, else before).
    const index = group.tabs.indexOf(panel);
    let next: PanelId | null = null;
    for (let i = index + 1; i < group.tabs.length; i++) {
      if (!drawerPanelIds.has(group.tabs[i]!)) {
        next = group.tabs[i]!;
        break;
      }
    }
    if (next === null) {
      for (let i = index - 1; i >= 0; i--) {
        if (!drawerPanelIds.has(group.tabs[i]!)) {
          next = group.tabs[i]!;
          break;
        }
      }
    }
    // No docked sibling means every tab drawerized — the pane drains on
    // its own (drainedIds); there is nothing to switch to.
    if (next !== null) {
      layouts.setActiveTab(root, group.id, group.tabs.indexOf(next));
    }
  });

  // --- Narrow-viewport auto-collapse (UI review P1 #5) ----------------------
  //
  // Below 1200px the history sidebar would squeeze the diff into wrapping
  // code lines, so it auto-collapses to the right-edge rail once. Widening
  // never auto-expands and a manual reopen is respected for the rest of the
  // session (the flag only resets while the workspace is wide, so the NEXT
  // narrowing may auto-collapse again — never this one).
  //
  // §10: Medium/Narrow drawerize history — there is no docked right sidebar
  // to auto-collapse there, so the one-shot only applies to the docked
  // tiers (Wide never matches the query; Stacked keeps today's behavior).

  let narrowAutoCollapsed = $state(false);

  $effect(() => {
    if (drawerPanelIds.has("history")) return;
    const targetRoot = root;
    const mq = window.matchMedia("(max-width: 1199px)");
    const apply = (): void => {
      if (mq.matches) {
        if (
          !narrowAutoCollapsed &&
          !untrack(() => collapsed.right) &&
          untrack(() => historyInRightSidebar) &&
          untrack(() => canCollapseRight)
        ) {
          narrowAutoCollapsed = true;
          layouts.setSidebarCollapsed(targetRoot, "right", true);
        }
        return;
      }
      // Wide again: only re-arm the one-shot; pane visibility stays as the
      // user left it (manual overrides win for the session).
      narrowAutoCollapsed = false;
    };
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  });

  // --- Medium sidebar clamp (§10: STATUS "docked, narrower") ----------------

  /**
   * Medium caps the primary sidebar's WIDTH (the docked STATUS column stays
   * narrow next to the editor, whatever the window width within the tier).
   * The cap converts to a ratio against the measured split box (container
   * tracker, §4.2 convention) and is enforced on the rendered tree AND on
   * drag writes so the divider stops at it; Wide and Narrow keep the
   * persisted ratio untouched.
   */
  const MEDIUM_SIDEBAR_MAX_PX = 280;

  const layoutAreaTracker = createWidthTracker();

  /** Outermost split whose left subtree holds the primary sidebar. */
  function outermostLeftSplitId(
    tree: LayoutNode,
    groupId: string,
  ): string | null {
    if (tree.kind !== "split") return null;
    if (nodeById(tree.a, groupId) !== null) return tree.id;
    return (
      outermostLeftSplitId(tree.a, groupId) ??
      outermostLeftSplitId(tree.b, groupId)
    );
  }

  const sidebarSplitId = $derived.by(() => {
    const group = activityGroup;
    return group
      ? outermostLeftSplitId(resolved.layout.main, group.id)
      : null;
  });

  /** The ratio cap at the measured layout width (`null` = unmeasured/off). */
  const mediumSidebarCap = $derived.by(() => {
    if (currentWidthMode !== "medium") return null;
    const width = layoutAreaTracker.width;
    return width > 0 ? MEDIUM_SIDEBAR_MAX_PX / width : null;
  });

  /** The tree as rendered: the medium clamp applied (display-only overlay). */
  const renderTree = $derived.by(() => {
    const splitId = sidebarSplitId;
    const cap = mediumSidebarCap;
    if (splitId === null || cap === null) return resolved.layout.main;
    const split = nodeById(resolved.layout.main, splitId);
    if (!split || split.kind !== "split" || split.ratio <= cap) {
      return resolved.layout.main;
    }
    return setRatio(resolved.layout.main, splitId, cap);
  });

  /** Whether the tab group at `tabsId` has a group to its left. */
  function canMoveLeftFor(tabsId: string): boolean {
    return leftSiblingOf(resolved.layout.main, tabsId) !== null;
  }
  /** Whether the tab group at `tabsId` has a group to its right. */
  function canMoveRightFor(tabsId: string): boolean {
    return rightSiblingOf(resolved.layout.main, tabsId) !== null;
  }

  function onRatio(splitId: string, ratio: number): void {
    let next = ratio;
    const cap = mediumSidebarCap;
    if (splitId === sidebarSplitId && cap !== null && next > cap) {
      next = cap;
    }
    layouts.setRatio(root, splitId, next);
  }

  function onActivateTab(tabsId: string, index: number): void {
    layouts.setActiveTab(root, tabsId, index);
  }

  function onAddPanel(tabsId: string, panel: PanelId): void {
    layouts.addPanel(root, panel, tabsId);
  }

  function onPanelAction(
    action: PanelGroupAction,
    tabsId: string,
    panel: PanelId,
  ): void {
    if (action === "hide") {
      layouts.hidePanel(root, panel);
      return;
    }
    if (action === "new-group-left") {
      layouts.movePanelTo(root, panel, { kind: "before", nodeId: tabsId });
      return;
    }
    if (action === "new-group-right") {
      layouts.movePanelTo(root, panel, { kind: "after", nodeId: tabsId });
      return;
    }
    // move-left-group / move-right-group: merge into the adjacent sibling
    // group. Both directions share the same failure handling.
    const side = action === "move-left-group" ? "left" : "right";
    let failed: string | null = null;
    layouts.updateTree(root, (tree) => {
      const outcome =
        side === "left"
          ? moveToLeftGroup(tree, panel)
          : moveToRightGroup(tree, panel);
      if (!outcome.ok) {
        failed = outcome.reason ?? "failed";
        return null;
      }
      return outcome.tree;
    });
    if (failed === "sibling-not-a-group") {
      toast(`The area to the ${side} is not a tab group`);
    } else if (failed !== null) {
      toast(`No group to the ${side}`);
    }
  }

  /** Save-layout prompt + abort confirmation (M9 F9 dialog state). */
  let saveLayoutOpen = $state(false);
  let abortOpen = $state(false);

  function onSaveLayout(): void {
    saveLayoutOpen = true;
  }

  function onResetLayout(): void {
    layouts.resetRepo(root);
  }

  /** Header "＋ Terminal" button: focuses the terminal tab when the panel is
   *  already placed, otherwise adds it into the primary sidebar. Stacked
   *  makes the terminal the active full-screen route (placing it first). */
  function onNewTerminal(): void {
    if (stackedView !== null) {
      selectStackedView("terminal");
      return;
    }
    const holder = findPanelNode(resolved.layout.main, "terminal");
    if (holder) {
      layouts.setSidebarCollapsed(root, "left", false);
      if (holder.kind === "tabs") {
        layouts.setActiveTab(root, holder.id, holder.tabs.indexOf("terminal"));
      }
      return;
    }
    layouts.setSidebarCollapsed(root, "left", false);
    layouts.addPanel(root, "terminal", activityGroup?.id ?? firstTabsId(resolved.layout.main) ?? undefined);
  }

  // --- Files panel + workspace + quick-open event wiring ---------------------

  $effect(() => {
    return onUiEvent("files-open-path", ({ repoId: target, path }) => {
      if (target !== repoId) return;
      void openWorkdirFile(path);
    });
  });

  $effect(() => {
    return onUiEvent("files-open-diff", ({ repoId: target, path }) => {
      if (target !== repoId) return;
      workdirDiffFor(path);
    });
  });

  $effect(() => {
    return onUiEvent("files-quick-open", () => {
      if (repoId === null) return;
      quickOpenVisible = true;
    });
  });

  // --- Panel registry (PanelId → snippet) -----------------------------------

  function renderPanel(id: PanelId): Snippet {
    switch (id) {
      case "files":
        return filesPanel;
      case "status":
        return statusPanel;
      case "branches":
        return branchesPanel;
      case "remotes":
        return remotesPanel;
      case "stashes":
        return stashesPanel;
      case "worktrees":
        return worktreesPanel;
      case "health":
        return healthPanel;
      case "submodules":
        return submodulesPanel;
      case "reflog":
        return reflogPanel;
      case "undo":
        return undoPanel;
      case "history":
        return historyPanel;
      case "diff":
        return editorPanel;
      case "terminal":
        return terminalPanel;
      case "forge":
        return forgePanel;
      case "actions":
        return actionsPanel;
      case "stats":
        return statsPanel;
      case "assistant":
        return assistantPanel;
      default:
        return missingPanel;
    }
  }

  // --- Clean dialog + AI feature bridge ------------------------------------

  let cleanOpen = $state(false);

  // --- GitHub remote override dialog (E1, plan §9.1) -------------------------
  //
  // The GitHub flow resolves owner/repo from one remote (`origin` default).
  // Forks and mirrors pick another remote here; the choice persists per repo
  // root (`mygitui.forge.remote.<root>`) and applies to the footer chips'
  // snapshot lanes + the panel context. The dialog shows each remote's host
  // and the resolved GitHub repo (or the off-reason).

  let forgeRemoteOpen = $state(false);
  let forgeRemoteList = $state<RemoteInfo[]>([]);
  /** The override in effect when the dialog opened (for the reset row). */
  let forgeRemoteCurrent = $state<string | null>(null);

  async function openForgeRemoteDialog(): Promise<void> {
    forgeRemoteCurrent = loadForgeRemote(root);
    forgeRemoteOpen = true;
    try {
      forgeRemoteList = await listRemotes(repoId);
    } catch {
      forgeRemoteList = [];
    }
  }

  /**
   * The header overflow's repo-specific commands as data (Workstream A):
   * RepoHeader renders this list as the menu's rows AND as the pinned
   * header buttons (`mygitui.repoActionPins`), so a pinned command keeps
   * working whichever surface launched it. `gapBefore` groups the list
   * like the old separators (bisect/forge | layout ops | recents).
   */
  const headerActions = $derived<HeaderAction[]>([
    {
      id: "bisect",
      label: "Start bisect…",
      icon: iconBisect,
      run: () => emitUiEvent("bisect-open-start"),
    },
    {
      id: "forge-remote",
      label: "GitHub remote…",
      icon: iconForgeRemote,
      run: () => void openForgeRemoteDialog(),
    },
    {
      id: "save-layout",
      label: "Save layout as…",
      icon: iconSaveLayout,
      run: onSaveLayout,
      gapBefore: true,
    },
    {
      id: "clean",
      label: "Clean untracked files…",
      icon: iconClean,
      run: () => (cleanOpen = true),
    },
    {
      id: "reset-layout",
      label: "Reset layout to preset",
      icon: iconResetLayout,
      disabled: !resolved.hasOverlay,
      run: onResetLayout,
    },
    {
      id: "recents",
      label: "Recent repositories",
      icon: iconHome,
      run: showRecentRepos,
      gapBefore: true,
    },
  ]);

  /** The repo's resolved GitHub slug / off-reason (one status line). */
  const forgeRemoteStatus = $derived.by(() => {
    const context = forgeStore.contextFor(repoId);
    if (context !== null) {
      return `Using ${context.remote_url} — resolved to ${context.owner}/${context.repo}.`;
    }
    const kind = forgeStore.snapshotFailureKindFor(repoId);
    const detail =
      forgeStore.snapshotErrorFor(repoId) ??
      (kind !== null ? `GitHub state is off (${kind}).` : "");
    return detail === "" ? "GitHub state has not been fetched yet." : detail;
  });

  function chooseForgeRemote(remoteName: string | null): void {
    saveForgeRemote(root, remoteName);
    forgeRemoteOpen = false;
    // The new remote changes everything the flow resolves: drop the cached
    // context + snapshot; the chips' lanes and the panel refetch on demand.
    forgeStore.dropSnapshot(repoId);
    void forgeStore.refreshContext(repoId);
    void forgeChips.refresh(repoId);
  }

  // The assistant panel's active tab, bound two-way into the panel snippet
  // (`ai-open-activity` flips it before focusing so the panel — whether
  // fresh-mounted or already open — lands on the Activity tab).
  let assistantTab = $state<"chat" | "activity">("chat");

  // H2's CreatePrDialog asks for AI generation via the typed UI bus; H1's
  // runFeature answers it. One bridge, registered once per workspace mount.
  // The dialog's `template` (the repo's PR template, when one exists) rides
  // along so the generated description follows its structure.
  $effect(() => {
    return onUiEvent("ai-generate-pr", ({ repoId: target, template }) => {
      void (async () => {
        try {
          const { ai } = await import("$lib/ai/ai.svelte");
          const outcome = await ai.run("pr-title-body", {
            repoId: target,
            template: template ?? undefined,
          });
          emitUiEvent("ai-pr-result", {
            subject: outcome.message?.subject ?? outcome.result.text,
            body: outcome.message?.body ?? "",
          });
        } catch (err) {
          const message = err instanceof Error ? err.message : String(err);
          toast(`AI PR description failed: ${message}`, { kind: "error" });
          // The dialog's "Waiting for AI…" only clears on events — tell it
          // the run failed so the user can retry or fill the fields manually.
          emitUiEvent("ai-pr-error", { message });
        }
      })();
    });
  });

  // --- Conflict banner state ---------------------------------------------

  /**
   * Sub-tab a `focus-panel { panel: "forge", tab }` event asked the forge
   * panel to open (E2 run-view breadcrumb → runs list). Carries a sequence
   * number so repeating the same tab still re-applies; consumed by the
   * panel through the `requestedTab` prop.
   */
  let forgeTabRequest = $state<{ tab: string; n: number } | null>(null);
  let forgeTabSeq = 0;

  /** ConflictEditor overlay open? */
  let conflictEditorOpen = $state(false);
  let conflictDialogEl = $state<HTMLElement | null>(null);
  /** Element focused before the overlay opened (restored on close). */
  let conflictRestoreTo: HTMLElement | null = null;
  /** Abort RPC in flight (disables the Abort button). */
  let aborting = $state(false);
  /** Continue RPC in flight (disables the Continue button). */
  let continuing = $state(false);

  // The banner's Continue reads the shared rebase monitor, which the history
  // planner normally owns. Attach whenever this repo reports a live rebase
  // so Continue/Abort stay available after the planner was closed to reach
  // the conflict editor.
  $effect(() => {
    if (status?.rebasing) rebaseMonitor.start(repoId);
    else if (rebaseMonitor.repoId === repoId) rebaseMonitor.stop();
  });

  /** In-progress operation (merge / rebase / sequencer), or null. */
  const opSource = $derived(status ? conflictSource(status) : null);
  /** Conflict files for the current repo (cached by the store). */
  const conflictFiles = $derived(
    opSource ? conflictsStore.files(repoId) : ([] as ConflictFile[]),
  );
  const conflictCount = $derived(conflictFiles.length);

  /**
   * Cheap fingerprint of the conflicted set (repo id + per-file path and
   * change kinds, so a stage change on a conflicted file counts too).
   * Status refreshes land every ~2s during a merge (the watcher's
   * force-flush window) and a completed load itself re-triggers the
   * effect below through the store's reactive entry — the fingerprint
   * turns those repeats into no-ops so `load` only runs when the set
   * actually changed (or transitions clean → in-progress).
   */
  const conflictSignature = $derived(
    status
      ? [
          repoId,
          ...status.entries
            .filter(
              (entry) =>
                entry.index === "conflicted" ||
                entry.worktree === "conflicted",
            )
            .map((entry) => `${entry.path}:${entry.index}:${entry.worktree}`)
            .sort(),
        ].join("\n")
      : null,
  );
  /** Fingerprint the conflict list was last loaded for (null = clean). */
  let loadedConflictSignature: string | null = null;

  // While an operation is in progress, (re)fetch the conflict list when
  // the conflicted set changed; the moment the status is clean, drop the
  // cache and any open editor.
  $effect(() => {
    const signature = conflictSignature;
    if (opSource) {
      if (loadedConflictSignature === signature) return;
      loadedConflictSignature = signature;
      void conflictsStore.load(repoId);
    } else {
      loadedConflictSignature = null;
      conflictsStore.clear(repoId);
      conflictEditorOpen = false;
    }
  });

  // Conflict overlay a11y: focus the dialog when it opens (ConflictEditor
  // has no autofocus of its own) and return focus to the invoker on close.
  $effect(() => {
    if (conflictEditorOpen) {
      conflictRestoreTo =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
      requestAnimationFrame(() => conflictDialogEl?.focus());
    } else if (conflictRestoreTo !== null) {
      conflictRestoreTo.focus();
      conflictRestoreTo = null;
    }
  });

  // Leaving a repo (switch or unmount) drops its conflicts cache entry.
  $effect(() => {
    const id = repoId;
    return () => conflictsStore.clear(id);
  });

  // --- M9 F1: palette event wiring -----------------------------------------

  /**
   * Reveals a panel in the layout: activates its tab, opens the matching
   * sidebar, or adds the panel to the primary sidebar when hidden.
   * `tab` (optional) additionally requests a sub-tab inside the panel —
   * only the forge panel consumes one today. Stacked reveals = make the
   * panel the active full-screen route (placed when the layout lacks it).
   */
  function focusPanel(targetPanel: PanelId, tab?: string): void {
    if (stackedView !== null) {
      if (targetPanel === "forge" && tab !== undefined) {
        forgeTabRequest = { tab, n: ++forgeTabSeq };
      }
      selectStackedView(targetPanel);
      return;
    }
    if (targetPanel === "history") showHistory();
    if (targetPanel === "forge" && tab !== undefined) {
      forgeTabRequest = { tab, n: ++forgeTabSeq };
    }
    const holder = findPanelNode(resolved.layout.main, targetPanel);
    if (holder) {
      if (holder.kind === "tabs") {
        layouts.setActiveTab(root, holder.id, holder.tabs.indexOf(targetPanel));
      }
      if (activityGroup && holder.id === activityGroup.id) {
        layouts.setSidebarCollapsed(root, "left", false);
      }
      return;
    }
    layouts.setSidebarCollapsed(root, "left", false);
    layouts.addPanel(
      root,
      targetPanel,
      activityGroup?.id ?? firstTabsId(resolved.layout.main) ?? undefined,
    );
  }

  $effect(() => {
    const unlisteners = [
      onUiEvent("focus-panel", ({ panel, repoId: target, tab }) => {
        if (target !== null && target !== repoId) return;
        focusPanel(panel as PanelId, tab);
      }),
      // Status-bar AI rows: reveal the assistant panel with its Activity
      // tab (job history + usage). The panel's tab is bound state, so this
      // works whether the panel is already mounted or added by focusPanel.
      onUiEvent("ai-open-activity", () => {
        assistantTab = "activity";
        focusPanel("assistant");
      }),
      onUiEvent("workflow-run-open", ({ repoId: target, runId }) => {
        if (target !== repoId) return;
        openWorkflowRun(runId);
      }),
      onUiEvent("action-run-open", ({ repoId: target, runId }) => {
        if (target !== repoId) return;
        openActionRun(runId);
      }),
      onUiEvent("open-conflicts", () => {
        if (opSource) conflictEditorOpen = true;
      }),
      onUiEvent("conflicts-recheck", () => {
        void conflictsStore.load(repoId);
      }),
    ];
    return () => {
      for (const unlisten of unlisteners) unlisten();
    };
  });

  /** Unique changed paths — a file staged *and* modified counts once. */
  const changeCount = $derived(
    status ? new Set(status.entries.map((entry) => entry.path)).size : 0,
  );

  // Remote repositories (device_id set): the forge flow runs on the device
  // (see the forgePanel snippet), and the connection state lives in the
  // status bar (Workstream A moved the header chip there; remote tabs carry
  // a cloud glyph).
  const isRemote = $derived(deviceId !== null);

  /** Any mutation refreshed its repo: pull the fresh status into the tab. */
  function refresh(): void {
    void refreshStatus(repoId);
  }

  // Op-progress + auth-request subscriptions are idempotent; start them with
  // the first repo view and keep them for the session.
  $effect(() => {
    startOpsEvents();
    startAuthEvents();
  });

  // Auto-fetch runs while this repo's tab is shown (interval from
  // localStorage config; stop on switch/close).
  $effect(() => {
    const id = repoId;
    void autofetch.start(id);
    return () => autofetch.stop(id);
  });

  // Terminal teardown on repo close: the view unmounts only when the last
  // tab closes (switching repos re-uses this component), so mirror the
  // current repoId in a plain variable and dispose that one at unmount —
  // the cleanup must NOT run per repoId change, or switching tabs would
  // kill the previous repo's shell. (Initialized empty: the first effect
  // fills it before anything can unmount.)
  let latestRepoId = "";
  $effect(() => {
    latestRepoId = repoId;
  });
  $effect(() => {
    return () => terminals.dispose(latestRepoId);
  });

  // Reset view state when switching repos — then restore the new root's
  // persisted workspace session (open files, active tab, center pane, diff,
  // scroll positions). The wipe itself stays: open tabs are keyed by workdir
  // path only, and keeping repo A's editors mounted across a switch would
  // run repo A's paths against repo B's IPC (read/write the wrong repo's
  // files) — restore re-opens everything through the normal open paths, so
  // every IPC call targets the NEW repoId.
  $effect(() => {
    void repoId;
    void root;
    // Capture the outgoing repo's dirty buffers while their editors are
    // still mounted (the wipe below unmounts them). Plain-variable read:
    // the capture API must not become an effect dependency.
    const previousRoot = lastSessionRoot;
    lastSessionRoot = root;
    const capture = workspaceCapture;
    if (previousRoot !== null && previousRoot !== root && capture !== null) {
      for (const [path, text] of Object.entries(capture.dirtyDocs())) {
        workspaceSession.setBuffer(previousRoot, path, text);
      }
    }
    selected = [];
    center = EMPTY_PANE;
    diffFiles = [];
    diffLoading = false;
    diffError = null;
    historyActions = null;
    pendingHistorySha = null;
    conflictEditorOpen = false;
    // Status-derived flags are repo-scoped too: a stale `hasSubmodules`
    // from repo A would render repo A's submodule accordions against repo
    // B's state until the next status snapshot landed.
    hasSubmodules = false;
    openFiles = [];
    activeFile = null;
    pendingDiffRestore = null;
    diffScrollSeed = null;
    diffRowSeed = null;
    editorSeed = null;
    // Untracked: the restore reads reactive state (status, openFiles) whose
    // changes must not re-trigger this wipe.
    untrack(() => void restoreWorkspaceSession(root));
  });

  /**
   * Re-opens the new root's persisted workspace after the wipe: file tabs
   * go through {@link openWorkdirFile} (stat IPC targeted at the new
   * repoId), then the active tab, the center pane and the open diff tabs.
   * Stored diffs resolve their StatusEntries against the current status —
   * deferred via {@link pendingDiffRestore} when the snapshot has not
   * landed yet; paths whose change is gone are skipped silently. A commit
   * detail restores from the in-memory commit object (in-app switch); on a
   * cold launch there is none, and HistoryView's persisted-selection
   * resolve re-opens it through the "restore" select, which the session's
   * `centerKind: "commit"` now permits.
   */
  async function restoreWorkspaceSession(targetRoot: string): Promise<void> {
    const session = workspaceSession.get(targetRoot);
    if (session === null) return;
    const token = ++restoreToken;
    const paths = session.openFiles;
    if (paths.length > 0) {
      // Editor seed for the FileEditors this restore mounts: dirty-buffer
      // text (drained — it must not re-apply after a later close/reopen)
      // and the saved per-file scroll positions.
      const scrolls: Record<string, number> = {};
      for (const [key, top] of Object.entries(session.scrolls)) {
        if (key.startsWith("file:")) scrolls[key] = top;
      }
      editorSeed = { docs: workspaceSession.takeBuffers(targetRoot), scrolls };
      await Promise.all(paths.map((path) => openWorkdirFile(path)));
      if (token !== restoreToken) return;
      editorSeed = null;
    }
    if (
      session.activeFile !== null &&
      openFiles.some((f) => f.path === session.activeFile)
    ) {
      activeFile = session.activeFile;
      syncSession();
    }
    if (session.centerKind === "diff" && session.diffs.length > 0) {
      const current = status;
      if (current === null) {
        // Status has not landed yet — resolve once it does (effect below).
        pendingDiffRestore = { refs: session.diffs, activePath: session.activeDiffPath };
      } else {
        restoreDiffTabs(current, session.diffs, session.activeDiffPath);
      }
    } else if (session.centerKind === "commit" && session.commitSha !== null) {
      const memo = workspaceSession.commitFor(targetRoot);
      if (memo !== null && memo.sha === session.commitSha) {
        center = { kind: "commit", commit: memo };
        syncSession();
      }
      // Cold launch: leave the center empty; HistoryView's resolve owns it.
    }
  }

  /**
   * Resolves a restored diff-tab set against a landed status and opens it
   * as one multi-diff stream: every ref whose change still exists becomes
   * a tab, the stored active path (or the last surviving tab) is focused.
   * Returns false when nothing survived (all changes gone).
   */
  function restoreDiffTabs(
    current: RepoStatus,
    refs: StoredDiffRef[],
    activePath: string | null,
  ): boolean {
    const tabs: DiffTab[] = [];
    for (const ref of refs) {
      const entry = current.entries.find(
        (e) => e.path === ref.path || e.old_path === ref.path,
      );
      if (entry) tabs.push({ entry, scope: ref.scope });
    }
    if (tabs.length === 0) return false;
    const active = tabs.some((t) => t.entry.path === activePath)
      ? activePath!
      : tabs[tabs.length - 1]!.entry.path;
    void streamDiffTabs(
      tabs,
      active,
      undefined,
      undefined,
      undefined,
      scrollSeedFor(active),
      rowSeedFor(active),
    );
    return true;
  }

  /** Consumes a deferred diff restore once the new repo's status lands. */
  $effect(() => {
    const pending = pendingDiffRestore;
    if (pending === null || status === null) return;
    pendingDiffRestore = null;
    const restored = restoreDiffTabs(status, pending.refs, pending.activePath);
    if (!restored) {
      // Every stored diff's change vanished while we were away: fall back
      // to the normal empty-editor behavior (re-arm first-change auto-open).
      if (center.kind === "empty") autoOpenedFor = null;
    }
  });

  // -- repo-switch transition --------------------------------------------------

  let workspaceEl = $state<HTMLElement | null>(null);
  /** RepoId the workspace transition was last applied to (null = mount). */
  let transitionedRepoId: string | null = null;

  $effect(() => {
    // Brief opacity settle when the repoId PROP changes — not on mount.
    // The first cut of this transition also dipped the workspace 3px down
    // and flashed it to 35% opacity; on a full-window surface that reads
    // as a blink-and-jump — the "whole UI twitches" report. So: opacity
    // only, a gentle floor (0.6) and a fast duration (the overlay chrome's
    // own fades run 100–150ms) — it smooths the content swap without any
    // positional movement. The workspace element is reused across switches,
    // so the Web Animations API (el.animate) keeps the transition
    // restartable without a remount or CSS-class hacks.
    const id = repoId;
    if (transitionedRepoId === null) {
      transitionedRepoId = id;
      return;
    }
    if (id === transitionedRepoId) return;
    transitionedRepoId = id;
    const el = workspaceEl;
    if (!el || typeof el.animate !== "function") return;
    if (
      typeof matchMedia === "function" &&
      matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      return;
    }
    el.animate([{ opacity: 0.6 }, { opacity: 1 }], {
      duration: 140,
      easing: "cubic-bezier(0.2, 0, 0, 1)",
    });
  });

  // Repos persisted before the editor existed may have no `diff` panel:
  // insert it left of history (or as a new right column) once on mount.
  $effect(() => {
    void repoId;
    layouts.showMainPanel(root);
  });

  // --- Center editor selection ------------------------------------------------

  /** Opens a GitHub Actions run in the main editor (forge sidebar event). */
  function openWorkflowRun(runId: number): void {
    diffToken++;
    diffFiles = [];
    diffLoading = false;
    diffError = null;
    center = { kind: "workflowRun", runId };
    syncSession();
    focusPanel("diff");
  }

  /** Opens a custom action run in the main editor (actions panel event). */
  function openActionRun(runId: string): void {
    diffToken++;
    diffFiles = [];
    diffLoading = false;
    diffError = null;
    center = { kind: "actionRun", runId };
    syncSession();
    focusPanel("diff");
  }

  /**
   * Working-copy row selected in the changes sidebar.
   *
   * VSCode-style scope semantics (`scope` comes from the row's section; see
   * {@link diffScopeSides}): the staged view streams HEAD→index (what
   * `git diff --cached` shows), the unstaged view index→worktree (plain
   * `git diff`). When nothing is staged, index == HEAD, so the unstaged view
   * equals the old head→worktree one — which is why every entry point that
   * has no section (auto-open, prev/next, the file workspace) defaults to
   * "unstaged".
   *
   * Multi-diff model: opening a second file APPENDS a tab (an already-open
   * path just focuses its tab, refreshing the entry + scope) and the one
   * stream request covers every open path — each tab filters its own files
   * out of the result in the view.
   */
  async function openDiff(
    entry: StatusEntry,
    targetRepoId?: string,
    targetRoot?: string,
    targetRepoName?: string,
    scope: DiffScope = "unstaged",
    /** Restore-only extras (a workspace session putting this diff back). */
    opts?: { initialScroll?: number; initialRow?: number },
  ): Promise<void> {
    const existing = center.kind === "diff" ? center.tabs : [];
    await streamDiffTabs(
      upsertDiffTab(existing, entry, scope),
      entry.path,
      targetRepoId,
      targetRoot,
      targetRepoName,
      opts?.initialScroll ?? null,
      opts?.initialRow ?? null,
    );
  }

  /**
   * Streams the working-copy diff for a full open-tab set: sets the diff
   * center with `activePath` focused and requests every open path in one
   * stream (the sides follow the ACTIVE tab's scope — a mixed-scope set
   * re-streams when another tab activates, see {@link activateDiffTab}).
   * Pages accumulate: the ~50-file pages together cover all open paths.
   */
  async function streamDiffTabs(
    tabs: DiffTab[],
    activePath: string,
    targetRepoId?: string,
    targetRoot?: string,
    targetRepoName?: string,
    initialScroll: number | null = null,
    initialRow: number | null = null,
  ): Promise<void> {
    const active = activeDiffTab(tabs, activePath);
    if (!active) return;
    const token = ++diffToken;
    center = { kind: "diff", tabs, activePath: active.entry.path };
    diffFiles = [];
    diffLoading = true;
    diffError = null;
    diffStreamScope = active.scope;
    // Consumed by the DiffViewer the stream remounts (null = top, as always).
    diffScrollSeed = initialScroll;
    diffRowSeed = initialRow;
    syncSession();
    const { oldSide, newSide } = diffScopeSides(active.scope);
    diffRepoId = targetRepoId ?? repoId;
    diffRoot = targetRoot ?? root;
    diffRepoName = targetRepoName ?? "";
    const openPaths = tabs.map((t) => t.entry.path);
    try {
      await streamDiff(
        diffRepoId,
        oldSide,
        newSide,
        (page) => {
          if (token !== diffToken) return;
          diffFiles = [
            ...diffFiles,
            ...diffPageForOpenPaths(page, openPaths),
          ];
        },
        openPaths,
        // Own stream slot so the workspace diff pane never supersedes the
        // agent/commit-detail/popout diff streams (per-consumer slots).
        "repo-diff",
        // The toolbar's ignore-whitespace flag rides the backend query.
        diffPrefs.ignoreWs,
      );
    } catch (err) {
      if (token === diffToken) {
        diffFiles = [];
        diffError = err instanceof Error ? err.message : String(err);
      }
    } finally {
      if (token === diffToken) diffLoading = false;
    }
  }

  /** The saved session scroll of a diff path (what a tab switch restores). */
  function scrollSeedFor(path: string): number | null {
    if (root === "") return null;
    return workspaceSession.get(root)?.scrolls[diffScrollKey(path)] ?? null;
  }

  /** Focuses an open diff tab (workspace-tab click). */
  function activateDiffTab(path: string): void {
    if (center.kind !== "diff") return;
    const tab = center.tabs.find((t) => t.entry.path === path);
    if (!tab) return;
    if (tab.scope !== diffStreamScope) {
      // The streamed sides follow the activated tab's scope: re-stream so
      // the shown diff really is that tab's staged/unstaged view.
      void streamDiffTabs(
        center.tabs,
        path,
        diffRepoId ?? undefined,
        diffRoot ?? undefined,
        diffRepoName ?? undefined,
        scrollSeedFor(path),
        rowSeedFor(path),
      );
      return;
    }
    // Same comparison as the shown batch: just switch, the viewer remounts
    // (keyed by path) and restores the tab's saved scroll.
    diffScrollSeed = scrollSeedFor(path);
    diffRowSeed = rowSeedFor(path);
    center = { kind: "diff", tabs: center.tabs, activePath: path };
    syncSession();
  }

  /** Closes one diff tab (per-tab ✕ / middle-click); the last one empties. */
  function closeDiffTab(path: string): void {
    if (center.kind !== "diff") return;
    const previous = center;
    const tabs = withoutDiffTab(previous.tabs, path);
    if (tabs.length === 0) {
      clearEditor();
      return;
    }
    const activePath =
      neighborDiffPath(previous.tabs, previous.activePath, path) ??
      tabs[0]!.entry.path;
    if (activePath === previous.activePath) {
      // An inactive tab closed: the shown diff stays, no remount.
      center = { kind: "diff", tabs, activePath };
      syncSession();
      return;
    }
    // The activated neighbor remounts the viewer: seed its saved scroll,
    // re-streaming when its comparison differs from the streamed sides.
    diffScrollSeed = scrollSeedFor(activePath);
    diffRowSeed = rowSeedFor(activePath);
    const tab = tabs.find((t) => t.entry.path === activePath)!;
    if (tab.scope !== diffStreamScope) {
      void streamDiffTabs(
        tabs,
        activePath,
        diffRepoId ?? undefined,
        diffRoot ?? undefined,
        diffRepoName ?? undefined,
        diffScrollSeed,
        diffRowSeed,
      );
      return;
    }
    center = { kind: "diff", tabs, activePath };
    syncSession();
  }

  /** Re-runs the current working-copy diff load (the error state's Retry). */
  function reopenDiff(): void {
    if (diffEntry === null) return;
    void openDiff(
      diffEntry,
      diffRepoId ?? undefined,
      diffRoot ?? undefined,
      diffRepoName ?? undefined,
      diffScope,
    );
  }

  function onEditorMutated(): void {
    refresh();
    if (diffRepoId && diffRepoId !== repoId) {
      void refreshStatus(diffRepoId);
    }
    if (diffEntry) {
      void openDiff(
        diffEntry,
        diffRepoId ?? undefined,
        diffRoot ?? undefined,
        diffRepoName ?? undefined,
        diffScope,
      );
    }
  }

  // --- Center pane: first-change open + file actions (UI review P2 #8) ------

  /** Changed files in status order — drives auto-open and prev/next. */
  const changeEntries = $derived(status?.entries ?? []);

  /** Auto-open the first change once per repo, when the editor is empty. */
  let autoOpenedFor = $state<string | null>(null);
  $effect(() => {
    if (autoOpenedFor === repoId || status === null) return;
    // A workspace restore in flight owns the center: skip without pinning
    // auto-open, so a restored diff whose path vanished can fall back to it.
    if (pendingDiffRestore !== null) return;
    autoOpenedFor = repoId;
    const prefs = root ? loadHistoryPrefs(root) : null;
    if (prefs?.selectedSha) return;
    const first = status.entries[0];
    // The old encoding checked its five nullable siblings but not
    // `filesMode` — an open file workspace got yanked to a diff. The
    // union makes "empty" exactly what it says.
    if (first && center.kind === "empty") {
      void openDiff(first, undefined, undefined, undefined, diffScopeForEntry(first));
    }
  });

  /** Keep every open diff tab's entry kinds in sync with the refreshed
   *  status — staging from the header flips the active entry's state, and
   *  the tab-strip indicators need the inactive tabs fresh too. The scopes
   *  stay: each tab keeps comparing the sides it was opened with (a staged
   *  view simply goes empty once everything is staged). */
  $effect(() => {
    const tabs = diffTabList;
    if (tabs.length === 0 || diffRepoId !== repoId || !status) return;
    if (center.kind !== "diff") return;
    let changed = false;
    const freshTabs = tabs.map((tab) => {
      const fresh = status.entries.find((e) => e.path === tab.entry.path);
      if (!fresh) return tab;
      if (
        fresh.index === tab.entry.index &&
        fresh.worktree === tab.entry.worktree
      ) {
        return tab;
      }
      changed = true;
      return { ...tab, entry: fresh };
    });
    if (changed) {
      center = { kind: "diff", tabs: freshTabs, activePath: center.activePath };
    }
  });

  /** Header ↑/↓: step through changed files (wraps around). Main-repo
   *  diffs only — submodule panes have no entry in this status list. */
  function openAdjacentChange(delta: 1 | -1): void {
    const list = changeEntries;
    if (!diffEntry || diffRepoId !== repoId || list.length < 2) return;
    const at = list.findIndex((e) => e.path === diffEntry?.path);
    const next = list[(Math.max(at, 0) + delta + list.length) % list.length]!;
    void openDiff(next, undefined, undefined, undefined, diffScopeForEntry(next));
  }

  /** Welcome pane: open the first changed file. */
  function openFirstChange(): void {
    const first = changeEntries[0];
    if (first) {
      void openDiff(first, undefined, undefined, undefined, diffScopeForEntry(first));
    }
  }

  /** True while the open file's stage/unstage IPC is in flight: the diff
   *  header's Stage/Unstage buttons disable (a rapid double-click fired
   *  duplicate stage calls + duplicate toasts). */
  let staging = $state(false);

  /** Header Stage/Unstage for the open file. */
  async function stageOpenEntry(unstage: boolean): Promise<void> {
    const current = diffEntry;
    if (!current || diffRepoId !== repoId || staging) return;
    staging = true;
    try {
      await stage(repoId, { targets: [{ file: current.path }], unstage });
      refresh();
      toast(
        `${unstage ? "Unstaged" : "Staged"} ${current.path}`,
        { kind: "success" },
      );
    } catch (err) {
      toast(
        `${unstage ? "Unstage" : "Stage"} failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      staging = false;
    }
  }

  /** Discard confirmation for the open file (discard checkpoints upstream).
   *  Both settings are read reactively so a mid-session Settings toggle
   *  applies to the next discard immediately. */
  let discardOpen = $state(false);
  const confirmDiscardEnabled = $derived(appSettings.confirmDiscard);
  const checkpointsEnabled = $derived(appSettings.checkpointsEnabled);

  function requestDiscardOpenEntry(): void {
    if (diffEntry === null) return;
    // "Confirm before discard" off: discard right away (same skip as
    // StatusPanel's row menus).
    if (!confirmDiscardEnabled) void discardOpenEntry();
    else discardOpen = true;
  }

  async function discardOpenEntry(): Promise<void> {
    const current = diffEntry;
    const targetRepoId = diffRepoId ?? repoId;
    if (!current) return;
    // Renames discard both sides: path alone leaves the old path's
    // deletion staged.
    const targets = [{ file: current.path }];
    if (current.old_path && current.old_path !== current.path) {
      targets.push({ file: current.old_path });
    }
    try {
      // A non-null warning means the discard succeeded but the undo
      // checkpoint failed (e.g. no git identity) — surface it instead
      // of silently losing the undo point.
      const warning = await discard(targetRepoId, targets, {
        checkpoint: checkpointsEnabled,
      });
      discardOpen = false;
      if (warning) {
        toast(`Discarded, but the undo checkpoint failed: ${warning}`, {
          kind: "info",
        });
      } else {
        toast(`Discarded changes in ${current.path}`, { kind: "success" });
      }
      refresh();
      if (diffRepoId && diffRepoId !== repoId) void refreshStatus(diffRepoId);
      void openDiff(
        current,
        diffRepoId ?? undefined,
        diffRoot ?? undefined,
        diffRepoName ?? undefined,
        diffScope,
      );
    } catch (err) {
      toast(
        `Discard failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    }
  }

  /** Clears the center editor (close button or the empty-state reset). */
  function clearEditor(): void {
    diffToken++;
    center = EMPTY_PANE;
    diffFiles = [];
    diffLoading = false;
    diffError = null;
    diffRepoId = null;
    diffRoot = null;
    diffRepoName = null;
    // Record the dismissal: the session's centerKind is what keeps a
    // re-entry's HistoryView restore from re-opening what the user closed.
    syncSession();
  }

  /** History sidebar: a commit row was selected (or restored on mount). */
  function onHistorySelect(
    info: CommitInfo | null,
    source?: "restore",
  ): void {
    // A restored selection only opens when nothing is in the editor: after
    // history is collapsed/reopened it must not replace an open file diff.
    // AND the session must say the detail was open when we left this repo —
    // that is what makes a dismissal stick across a repo switch (clearEditor
    // recorded "empty") while a relaunch with the detail open brings it back.
    if (source === "restore") {
      const session = workspaceSession.get(root);
      const allowed =
        session !== null &&
        session.centerKind === "commit" &&
        (session.commitSha === null || session.commitSha === info?.sha);
      if (!allowed || center.kind !== "empty") {
        return;
      }
    }
    if (info === null) {
      // Deselect: close a shown commit detail, leave other modes alone.
      if (center.kind === "commit") center = EMPTY_PANE;
      syncSession();
      return;
    }
    center = { kind: "commit", commit: info };
    diffToken++;
    diffFiles = [];
    diffLoading = false;
    diffError = null;
    syncSession();
  }

  /** History sidebar: compare results open in the center editor. */
  function onHistoryCompare(result: CompareResult): void {
    center = { kind: "compare", result };
    diffToken++;
    diffFiles = [];
    diffLoading = false;
    diffError = null;
    syncSession();
  }

  function onHistoryActions(actions: HistoryCommitActions | null): void {
    historyActions = actions;
  }

  /**
   * Parent navigation from the commit detail: reveal + select the sha in
   * the history sidebar, expanding the sidebar first when it is collapsed.
   * The request travels as a prop (not the shared event bus) so a panel that
   * mounts for the first time afterwards still receives it.
   */
  function revealHistoryCommit(sha: string): void {
    pendingHistorySha = sha;
    if (!historyVisible) showHistory();
  }

  // --- Popouts ---------------------------------------------------------------

  function popOutDiff(): void {
    void openPanelPopout("diff", diffRepoId ?? repoId, `${diffRepoName || name} — diff`);
  }

  function popOutHistory(): void {
    void openPanelPopout("history", repoId, `${name} — history`);
  }

  function popOutCommit(): void {
    const sha = selectedCommit?.sha;
    if (!sha) return;
    void openPanelPopout(
      "commitdetail",
      repoId,
      `Commit ${sha.slice(0, 7)}`,
      { sha },
    );
  }

  function popOutWorkflowRun(): void {
    if (workflowRunId === null) return;
    void openPanelPopout(
      "workflowrun",
      repoId,
      `${name} — run ${workflowRunId}`,
      { runId: workflowRunId },
    );
  }

  function popOutActionRun(): void {
    if (actionRunId === null) return;
    const run = actionsStore.runFor(actionRunId);
    if (!run) return;
    // The popout window is a separate JS context: hand it the log so far,
    // then it follows future `action-output` events itself.
    stashActionRun(run);
    void openPanelPopout(
      "actionrun",
      repoId,
      `${run.name} — run log`,
      { actionRunId: actionRunId },
    );
  }

  // --- Conflict banner actions -------------------------------------------

  function onResolve(): void {
    conflictEditorOpen = true;
  }

  /** ConflictEditor finished: every file was resolved (or skipped safely). */
  function onConflictsResolved(): void {
    conflictEditorOpen = false;
    conflictsStore.clear(repoId);
    refresh();
    toast("Conflicts resolved", { kind: "success" });
  }

  function onConflictsClosed(): void {
    conflictEditorOpen = false;
  }

  /**
   * Continues a paused rebase from the top banner. Delegates to the shared
   * monitor so its state (and the history planner's view of it) stays in
   * sync; the monitor toasts failures and refreshes the conflict list.
   */
  async function onContinueRebase(): Promise<void> {
    if (
      opSource !== "rebase" ||
      continuing ||
      !rebaseMonitor.paused ||
      rebaseMonitor.inConflict
    ) {
      return;
    }
    continuing = true;
    try {
      await rebaseMonitor.continueRebase();
      await refreshStatus(repoId);
    } finally {
      continuing = false;
    }
  }

  /**
   * Skips the rebase step the backend is paused on (conflicted commit
   * dropped, failed exec advanced past). Only rendered while the monitor
   * reports `skippable`; no confirmation — the rebase planner's pattern
   * keeps destructive confirms for the abort path only.
   */
  async function onSkipRebase(): Promise<void> {
    if (opSource !== "rebase" || continuing || !rebaseMonitor.skippable) return;
    continuing = true;
    try {
      await rebaseMonitor.skipRebase();
      await refreshStatus(repoId);
    } finally {
      continuing = false;
    }
  }

  /** Aborts the in-progress operation (rebase / merge / sequencer each have
   *  their own backend command — `conflictAbortCommand`). The confirmation
   *  is a ConfirmDialog (M9 F9). */
  async function onAbort(): Promise<void> {
    if (!opSource || aborting) return;
    abortOpen = true;
  }

  async function onAbortConfirmed(): Promise<void> {
    if (!opSource || aborting) return;
    const label = conflictSourceLabel(opSource);
    const command = conflictAbortCommand(opSource);
    aborting = true;
    try {
      if (command === "rebase_abort") {
        await rebaseAbort(repoId);
      } else if (command === "sequencer_abort") {
        await sequencerAbort(repoId);
      } else {
        await mergeAbort(repoId);
      }
      conflictsStore.clear(repoId);
      conflictEditorOpen = false;
      await refreshStatus(repoId);
      toast(`${label} aborted`, { kind: "success" });
    } catch (err) {
      toast(
        `Abort failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      aborting = false;
    }
  }
</script>

{#snippet filesPanel()}
  {#await filesPanelImport() then FilesPanelModule}
    <FilesPanelModule.default
      {repoId}
      {root}
      {status}
      activeFile={filesMode ? activeFile : null}
    />
  {/await}
{/snippet}
{#snippet statusPanel()}
  <!-- VSCode-style source-control column: the changes list scrolls above
       the panel's own pinned composer (Workstream B §6.1 — the composer
       lives inside StatusPanel so its scope chips can reach the
       submodules too). -->
  <div
    class="changes-panel flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
  >
    <StatusPanel
      {status}
      statusError={tabStore.tabs.find((t) => t.id === repoId)?.statusError ?? null}
      {repoId}
      {root}
      repoName={name}
      onOpenDiff={(e, subRepoId, subRoot, subName, scope) => {
        // Opening a diff surfaces the editor through the drawer's edge —
        // yield the overlay (§10 Narrow).
        if (drawerPanelIds.has("status")) closeDrawer("status");
        void openDiff(e, subRepoId, subRoot, subName, scope);
      }}
      onAfterMutation={refresh}
      bind:selected
      bind:hasSubmodules
    />
  </div>
{/snippet}
{#snippet editorPanel()}
  <EditorPane
    repoId={diffRepoId ?? repoId}
    root={diffRoot ?? root}
    repoName={diffRepoName ?? ""}
    status={diffRepoId && diffRepoId !== repoId ? (tabStore.tabs.find((t) => t.id === diffRepoId)?.status ?? null) : status}
    commit={selectedCommit}
    compare={compareResult}
    entry={diffEntry}
    diffTabs={diffTabList}
    onActivateDiffTab={activateDiffTab}
    onCloseDiffTab={closeDiffTab}
    diffFiles={diffFiles}
    diffLoading={diffLoading}
    diffError={diffError}
    onRetryDiff={diffEntry ? reopenDiff : undefined}
    diffScope={diffScope}
    historyActions={historyActions}
    workflowRunId={workflowRunId}
    actionRunId={actionRunId}
    entryIndex={diffEntry && diffRepoId === repoId
      ? changeEntries.findIndex((e) => e.path === diffEntry?.path)
      : null}
    entryCount={diffRepoId === repoId ? changeEntries.length : 0}
    files={openFiles.length > 0
      ? { repoId, root, openFiles, active: activeFile, visible: filesMode }
      : null}
    onClose={clearEditor}
    onCloseCompare={() => {
      if (center.kind === "compare") center = EMPTY_PANE;
    }}
    onCloseWorkflowRun={() => {
      if (center.kind === "workflowRun") center = EMPTY_PANE;
    }}
    onCloseActionRun={() => {
      if (center.kind === "actionRun") center = EMPTY_PANE;
    }}
    onSelectParent={revealHistoryCommit}
    onPopoutCommit={selectedCommit ? popOutCommit : undefined}
    onPopoutDiff={popOutDiff}
    onPopoutWorkflowRun={workflowRunId !== null ? popOutWorkflowRun : undefined}
    onPopoutActionRun={actionRunId !== null ? popOutActionRun : undefined}
    onMutated={onEditorMutated}
    onPrevFile={() => openAdjacentChange(-1)}
    onNextFile={() => openAdjacentChange(1)}
    onScopeChange={diffEntry
      ? (scope) =>
          void openDiff(
            diffEntry,
            diffRepoId ?? undefined,
            diffRoot ?? undefined,
            diffRepoName ?? undefined,
            scope,
          )
      : undefined}
    onStageEntry={diffRepoId === repoId
      ? (unstage) => void stageOpenEntry(unstage)
      : undefined}
    stageBusy={staging}
    onDiscardEntry={diffRepoId === repoId ? requestDiscardOpenEntry : undefined}
    onEditEntry={diffRepoId === repoId && diffEntry
      ? () => void openWorkdirFile(diffEntry!.path)
      : undefined}
    onActivateFile={(path) => {
      activeFile = path;
      syncSession();
    }}
    onCloseFile={closeWorkdirFile}
    onOpenDiffFile={workdirDiffFor}
    onOpenFirstChange={openFirstChange}
    onBrowseHistory={() => focusPanel("history")}
    filesSeed={editorSeed}
    onFileScroll={onFileScrollCapture}
    attachWorkspace={attachCapture}
    diffInitialScroll={diffScrollSeed}
    diffInitialRow={diffRowSeed}
    onDiffScroll={onDiffScrollCapture}
  />
{/snippet}
{#snippet branchesPanel()}
  {#await branchPanelImport() then BranchPanelModule}
    <BranchPanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet remotesPanel()}
  {#await remotePanelImport() then RemotePanelModule}
    <RemotePanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet stashesPanel()}
  {#await stashPanelImport() then StashPanelModule}
    <StashPanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet worktreesPanel()}
  {#await worktreePanelImport() then WorktreePanelModule}
    <WorktreePanelModule.default {repoId} {root} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet healthPanel()}
  {#await repoPanelImport() then RepoPanelModule}
    <RepoPanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet submodulesPanel()}
  {#await submodulePanelImport() then SubmodulePanelModule}
    <SubmodulePanelModule.default {repoId} {root} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet reflogPanel()}
  {#await reflogPanelImport() then ReflogPanelModule}
    <ReflogPanelModule.default {repoId} />
  {/await}
{/snippet}
{#snippet undoPanel()}
  {#await undoPanelImport() then UndoPanelModule}
    <UndoPanelModule.default {repoId} />
  {/await}
{/snippet}
{#snippet historyPanel()}
  {#await historyViewImport() then HistoryViewModule}
    <HistoryViewModule.default
      {repoId}
      {root}
      variant="sidebar"
      onPopout={popOutHistory}
      bookmarks={bookmarks.shas(root)}
      branchColors={(refs) => branchColorForRefs(refs, branchRules)}
      onSelectCommit={onHistorySelect}
      onCompareResult={onHistoryCompare}
      onActionsChange={onHistoryActions}
      onCollapse={canHideHistory ? hideHistory : undefined}
      revealSha={pendingHistorySha}
      onRevealHandled={() => (pendingHistorySha = null)}
    />
  {/await}
{/snippet}
{#snippet terminalPanel()}
  {#await terminalPanelImport() then TerminalPanelModule}
    <TerminalPanelModule.default {repoId} remote={isRemote} />
  {/await}
{/snippet}
{#snippet forgePanel()}
  <!-- Device repos run the gh flow ON THE DEVICE (forge.rs routes every
       command through the device session), so the panel mounts for them
       like for local repos — its banners speak for the repo's machine. -->
  {#await forgePanelImport() then ForgePanelModule}
    <ForgePanelModule.default {repoId} requestedTab={forgeTabRequest} />
  {/await}
{/snippet}
{#snippet actionsPanel()}
  {#await actionsPanelImport() then ActionsPanelModule}
    <ActionsPanelModule.default {repoId} {root} />
  {/await}
{/snippet}
{#snippet statsPanel()}
  {#await statsPanelImport() then StatsPanelModule}
    <StatsPanelModule.default {repoId} {root} />
  {/await}
{/snippet}
{#snippet assistantPanel()}
  {#await assistantPanelImport() then AssistantPanelModule}
    <AssistantPanelModule.default
      {repoId}
      {root}
      repoName={name}
      bind:tab={assistantTab}
      diffPath={diffEntry?.path ?? null}
    />
  {/await}
{/snippet}
{#snippet missingPanel()}
  <aside
    class="missing-panel flex flex-1 items-center justify-center text-ui-base text-muted-foreground"
  >This panel is not available.</aside>
{/snippet}

<!-- Fills the tab content area: without flex-1 this flex item shrink-wraps to
     its content, so collapsing a sidebar would shrink the whole workspace
     instead of handing the freed width to the center pane. min-width:0 keeps
     wide panel content from stretching the workspace past the window edge. -->
<div
  class="repo-workspace flex h-full w-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
  bind:this={workspaceEl}
>
  <!-- Header row: breadcrumb restyle, Workstream A §5.3 (path/preset/
       open-with demoted into the header overflow, remote chip → StatusBar). -->
  <RepoHeader
    {repoId}
    {name}
    {root}
    {status}
    presetId={resolved.presetId}
    {historyVisible}
    {canHideHistory}
    onMutated={refresh}
    onToggleHistory={toggleHistory}
    onNewTerminal={onNewTerminal}
    {headerActions}
  />

  {#if opSource && (conflictCount > 0 || (opSource === "rebase" && rebaseMonitor.paused))}
    <div
      class="conflict-banner flex flex-none items-center gap-2 border-b border-border bg-destructive/15 px-3 py-1 text-ui-base text-destructive"
      role="alert"
    >
      <span
        class="cb-icon inline-flex size-4 flex-none items-center justify-center rounded-full border-[1.5px] border-current text-ui-micro font-bold"
        aria-hidden="true"
      >!</span>
      <span
        class="cb-text min-w-0 overflow-hidden text-ellipsis whitespace-nowrap"
      >
        {conflictSourceLabel(opSource)} in progress{#if conflictCount > 0} — {conflictCount}
        conflicted file{conflictCount === 1 ? "" : "s"}{/if}
      </span>
      <span class="cb-actions ml-auto flex flex-none gap-1.5">
        {#if conflictCount > 0}
          <Button
            class="cb-resolve rounded-full px-3 py-0.5 h-auto bg-destructive text-destructive-foreground hover:bg-destructive hover:brightness-110 text-ui-meta font-normal"
            type="button"
            onclick={onResolve}
          >
            Resolve
          </Button>
        {/if}
        {#if opSource === "rebase"}
          <Button
            class="cb-continue rounded-full px-3 py-0.5 h-auto text-ui-meta font-normal"
            type="button"
            disabled={continuing || !rebaseMonitor.paused || rebaseMonitor.inConflict}
            title={rebaseMonitor.inConflict
              ? "Resolve the conflicted files first"
              : "Continue the paused rebase"}
            onclick={() => void onContinueRebase()}
          >
            {continuing ? "Continuing…" : "Continue"}
          </Button>
          {#if rebaseMonitor.skippable}
            <!-- M12: rebase_skip — drop the conflicted commit / advance past
                 a failed exec. Gated on the backend pause state (a skip
                 against anything else errors backend-side). -->
            <Button
              class="cb-skip rounded-full border-current px-3 py-0.5 h-auto bg-transparent text-ui-meta font-normal text-inherit hover:bg-foreground/10 hover:text-inherit dark:bg-transparent dark:border-current dark:hover:bg-foreground/10"
              variant="outline"
              type="button"
              disabled={continuing}
              title={rebaseMonitor.state?.paused_for_exec
                ? "Skip the failed exec step (nothing is recorded)"
                : "Skip this commit — drop it from the rebase"}
              onclick={() => void onSkipRebase()}
            >
              Skip this commit
            </Button>
          {/if}
        {/if}
        <Button
          variant="outline"
          class="cb-abort rounded-full border-current px-3 py-0.5 h-auto bg-transparent text-ui-meta font-normal text-inherit hover:bg-foreground/10 hover:text-inherit dark:bg-transparent dark:border-current dark:hover:bg-foreground/10 disabled:cursor-not-allowed disabled:opacity-55"
          type="button"
          disabled={aborting}
          onclick={() => void onAbort()}
        >
          {aborting ? "Aborting…" : "Abort"}
        </Button>
      </span>
    </div>
  {/if}

  <BisectBanner {repoId} showLauncher={false} />

  <!-- Width of the collapsed-history rail. SplitContainer reads it to keep
       the left panes at their size while the rail is out (see its rightRail
       doc); the rail button itself uses the same variable. `relative` makes
       this frame the containing block for the §10 overlay drawers: their
       hosts stay inside the split tree (keep-alive) but position against
       this frame, below the repo header and beside the activity rail.
       Stacked turns the frame into a column: panes above, the bottom tab
       bar below (the rail's below-the-floor replacement). -->
  <div
    class="main-area relative flex min-h-0 min-w-0 flex-1 [--history-rail-width:1.75rem] wm-stacked:flex-col"
  >
    <ActivityBar
      panels={activityPanels}
      pinned={railPins.pins}
      active={activityPanel}
      drawerActive={{
        files: drawerPanelIds.has("files") && drawerOpen.files,
        status: drawerPanelIds.has("status") && drawerOpen.status,
      }}
      historyActive={historyVisible}
      historyCanHide={canHideHistory}
      collapsed={collapsed.left}
      canCollapse={canCollapseLeft}
      badges={{ status: changeCount }}
      onSelect={selectActivityPanel}
      onHistory={selectHistoryActivity}
      onToggleCollapse={toggleLeftSidebar}
      onTogglePin={(panel) => railPins.toggle(panel)}
    />
    <!-- Stacked (`wm-stacked:relative`): the panes' containing block for
         the full-screen hosts — `inset-0` fills this area, which sits
         above the bottom tab bar (a sibling inside .main-area's column). -->
    <div class="layout-area flex min-h-0 min-w-0 flex-1 wm-stacked:relative" bind:this={layoutAreaTracker.el}>
      <SplitContainer
        node={renderTree}
        renderPanel={renderPanel}
        {hiddenPanels}
        {collapsedIds}
        {drainedIds}
        drawerPanes={drawerPaneBindings}
        drawerPanels={drawerTabBindings}
        hideTabsFor={activityGroup?.id ?? null}
        canMoveLeftFor={canMoveLeftFor}
        canMoveRightFor={canMoveRightFor}
        onRatio={onRatio}
        onActivateTab={onActivateTab}
        onAddPanel={onAddPanel}
        onPanelAction={onPanelAction}
        rightRail={showHistoryRail}
      />
    </div>
    {#if showHistoryRail}
      <!-- Workstream A §5.4: a compact edge toggle button replaced the
           rotated-text rail — same 28px column (SplitContainer reads the
           width var), same single-click "Show history sidebar" action. -->
      <div
        class="history-rail flex w-[var(--history-rail-width)] flex-none flex-col items-center border-l border-border bg-popover pt-2"
      >
        <Hint label="Show history sidebar" side="left">
          {#snippet child({ props })}
            <button
              class="history-rail-toggle inline-flex size-ui-hit flex-none cursor-pointer items-center justify-center rounded-lg text-muted-foreground hover:bg-accent hover:text-primary-ink focus-visible:-outline-offset-2 focus-visible:outline-2 focus-visible:outline-primary"
              type="button"
              {...props}
              aria-label="Show history sidebar"
              onclick={showHistory}
            >
              <svg
                class="h-4 w-4 flex-none"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <path d={PANEL_META.history.icon} />
              </svg>
            </button>
          {/snippet}
        </Hint>
      </div>
    {/if}
    {#if currentWidthMode === "stacked"}
      <!-- §10 Stacked (<800): the bottom tab bar replaces the vertical
           activity rail — 5 primary views + "More" overflow. Selecting a
           view makes its keep-alive host the one visible full-screen
           route (stackedPaneClass) — no remounts, mode flips swap classes
           only. Chrome-only conditional mount: the hosts themselves stay
           untouched by this if. -->
      <StackedTabBar
        views={[...STACKED_PRIMARY_VIEWS]}
        morePanels={stackedMorePanelsList}
        active={stackedView}
        badges={{ status: changeCount }}
        onSelect={selectStackedView}
      />
    {/if}
  </div>

</div>

{#if conflictEditorOpen}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <!-- Conflict resolution overlay: centered, large, above everything. -->
  <div
    class="conflict-overlay fixed inset-0 z-60 flex items-center justify-center bg-black/45 p-8"
    role="presentation"
    onkeydown={(e) => {
      // Tab stays inside the dialog (shared focus trap).
      trapTab(conflictDialogEl, e);
    }}
  >
    <div
      class="conflict-dialog flex h-[min(48rem,100%)] w-[min(64rem,100%)] flex-col overflow-hidden rounded-lg bg-card text-card-foreground shadow-modal [&>*]:flex-1 [&>*]:min-h-0"
      role="dialog"
      aria-modal="true"
      aria-label="Resolve merge conflicts"
      tabindex="-1"
      bind:this={conflictDialogEl}
    >
      <ConflictEditor
        {repoId}
        files={conflictFiles}
        onResolved={onConflictsResolved}
        onClose={onConflictsClosed}
      />
    </div>
  </div>
{/if}

{#if quickOpenVisible}
  {#await quickOpenImport() then QuickOpenModule}
    <QuickOpenModule.default
      {repoId}
      onClose={() => (quickOpenVisible = false)}
      onPick={(path) => void openWorkdirFile(path)}
    />
  {/await}
{/if}

<CleanDialog
      {repoId}
      open={cleanOpen}
      onClose={() => (cleanOpen = false)}
      onDone={() => {
        cleanOpen = false;
        void refreshStatus(repoId);
      }}
    />
    <AuthDialog />

<PromptDialog
  bind:open={saveLayoutOpen}
  title="Save layout as"
  message="Name the preset (per-repo overlays are kept)."
  placeholder="Preset name"
  confirmLabel="Save"
  onSubmit={(value) => layouts.saveAs(root, value)}
/>

<ConfirmDialog
  bind:open={abortOpen}
  title={opSource ? `Abort the ${conflictSourceLabel(opSource).toLowerCase()}?` : "Abort?"}
  message="The repository returns to its pre-operation state."
  confirmLabel="Abort operation"
  danger
  onConfirm={() => void onAbortConfirmed()}
/>

<ConfirmDialog
  bind:open={discardOpen}
  title="Discard changes?"
  message={diffEntry
    ? `All uncommitted changes in ${diffEntry.path} are discarded.${
        checkpointsEnabled
          ? " A safety checkpoint is created first — restore it from the Undo panel."
          : " No checkpoint is created (checkpoints are disabled in Settings)."
      }`
    : "All uncommitted changes in this file are discarded."}
  confirmLabel="Discard changes"
  danger
  onConfirm={() => void discardOpenEntry()}
/>

<!-- GitHub remote override (E1 §9.1): pick the remote the GitHub flow
     resolves owner/repo from (default origin). The host of each remote and
     the resolved state are disclosed; the choice persists per repo root. -->
<DialogShell
  bind:open={forgeRemoteOpen}
  labelledBy="forge-remote-title"
  width="24rem"
>
  {#snippet children()}
    <div class="flex min-w-0 flex-col gap-2">
      <p id="forge-remote-title" class="text-ui-base font-semibold">
        GitHub remote
      </p>
      <p
        id="forge-remote-status"
        class="text-ui-meta leading-[1.45] break-words text-muted-foreground"
      >
        {forgeRemoteStatus}
      </p>
      <div class="flex flex-col gap-0.5" role="listbox" aria-label="Remotes">
        {#if forgeRemoteList.length === 0}
          <p class="text-ui-meta text-muted-foreground">No remotes configured.</p>
        {/if}
        {#each forgeRemoteList as remote (remote.name)}
          {@const effective = forgeRemoteCurrent ?? "origin"}
          <button
            class="flex w-full cursor-pointer items-center gap-2 rounded-sm px-1.5 py-1.5 text-left text-ui-meta hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            role="option"
            aria-selected={effective === remote.name}
            onclick={() => chooseForgeRemote(remote.name)}
          >
            <span
              class="inline-flex size-3 flex-none items-center justify-center rounded-full border {effective ===
              remote.name
                ? "border-primary"
                : "border-muted-foreground/50"}"
              aria-hidden="true"
            >
              {#if effective === remote.name}
                <span class="size-1.5 rounded-full bg-primary"></span>
              {/if}
            </span>
            <span class="min-w-0 flex-1 truncate font-medium">{remote.name}</span>
            <span class="flex-none text-ui-micro text-muted-foreground">
              {hostOfRemoteUrl(remote.url) ?? "—"}
            </span>
          </button>
        {/each}
      </div>
      {#if forgeRemoteCurrent !== null && forgeRemoteCurrent !== "origin"}
        <button
          class="inline-flex cursor-pointer items-center gap-1 self-start text-ui-micro text-muted-foreground underline decoration-dotted underline-offset-2 hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
          type="button"
          data-autofocus
          onclick={() => chooseForgeRemote("origin")}
        >
          Reset to origin
        </button>
      {/if}
    </div>
  {/snippet}
  {#snippet actions()}
    <Button
      class="sdlg-primary"
      variant="default"
      type="button"
      onclick={() => (forgeRemoteOpen = false)}
    >Close</Button>
  {/snippet}
</DialogShell>
