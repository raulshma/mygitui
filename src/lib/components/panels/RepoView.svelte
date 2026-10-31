<script module lang="ts">
  /**
   * Dynamic panel imports cached at module scope. Inside a reactive snippet
   * a fresh `import()` call creates a new promise on every evaluation, and
   * the surrounding {#await} tears the panel down and remounts it — the
   * file tree re-listed its directories on every status snapshot (each
   * listing used to open a fresh SFTP channel on remote repos). A stable
   * promise reference renders the await block once.
   */
const FilesPanelImport = import("$lib/components/panels/FilesPanel.svelte");
const BranchPanelImport = import("$lib/components/panels/BranchPanel.svelte");
const RemotePanelImport = import("$lib/components/panels/RemotePanel.svelte");
const StashPanelImport = import("$lib/components/panels/StashPanel.svelte");
const WorktreePanelImport = import("$lib/components/panels/WorktreePanel.svelte");
const RepoPanelImport = import("$lib/components/panels/RepoPanel.svelte");
const SubmodulePanelImport = import("$lib/components/panels/SubmodulePanel.svelte");
const ReflogPanelImport = import("$lib/components/panels/ReflogPanel.svelte");
const UndoPanelImport = import("$lib/components/safety/UndoPanel.svelte");
const HistoryViewImport = import("$lib/components/panels/HistoryView.svelte");
const TerminalPanelImport = import("$lib/components/terminal/TerminalPanel.svelte");
const ForgePanelImport = import("$lib/components/forge/ForgePanel.svelte");
const ActionsPanelImport = import("$lib/components/actions/ActionsPanel.svelte");
const StatsPanelImport = import("$lib/components/panels/StatsPanel.svelte");
const QuickOpenImport = import("$lib/components/files/QuickOpen.svelte");
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
    discard,
    mergeAbort,
    rebaseAbort,
    sequencerAbort,
    stage,
    statFile,
    streamDiff,
  } from "$lib/ipc/client";
  import type { OpenFileInfo } from "$lib/components/files/FileWorkspace.svelte";
  import StatusPanel from "$lib/components/panels/StatusPanel.svelte";
  import ConflictEditor from "$lib/components/merge/ConflictEditor.svelte";
  import BisectBanner from "$lib/components/bisect/BisectBanner.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import CommitBar from "$lib/components/panels/CommitBar.svelte";
  import OpLogStrip from "$lib/components/panels/OpLogStrip.svelte";
  import EditorPane from "$lib/components/panels/EditorPane.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import type { CompareResult } from "$lib/components/panels/compare";
  import type { HistoryCommitActions } from "$lib/components/panels/historySidebar";
  import AuthDialog from "$lib/components/AuthDialog.svelte";
  import CleanDialog from "$lib/components/actions/CleanDialog.svelte";
  import OpenWithButton from "$lib/components/header/OpenWithButton.svelte";
  import RepoQuickActions from "$lib/components/header/RepoQuickActions.svelte";
  import BranchSwitcher from "$lib/components/header/BranchSwitcher.svelte";
  // Startup: every panel that is not part of the first paint (status is the
  // default tab; diff and CommitBar are always visible) is dynamically
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
    PANEL_META,
    rightSiblingOf,
    TREE_PANELS,
    visiblePanels,
    type PanelGroupAction,
    type PanelId,
  } from "$lib/layout/layoutModel";
  import { layouts } from "$lib/layout/layout.svelte";
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
  import { railPins } from "$lib/stores/railPins.svelte";
  import { startAuthEvents, startOpsEvents } from "$lib/stores/ops.svelte";
  import { autofetch } from "$lib/stores/autofetch.svelte";
  import { remoteDevices } from "$lib/stores/remoteDevices.svelte";
  import { toast } from "$lib/toast";
  import { contextMenu, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import iconHistory from "@ktibow/iconset-material-symbols/history";
  import iconBisect from "@ktibow/iconset-material-symbols/alt-route";
  import iconSaveLayout from "@ktibow/iconset-material-symbols/save";
  import iconTerminal from "@ktibow/iconset-material-symbols/terminal";
  import iconClean from "@ktibow/iconset-material-symbols/cleaning-services";
  import iconResetLayout from "@ktibow/iconset-material-symbols/restart-alt";
  import iconMore from "@ktibow/iconset-material-symbols/more-horiz";
  import iconHome from "@ktibow/iconset-material-symbols/home-rounded";

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

  let selected: string[] = $state([]);
  let hasSubmodules = $state(false);

  // --- Center editor state -------------------------------------------------

  /** Working-copy file whose diff is shown in the editor (null = none). */
  let diffEntry = $state<StatusEntry | null>(null);
  let diffRepoId = $state<string | null>(null);
  let diffRoot = $state<string | null>(null);
  let diffRepoName = $state<string | null>(null);
  let diffFiles: FileDiff[] = $state([]);
  let diffLoading = $state(false);
  /** Hunk staging is only safe when the file has no staged changes. */
  let diffHunkable = $state(false);
  /** Commit selected in the history sidebar. */
  let selectedCommit = $state<CommitInfo | null>(null);
  /** Compare result selected in the history sidebar. */
  let compareResult = $state<CompareResult | null>(null);
  /** GitHub Actions run shown in the editor (from the forge sidebar). */
  let workflowRunId = $state<number | null>(null);
  /** Custom action run shown in the editor (from the actions panel). */
  let actionRunId = $state<string | null>(null);
  /** Multi-selection action toolbar state reported by the history sidebar. */
  let historyActions = $state<HistoryCommitActions | null>(null);
  /** Commit to reveal/select in history (parent navigation from the editor). */
  let pendingHistorySha = $state<string | null>(null);
  /** Guards the working-copy diff stream against stale pages. */
  let diffToken = 0;

  // --- File workspace state (open workdir files, VS Code-style) -------------

  /** Open files: tab order = open order; each keeps its own dirty/meta. */
  let openFiles = $state<OpenFileInfo[]>([]);
  /** Active (visible) workspace tab. */
  let activeFile = $state<string | null>(null);
  /** Center shows the file workspace (vs the diff/commit views). */
  let filesMode = $state(false);
  /** Quick-open (Ctrl+P) overlay visibility. */
  let quickOpenVisible = $state(false);

  /** Open (or focus) a workdir file in the center workspace. */
  async function openWorkdirFile(path: string): Promise<void> {
    const existing = openFiles.find((f) => f.path === path);
    if (existing) {
      activeFile = path;
      filesMode = true;
      return;
    }
    const file: OpenFileInfo = { path, dirty: false, meta: null };
    openFiles.push(file);
    activeFile = path;
    filesMode = true;
    diffEntry = null;
    selectedCommit = null;
    compareResult = null;
    workflowRunId = null;
    actionRunId = null;
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
    if (openFiles.length === 0) filesMode = false;
  }

  function workdirDiffFor(path: string): void {
    const entry =
      status?.entries.find((e) => e.path === path || e.old_path === path) ??
      null;
    if (!entry) {
      toast(`No working-copy changes for ${path}`);
      return;
    }
    filesMode = false;
    void openDiff(entry);
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

  /** Whether the history panel is actually on screen (not behind a rail/tab). */
  const historyVisible = $derived.by(() => {
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
  /** Right-edge rail shown when the right sidebar is collapsed or absent. */
  const showHistoryRail = $derived(
    !historyNode || (historyInRightSidebar && collapsed.right),
  );

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

  /** Hiding history must not empty the workspace either. */
  const canHideHistory = $derived(
    historyInActivityGroup ? canCollapseLeft : canCollapseRight,
  );

  /** Pane ids hidden by the two sidebar rails; mounted, just out of layout. */
  const collapsedIds = $derived.by(() => {
    const ids: string[] = [];
    if (collapsed.left && activityGroup) ids.push(activityGroup.id);
    if (collapsed.right && historyNode && !ids.includes(historyNode.id)) {
      ids.push(historyNode.id);
    }
    return ids;
  });

  /**
   * Re-opens history (adding the panel for layouts that predate it) and
   * brings it to the front of whichever holder it lives in.
   */
  function showHistory(): void {
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

  /** Activity bar: activate a panel, adding it to the sidebar when hidden. */
  function selectActivityPanel(panel: PanelId): void {
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

  /** History rail: toggle the right sidebar (or activate it in-pane). */
  function selectHistoryActivity(): void {
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

  // --- Narrow-viewport auto-collapse (UI review P1 #5) ----------------------
  //
  // Below 1200px the history sidebar would squeeze the diff into wrapping
  // code lines, so it auto-collapses to the right-edge rail once. Widening
  // never auto-expands and a manual reopen is respected for the rest of the
  // session (the flag only resets while the workspace is wide, so the NEXT
  // narrowing may auto-collapse again — never this one).
  let narrowAutoCollapsed = $state(false);

  $effect(() => {
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

  /** Whether the tab group at `tabsId` has a group to its left. */
  function canMoveLeftFor(tabsId: string): boolean {
    return leftSiblingOf(resolved.layout.main, tabsId) !== null;
  }
  /** Whether the tab group at `tabsId` has a group to its right. */
  function canMoveRightFor(tabsId: string): boolean {
    return rightSiblingOf(resolved.layout.main, tabsId) !== null;
  }

  function onRatio(splitId: string, ratio: number): void {
    layouts.setRatio(root, splitId, ratio);
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
   *  already placed, otherwise adds it into the primary sidebar. */
  function onNewTerminal(): void {
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
      default:
        return missingPanel;
    }
  }

  // --- Clean dialog + AI feature bridge ------------------------------------

  let cleanOpen = $state(false);

  // H2's CreatePrDialog asks for AI generation via the typed UI bus; H1's
  // runFeature answers it. One bridge, registered once per workspace mount.
  $effect(() => {
    return onUiEvent("ai-generate-pr", ({ repoId: target }) => {
      void (async () => {
        try {
          const { ai } = await import("$lib/ai/ai.svelte");
          const outcome = await ai.run("pr-title-body", { repoId: target });
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

  /** ConflictEditor overlay open? */
  let conflictEditorOpen = $state(false);
  /** Abort RPC in flight (disables the Abort button). */
  let aborting = $state(false);

  /** In-progress operation (merge / rebase / sequencer), or null. */
  const opSource = $derived(status ? conflictSource(status) : null);
  /** Conflict files for the current repo (cached by the store). */
  const conflictFiles = $derived(
    opSource ? conflictsStore.files(repoId) : ([] as ConflictFile[]),
  );
  const conflictCount = $derived(conflictFiles.length);

  // While an operation is in progress, (re)fetch the conflict list; the
  // moment the status is clean, drop the cache and any open editor.
  $effect(() => {
    if (opSource) {
      void conflictsStore.load(repoId);
    } else {
      conflictsStore.clear(repoId);
      conflictEditorOpen = false;
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
   */
  function focusPanel(targetPanel: PanelId): void {
    if (targetPanel === "history") showHistory();
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
      onUiEvent("focus-panel", ({ panel, repoId: target }) => {
        if (target !== null && target !== repoId) return;
        focusPanel(panel as PanelId);
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

  // Remote repositories (device_id set) show a connection chip fed by the
  // remote-devices store (`device-status` events keep it live); the forge
  // panel is unavailable there (it shells out to a local `gh`).
  const isRemote = $derived(deviceId !== null);
  const remoteStatus = $derived(
    deviceId !== null ? remoteDevices.statusOf(deviceId) : null,
  );

  /** Header overflow: low-frequency repository tools. */
  function openHeaderMenu(event: MouseEvent): void {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const entries: MenuEntry[] = [
      {
        id: "bisect",
        label: "Start bisect…",
        icon: iconBisect,
        run: () => emitUiEvent("bisect-open-start"),
      },
      { separator: true },
      {
        id: "save-layout",
        label: "Save layout as…",
        icon: iconSaveLayout,
        run: onSaveLayout,
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
      { separator: true },
      {
        id: "recent-repos",
        label: "Recent repositories",
        icon: iconHome,
        run: showRecentRepos,
      },
    ];
    contextMenu.show(rect.right, rect.bottom, entries);
  }

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

  // Reset view state when switching repos.
  $effect(() => {
    void repoId;
    selected = [];
    diffEntry = null;
    diffFiles = [];
    diffLoading = false;
    diffHunkable = false;
    selectedCommit = null;
    compareResult = null;
    historyActions = null;
    pendingHistorySha = null;
    workflowRunId = null;
    actionRunId = null;
    conflictEditorOpen = false;
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
    diffEntry = null;
    diffFiles = [];
    diffLoading = false;
    diffHunkable = false;
    selectedCommit = null;
    compareResult = null;
    actionRunId = null;
    workflowRunId = runId;
    focusPanel("diff");
  }

  /** Opens a custom action run in the main editor (actions panel event). */
  function openActionRun(runId: string): void {
    diffToken++;
    diffEntry = null;
    diffFiles = [];
    diffLoading = false;
    diffHunkable = false;
    selectedCommit = null;
    compareResult = null;
    workflowRunId = null;
    actionRunId = runId;
    focusPanel("diff");
  }

  /** Working-copy row selected in the changes sidebar. */
  async function openDiff(
    entry: StatusEntry,
    targetRepoId?: string,
    targetRoot?: string,
    targetRepoName?: string,
  ): Promise<void> {
    const token = ++diffToken;
    diffEntry = entry;
    diffFiles = [];
    diffLoading = true;
    // Only files with NO staged changes can use head→worktree hunks for
    // staging: there the pane's hunks coincide with index→workdir.
    diffHunkable = entry.index === "unmodified";
    selectedCommit = null;
    compareResult = null;
    workflowRunId = null;
    actionRunId = null;
    filesMode = false;
    diffRepoId = targetRepoId ?? repoId;
    diffRoot = targetRoot ?? root;
    diffRepoName = targetRepoName ?? "";
    try {
      await streamDiff(
        diffRepoId,
        "head",
        "worktree",
        (page) => {
          if (token !== diffToken) return;
          diffFiles = page.filter(
            (f) => f.path === entry.path || f.old_path === entry.path,
          );
        },
        [entry.path],
      );
    } catch {
      if (token === diffToken) diffFiles = [];
    } finally {
      if (token === diffToken) diffLoading = false;
    }
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
    autoOpenedFor = repoId;
    const prefs = root ? loadHistoryPrefs(root) : null;
    if (prefs?.selectedSha) return;
    const first = status.entries[0];
    if (
      first &&
      diffEntry === null &&
      selectedCommit === null &&
      compareResult === null &&
      workflowRunId === null &&
      actionRunId === null
    ) {
      void openDiff(first);
    }
  });

  /** Keep the open diff's kinds (and hunk staging safety) in sync with the
   *  refreshed status — staging from the header flips the entry state. */
  $effect(() => {
    const current = diffEntry;
    if (!current || diffRepoId !== repoId || !status) return;
    const fresh = status.entries.find((e) => e.path === current.path);
    if (!fresh) return;
    diffHunkable = fresh.index === "unmodified";
    if (fresh.index !== current.index || fresh.worktree !== current.worktree) {
      diffEntry = fresh;
    }
  });

  /** Header ↑/↓: step through changed files (wraps around). Main-repo
   *  diffs only — submodule panes have no entry in this status list. */
  function openAdjacentChange(delta: 1 | -1): void {
    const list = changeEntries;
    if (!diffEntry || diffRepoId !== repoId || list.length < 2) return;
    const at = list.findIndex((e) => e.path === diffEntry?.path);
    const next = list[(Math.max(at, 0) + delta + list.length) % list.length]!;
    void openDiff(next);
  }

  /** Welcome pane: open the first changed file. */
  function openFirstChange(): void {
    const first = changeEntries[0];
    if (first) void openDiff(first);
  }

  /** Header Stage/Unstage for the open file. */
  async function stageOpenEntry(unstage: boolean): Promise<void> {
    const current = diffEntry;
    if (!current || diffRepoId !== repoId) return;
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
    }
  }

  /** Discard confirmation for the open file (discard checkpoints upstream). */
  let discardOpen = $state(false);

  function requestDiscardOpenEntry(): void {
    if (diffEntry !== null) discardOpen = true;
  }

  async function discardOpenEntry(): Promise<void> {
    const current = diffEntry;
    const targetRepoId = diffRepoId ?? repoId;
    if (!current) return;
    try {
      await discard(targetRepoId, [{ file: current.path }]);
      discardOpen = false;
      toast(`Discarded changes in ${current.path}`, { kind: "success" });
      refresh();
      if (diffRepoId && diffRepoId !== repoId) void refreshStatus(diffRepoId);
      void openDiff(
        current,
        diffRepoId ?? undefined,
        diffRoot ?? undefined,
        diffRepoName ?? undefined,
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
    diffEntry = null;
    diffFiles = [];
    diffLoading = false;
    diffHunkable = false;
    selectedCommit = null;
    compareResult = null;
    workflowRunId = null;
    actionRunId = null;
    diffRepoId = null;
    diffRoot = null;
    diffRepoName = null;
  }

  /** History sidebar: a commit row was selected (or restored on mount). */
  function onHistorySelect(
    info: CommitInfo | null,
    source?: "restore",
  ): void {
    // A restored selection only opens when nothing is in the editor: after
    // history is collapsed/reopened it must not replace an open file diff.
    if (
      source === "restore" &&
      (selectedCommit ||
        diffEntry ||
        compareResult ||
        workflowRunId !== null ||
        actionRunId !== null)
    ) {
      return;
    }
    selectedCommit = info;
    if (!info) return;
    diffToken++;
    diffEntry = null;
    diffFiles = [];
    diffLoading = false;
    diffHunkable = false;
    compareResult = null;
    workflowRunId = null;
    actionRunId = null;
  }

  /** History sidebar: compare results open in the center editor. */
  function onHistoryCompare(result: CompareResult): void {
    compareResult = result;
    selectedCommit = null;
    workflowRunId = null;
    actionRunId = null;
    diffToken++;
    diffEntry = null;
    diffFiles = [];
    diffLoading = false;
    diffHunkable = false;
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
  {#await FilesPanelImport then FilesPanelModule}
    <FilesPanelModule.default
      {repoId}
      {root}
      {status}
      activeFile={filesMode ? activeFile : null}
    />
  {/await}
{/snippet}
{#snippet statusPanel()}
  <!-- VSCode-style source-control column: changes scroll above the pinned
       commit box. -->
  <div
    class="changes-panel flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
  >
    <StatusPanel
      {status}
      {repoId}
      {root}
      repoName={name}
      onOpenDiff={(e, subRepoId, subRoot, subName) => void openDiff(e, subRepoId, subRoot, subName)}
      onAfterMutation={refresh}
      bind:selected
      bind:hasSubmodules
    />
    {#if !hasSubmodules}
      <CommitBar {repoId} onCommitted={refresh} />
    {/if}
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
    diffFiles={diffFiles}
    diffLoading={diffLoading}
    diffHunkable={diffHunkable}
    historyActions={historyActions}
    workflowRunId={workflowRunId}
    actionRunId={actionRunId}
    entryIndex={diffEntry && diffRepoId === repoId
      ? changeEntries.findIndex((e) => e.path === diffEntry?.path)
      : null}
    entryCount={diffRepoId === repoId ? changeEntries.length : 0}
    files={filesMode && openFiles.length > 0
      ? { repoId, root, openFiles, active: activeFile }
      : null}
    onClose={clearEditor}
    onCloseCompare={() => (compareResult = null)}
    onCloseWorkflowRun={() => (workflowRunId = null)}
    onCloseActionRun={() => (actionRunId = null)}
    onSelectParent={revealHistoryCommit}
    onPopoutCommit={selectedCommit ? popOutCommit : undefined}
    onPopoutDiff={popOutDiff}
    onPopoutWorkflowRun={workflowRunId !== null ? popOutWorkflowRun : undefined}
    onPopoutActionRun={actionRunId !== null ? popOutActionRun : undefined}
    onMutated={onEditorMutated}
    onPrevFile={() => openAdjacentChange(-1)}
    onNextFile={() => openAdjacentChange(1)}
    onStageEntry={diffRepoId === repoId
      ? (unstage) => void stageOpenEntry(unstage)
      : undefined}
    onDiscardEntry={diffRepoId === repoId ? requestDiscardOpenEntry : undefined}
    onEditEntry={diffRepoId === repoId && diffEntry
      ? () => void openWorkdirFile(diffEntry!.path)
      : undefined}
    onActivateFile={(path) => (activeFile = path)}
    onCloseFile={closeWorkdirFile}
    onOpenDiffFile={workdirDiffFor}
    onOpenFirstChange={openFirstChange}
    onBrowseHistory={() => focusPanel("history")}
  />
{/snippet}
{#snippet branchesPanel()}
  {#await BranchPanelImport then BranchPanelModule}
    <BranchPanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet remotesPanel()}
  {#await RemotePanelImport then RemotePanelModule}
    <RemotePanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet stashesPanel()}
  {#await StashPanelImport then StashPanelModule}
    <StashPanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet worktreesPanel()}
  {#await WorktreePanelImport then WorktreePanelModule}
    <WorktreePanelModule.default {repoId} {root} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet healthPanel()}
  {#await RepoPanelImport then RepoPanelModule}
    <RepoPanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet submodulesPanel()}
  {#await SubmodulePanelImport then SubmodulePanelModule}
    <SubmodulePanelModule.default {repoId} {root} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet reflogPanel()}
  {#await ReflogPanelImport then ReflogPanelModule}
    <ReflogPanelModule.default {repoId} />
  {/await}
{/snippet}
{#snippet undoPanel()}
  {#await UndoPanelImport then UndoPanelModule}
    <UndoPanelModule.default {repoId} />
  {/await}
{/snippet}
{#snippet historyPanel()}
  {#await HistoryViewImport then HistoryViewModule}
    <HistoryViewModule.default
      {repoId}
      {root}
      variant="sidebar"
      onPopout={popOutHistory}
      bookmarks={bookmarks.shas(root)}
      branchColors={(refs) => branchColorForRefs(refs, branchColorStore.rules(root))}
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
  {#await TerminalPanelImport then TerminalPanelModule}
    <TerminalPanelModule.default {repoId} remote={isRemote} />
  {/await}
{/snippet}
{#snippet forgePanel()}
  {#if isRemote}
    <aside
      class="missing-panel flex flex-1 items-center justify-center text-[0.8125rem] text-muted-foreground"
      title="Not available for remote projects"
    >
      Not available for remote projects
    </aside>
  {:else}
    {#await ForgePanelImport then ForgePanelModule}
      <ForgePanelModule.default {repoId} />
    {/await}
  {/if}
{/snippet}
{#snippet actionsPanel()}
  {#await ActionsPanelImport then ActionsPanelModule}
    <ActionsPanelModule.default {repoId} />
  {/await}
{/snippet}
{#snippet statsPanel()}
  {#await StatsPanelImport then StatsPanelModule}
    <StatsPanelModule.default {repoId} {root} />
  {/await}
{/snippet}
{#snippet missingPanel()}
  <aside
    class="missing-panel flex flex-1 items-center justify-center text-[0.8125rem] text-muted-foreground"
  >This panel is not available.</aside>
{/snippet}

<!-- Fills the tab content area: without flex-1 this flex item shrink-wraps to
     its content, so collapsing a sidebar would shrink the whole workspace
     instead of handing the freed width to the center pane. min-width:0 keeps
     wide panel content from stretching the workspace past the window edge. -->
<div
  class="repo-workspace flex h-full w-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
>
  <header
    class="repo-header flex min-w-0 flex-none flex-wrap items-center gap-x-2 gap-y-1.5 border-b border-border bg-popover px-[0.6rem] py-[0.3rem] text-[0.8125rem]"
  >
    <span class="repo-identity flex min-w-0 flex-1 items-center gap-[0.4rem]">
      <span
        class="repo-name max-w-[15rem] min-w-0 flex-[0_1_auto] overflow-hidden text-ellipsis whitespace-nowrap font-semibold"
        title={name}
      >{name}</span>
      {#if remoteStatus}
        <!-- Remote repository: connection chip fed by the devices store. -->
        <Badge
          variant="secondary"
          class="remote-chip flex-none gap-[0.3rem] rounded-full bg-accent px-[0.45rem] py-[0.1rem] h-auto text-[0.6875rem] {remoteStatus === 'connected' ? 'text-foreground' : 'text-muted-foreground'}"
          data-status={remoteStatus}
          title={remoteStatus === "connected"
            ? "Device connected"
            : remoteStatus === "connecting"
              ? "Device connecting…"
              : "Device offline"}
        >
          <span
            class="remote-dot h-[0.45rem] w-[0.45rem] flex-none rounded-full {remoteStatus === 'connected' ? 'bg-primary' : remoteStatus === 'connecting' ? 'bg-muted-foreground' : 'bg-border'}"
            aria-hidden="true"
          ></span>
          <span class="remote-label">
            {remoteStatus === "connected"
              ? "connected"
              : remoteStatus === "connecting"
                ? "connecting…"
                : "offline"}
          </span>
        </Badge>
      {/if}
      <BranchSwitcher {repoId} {status} onMutated={refresh} />
      <RepoQuickActions {repoId} {status} onMutated={refresh} />
      <span
        class="repo-root max-[980px]:hidden min-w-0 flex-[1_1_auto] overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.6875rem] text-muted-foreground"
        title={root}
      >{root}</span>
    </span>
    <span class="layout-controls ml-auto flex flex-none items-center gap-1">
      <OpenWithButton {root} />
      <Button
        class="text-muted-foreground hover:bg-accent hover:text-accent-foreground aria-pressed:bg-accent aria-pressed:text-accent-foreground"
        variant="ghost"
        size="icon-sm"
        type="button"
        aria-label={historyVisible ? "Hide history" : "Show history"}
        aria-pressed={historyVisible}
        title={historyVisible
          ? canHideHistory
            ? "Hide the history sidebar"
            : "Expand the primary sidebar before hiding history"
          : "Show the history sidebar"}
        disabled={historyVisible && !canHideHistory}
        onclick={toggleHistory}
      >
        <Icon icon={iconHistory} size={16} />
      </Button>
      <Select.Root
        type="single"
        value={resolved.presetId}
        onValueChange={(value) => layouts.applyPreset(root, value)}
      >
        <Select.Trigger
          class="preset-select max-[760px]:hidden max-w-[8.5rem] h-[1.35rem] rounded-sm border-border bg-card dark:bg-card px-1 py-0 text-[0.6875rem] text-card-foreground gap-1"
          aria-label="Layout preset"
          title="Layout preset"
        >
          <Select.Value />
        </Select.Trigger>
        <Select.Content>
          {#each layouts.listPresets() as preset (preset.id)}
            <Select.Item value={preset.id} label={preset.name} class="text-[0.6875rem]" />
          {/each}
        </Select.Content>
      </Select.Root>
      <Button
        class="text-muted-foreground hover:bg-accent hover:text-accent-foreground"
        variant="ghost"
        size="icon-sm"
        type="button"
        aria-label="Terminal"
        title="Open (or focus) this repository's terminal panel"
        onclick={onNewTerminal}
      >
        <Icon icon={iconTerminal} size={16} />
      </Button>
      <Button
        class="text-muted-foreground hover:bg-accent hover:text-accent-foreground"
        variant="ghost"
        size="icon-sm"
        type="button"
        aria-label="More repository actions"
        title="More repository actions"
        aria-haspopup="menu"
        onclick={openHeaderMenu}
      >
        <Icon icon={iconMore} size={16} />
      </Button>
    </span>
  </header>

  <OpLogStrip {repoId} />

  {#if opSource && conflictCount > 0}
    <div
      class="conflict-banner flex flex-none items-center gap-2 border-b border-border bg-destructive/15 px-3 py-[0.3rem] text-[0.8125rem] text-destructive"
      role="alert"
    >
      <span
        class="cb-icon inline-flex h-[1.1rem] w-[1.1rem] flex-none items-center justify-center rounded-full border-[1.5px] border-current text-[0.6875rem] font-bold"
        aria-hidden="true"
      >!</span>
      <span
        class="cb-text min-w-0 overflow-hidden text-ellipsis whitespace-nowrap"
      >
        {conflictSourceLabel(opSource)} in progress — {conflictCount}
        conflicted file{conflictCount === 1 ? "" : "s"}
      </span>
      <span class="cb-actions ml-auto flex flex-none gap-1.5">
        <Button
          class="cb-resolve rounded-full px-3 py-[0.15rem] h-auto bg-destructive text-white hover:bg-destructive hover:brightness-110 text-[0.72rem] font-normal"
          type="button"
          onclick={onResolve}
        >
          Resolve
        </Button>
        <Button
          variant="outline"
          class="cb-abort rounded-full border-current px-3 py-[0.15rem] h-auto bg-transparent text-[0.72rem] font-normal text-inherit hover:bg-foreground/10 hover:text-inherit dark:bg-transparent dark:border-current dark:hover:bg-foreground/10 disabled:cursor-not-allowed disabled:opacity-55"
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
       doc); the rail button itself uses the same variable. -->
  <div
    class="main-area flex min-h-0 min-w-0 flex-1 [--history-rail-width:1.75rem]"
  >
    <ActivityBar
      panels={activityPanels}
      pinned={railPins.pins}
      active={activityPanel}
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
    <div class="layout-area flex min-h-0 min-w-0 flex-1">
      <SplitContainer
        node={resolved.layout.main}
        renderPanel={renderPanel}
        {hiddenPanels}
        {collapsedIds}
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
      <button
        class="history-rail flex w-[var(--history-rail-width)] flex-none cursor-pointer flex-col items-center gap-2 border-l border-border bg-popover py-2 text-muted-foreground hover:bg-accent hover:text-primary focus-visible:-outline-offset-2 focus-visible:outline-2 focus-visible:outline-primary"
        type="button"
        aria-label="Show history sidebar"
        title="Show history sidebar"
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
        <span
          class="history-rail-label rotate-180 text-[0.6875rem] uppercase tracking-[0.04em] [writing-mode:vertical-rl]"
        >History</span>
      </button>
    {/if}
  </div>

</div>

{#if conflictEditorOpen}
  <!-- Conflict resolution overlay: centered, large, above everything. -->
  <div
    class="conflict-overlay fixed inset-0 z-[60] flex items-center justify-center bg-black/45 p-8"
  >
    <div
      class="conflict-dialog flex h-[min(48rem,100%)] w-[min(64rem,100%)] flex-col overflow-hidden rounded-lg bg-card text-card-foreground shadow-[0_1rem_3rem_rgba(0,0,0,0.35)] [&>*]:flex-1 [&>*]:min-h-0"
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
  {#await QuickOpenImport then QuickOpenModule}
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
    ? `All uncommitted changes in ${diffEntry.path} are discarded. A safety checkpoint is created first — restore it from the Undo panel.`
    : "All uncommitted changes in this file are discarded."}
  confirmLabel="Discard changes"
  danger
  onConfirm={() => void discardOpenEntry()}
/>
