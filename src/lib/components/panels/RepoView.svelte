<script lang="ts">
  /**
   * Per-tab repo workspace (M2, extended in M3, layout-configurable in M4):
   * header facts + sync quick actions + the configurable main splitter tree
   * (SplitContainer).
   * The default tree is VSCode-like: a narrow source-control sidebar
   * (CommitBar pinned above StatusPanel) on the left, the Diff panel as the
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
  import type { Snippet } from "svelte";
  import type {
    CommitInfo,
    ConflictFile,
    FileDiff,
    RepoStatus,
    StatusEntry,
  } from "$lib/ipc/types";
  import {
    mergeAbort,
    rebaseAbort,
    sequencerAbort,
    streamDiff,
  } from "$lib/ipc/client";
  import StatusPanel from "$lib/components/panels/StatusPanel.svelte";
  import ConflictEditor from "$lib/components/merge/ConflictEditor.svelte";
  import BisectBanner from "$lib/components/bisect/BisectBanner.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import CommitBar from "$lib/components/panels/CommitBar.svelte";
  import OpLogStrip from "$lib/components/panels/OpLogStrip.svelte";
  import EditorPane from "$lib/components/panels/EditorPane.svelte";
  import { IconButton } from "$lib/components/m3";
  import Icon from "$lib/components/icons/Icon.svelte";
  import type { CompareResult } from "$lib/components/panels/compare";
  import type { HistoryCommitActions } from "$lib/components/panels/historySidebar";
  import AuthDialog from "$lib/components/AuthDialog.svelte";
  import CleanDialog from "$lib/components/actions/CleanDialog.svelte";
  import OpenWithButton from "$lib/components/header/OpenWithButton.svelte";
  import RepoQuickActions from "$lib/components/header/RepoQuickActions.svelte";
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
  import { terminals } from "$lib/terminal/terminalStore.svelte";
  import { openPanelPopout } from "$lib/layout/popout";
  import { bookmarks } from "$lib/components/graph/bookmarks.svelte";
  import {
    branchColorForRefs,
    branchColorStore,
  } from "$lib/stats/branchColors.svelte";
  import { refreshStatus } from "$lib/stores/tabs.svelte";
  import { startAuthEvents, startOpsEvents } from "$lib/stores/ops.svelte";
  import { autofetch } from "$lib/stores/autofetch.svelte";
  import { toast } from "$lib/toast";
  import { contextMenu, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import iconHistory from "@ktibow/iconset-material-symbols/history";
  import iconBisect from "@ktibow/iconset-material-symbols/alt-route";
  import iconSaveLayout from "@ktibow/iconset-material-symbols/save";
  import iconTerminal from "@ktibow/iconset-material-symbols/terminal";
  import iconClean from "@ktibow/iconset-material-symbols/cleaning-services";
  import iconResetLayout from "@ktibow/iconset-material-symbols/restart-alt";
  import iconMore from "@ktibow/iconset-material-symbols/more-horiz";
  import iconBranch from "@ktibow/iconset-material-symbols/account-tree";

  let {
    repoId,
    name,
    root,
    status,
  }: {
    repoId: string;
    name: string;
    root: string;
    status: RepoStatus | null;
  } = $props();

  let selected: string[] = $state([]);

  // --- Center editor state -------------------------------------------------

  /** Working-copy file whose diff is shown in the editor (null = none). */
  let diffEntry = $state<StatusEntry | null>(null);
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
  /** Multi-selection action toolbar state reported by the history sidebar. */
  let historyActions = $state<HistoryCommitActions | null>(null);
  /** Commit to reveal/select in history (parent navigation from the editor). */
  let pendingHistorySha = $state<string | null>(null);
  /** Guards the working-copy diff stream against stale pages. */
  let diffToken = 0;

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

  function onPresetChange(event: Event): void {
    layouts.applyPreset(root, (event.currentTarget as HTMLSelectElement).value);
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

  // --- Panel registry (PanelId → snippet) -----------------------------------

  function renderPanel(id: PanelId): Snippet {
    switch (id) {
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

  function branchLabel(s: RepoStatus): string {
    if (s.branch) return s.branch;
    if (s.detached && s.head) return `detached @ ${s.head.slice(0, 7)}`;
    return s.detached ? "detached" : "unknown";
  }

  /** Unique changed paths — a file staged *and* modified counts once. */
  const changeCount = $derived(
    status ? new Set(status.entries.map((entry) => entry.path)).size : 0,
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
    workflowRunId = runId;
    focusPanel("diff");
  }

  /** Working-copy row selected in the changes sidebar. */
  async function openDiff(entry: StatusEntry): Promise<void> {
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
    try {
      await streamDiff(
        repoId,
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
      (selectedCommit || diffEntry || compareResult || workflowRunId !== null)
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
  }

  /** History sidebar: compare results open in the center editor. */
  function onHistoryCompare(result: CompareResult): void {
    compareResult = result;
    selectedCommit = null;
    workflowRunId = null;
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
    void openPanelPopout("diff", repoId, `${name} — diff`);
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

{#snippet statusPanel()}
  <!-- VSCode-style source-control column: commit box pinned on top, the
       working-copy list scrolls underneath. -->
  <div class="changes-panel">
    <CommitBar {repoId} onCommitted={refresh} />
    <StatusPanel
      {status}
      {repoId}
      {root}
      onOpenDiff={(e) => void openDiff(e)}
      onAfterMutation={refresh}
      bind:selected
    />
  </div>
{/snippet}
{#snippet editorPanel()}
  <EditorPane
    {repoId}
    {root}
    {status}
    commit={selectedCommit}
    compare={compareResult}
    entry={diffEntry}
    diffFiles={diffFiles}
    diffLoading={diffLoading}
    diffHunkable={diffHunkable}
    historyActions={historyActions}
    workflowRunId={workflowRunId}
    onClose={clearEditor}
    onCloseCompare={() => (compareResult = null)}
    onCloseWorkflowRun={() => (workflowRunId = null)}
    onSelectParent={revealHistoryCommit}
    onPopoutCommit={selectedCommit ? popOutCommit : undefined}
    onPopoutDiff={popOutDiff}
    onPopoutWorkflowRun={workflowRunId !== null ? popOutWorkflowRun : undefined}
    onMutated={refresh}
  />
{/snippet}
{#snippet branchesPanel()}
  {#await import("$lib/components/panels/BranchPanel.svelte") then BranchPanelModule}
    <BranchPanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet remotesPanel()}
  {#await import("$lib/components/panels/RemotePanel.svelte") then RemotePanelModule}
    <RemotePanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet stashesPanel()}
  {#await import("$lib/components/panels/StashPanel.svelte") then StashPanelModule}
    <StashPanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet worktreesPanel()}
  {#await import("$lib/components/panels/WorktreePanel.svelte") then WorktreePanelModule}
    <WorktreePanelModule.default {repoId} {root} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet healthPanel()}
  {#await import("$lib/components/panels/RepoPanel.svelte") then RepoPanelModule}
    <RepoPanelModule.default {repoId} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet submodulesPanel()}
  {#await import("$lib/components/panels/SubmodulePanel.svelte") then SubmodulePanelModule}
    <SubmodulePanelModule.default {repoId} {root} onMutated={refresh} />
  {/await}
{/snippet}
{#snippet reflogPanel()}
  {#await import("$lib/components/panels/ReflogPanel.svelte") then ReflogPanelModule}
    <ReflogPanelModule.default {repoId} />
  {/await}
{/snippet}
{#snippet undoPanel()}
  {#await import("$lib/components/safety/UndoPanel.svelte") then UndoPanelModule}
    <UndoPanelModule.default {repoId} />
  {/await}
{/snippet}
{#snippet historyPanel()}
  {#await import("$lib/components/panels/HistoryView.svelte") then HistoryViewModule}
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
  {#await import("$lib/components/terminal/TerminalPanel.svelte") then TerminalPanelModule}
    <TerminalPanelModule.default {repoId} />
  {/await}
{/snippet}
{#snippet forgePanel()}
  {#await import("$lib/components/forge/ForgePanel.svelte") then ForgePanelModule}
    <ForgePanelModule.default {repoId} />
  {/await}
{/snippet}
{#snippet actionsPanel()}
  {#await import("$lib/components/actions/ActionsPanel.svelte") then ActionsPanelModule}
    <ActionsPanelModule.default {repoId} />
  {/await}
{/snippet}
{#snippet statsPanel()}
  {#await import("$lib/components/panels/StatsPanel.svelte") then StatsPanelModule}
    <StatsPanelModule.default {repoId} {root} />
  {/await}
{/snippet}
{#snippet missingPanel()}
  <aside class="missing-panel">This panel is not available.</aside>
{/snippet}

<div class="repo-workspace">
  <header class="repo-header">
    <span class="repo-identity">
      <span class="repo-name" title={name}>{name}</span>
      {#if status}
        <span class="branch-chip" title={`Current branch: ${branchLabel(status)}`}>
          <Icon icon={iconBranch} size={12} />
          <span class="branch-label">{branchLabel(status)}</span>
        </span>
      {/if}
      <RepoQuickActions {repoId} {status} onMutated={refresh} />
      <span class="repo-root" title={root}>{root}</span>
    </span>
    <span class="layout-controls">
      <OpenWithButton {root} />
      <IconButton
        icon={iconHistory}
        label={historyVisible ? "Hide history" : "Show history"}
        title={historyVisible
          ? canHideHistory
            ? "Hide the history sidebar"
            : "Expand the primary sidebar before hiding history"
          : "Show the history sidebar"}
        pressed={historyVisible}
        disabled={historyVisible && !canHideHistory}
        onclick={toggleHistory}
      />
      <select
        class="preset-select"
        aria-label="Layout preset"
        title="Layout preset"
        value={resolved.presetId}
        onchange={onPresetChange}
      >
        {#each layouts.listPresets() as preset (preset.id)}
          <option value={preset.id}>{preset.name}</option>
        {/each}
      </select>
      <IconButton
        icon={iconTerminal}
        label="Terminal"
        title="Open (or focus) this repository's terminal panel"
        onclick={onNewTerminal}
      />
      <IconButton
        icon={iconMore}
        label="More repository actions"
        title="More repository actions"
        aria-haspopup="menu"
        onclick={openHeaderMenu}
      />
    </span>
  </header>

  <OpLogStrip {repoId} />

  {#if opSource && conflictCount > 0}
    <div class="conflict-banner" role="alert">
      <span class="cb-icon" aria-hidden="true">!</span>
      <span class="cb-text">
        {conflictSourceLabel(opSource)} in progress — {conflictCount}
        conflicted file{conflictCount === 1 ? "" : "s"}
      </span>
      <span class="cb-actions">
        <button class="cb-resolve" type="button" onclick={onResolve}>
          Resolve
        </button>
        <button
          class="cb-abort"
          type="button"
          disabled={aborting}
          onclick={() => void onAbort()}
        >
          {aborting ? "Aborting…" : "Abort"}
        </button>
      </span>
    </div>
  {/if}

  <BisectBanner {repoId} showLauncher={false} />

  <div class="main-area">
    <ActivityBar
      panels={activityPanels}
      active={activityPanel}
      historyActive={historyVisible}
      historyCanHide={canHideHistory}
      collapsed={collapsed.left}
      canCollapse={canCollapseLeft}
      badges={{ status: changeCount }}
      onSelect={selectActivityPanel}
      onHistory={selectHistoryActivity}
      onToggleCollapse={toggleLeftSidebar}
    />
    <div class="layout-area">
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
        class="history-rail"
        type="button"
        aria-label="Show history sidebar"
        title="Show history sidebar"
        onclick={showHistory}
      >
        <svg
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
        <span class="history-rail-label">History</span>
      </button>
    {/if}
  </div>

</div>

{#if conflictEditorOpen}
  <!-- Conflict resolution overlay: centered, large, above everything. -->
  <div class="conflict-overlay">
    <div class="conflict-dialog">
      <ConflictEditor
        {repoId}
        files={conflictFiles}
        onResolved={onConflictsResolved}
        onClose={onConflictsClosed}
      />
    </div>
  </div>
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

<style>
  .repo-workspace {
    display: flex;
    flex-direction: column;
    /* Fill the tab content area: without flex:1 this flex item shrink-wraps
       to its content, so collapsing a sidebar would shrink the whole
       workspace instead of handing the freed width to the center pane. */
    flex: 1;
    width: 100%;
    height: 100%;
    min-height: 0;
    /* Never wider than the tab area: without this the flex min-width:auto
       floor lets panel content (wide history rows, tables) stretch the whole
       workspace past the window edge. Panels clip/scroll inside their panes. */
    min-width: 0;
    overflow: hidden;
  }

  .repo-header {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.35rem 0.5rem;
    padding: 0.3rem 0.6rem;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container, var(--m3-surface));
    font-size: 0.8125rem;
    flex: none;
    min-width: 0;
  }

  .repo-identity {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .repo-name {
    flex: 0 1 auto;
    min-width: 0;
    max-width: 15rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }

  .branch-chip {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    max-width: 16rem;
    min-width: 0;
    padding: 0.1rem 0.45rem;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
    color: var(--m3-on-secondary-container, var(--m3-secondary));
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.6875rem;
  }

  .branch-chip :global(svg) {
    flex: none;
  }

  .branch-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .layout-controls {
    margin-left: auto;
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }

  .preset-select {
    max-width: 8.5rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface);
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.6875rem;
    padding: 0.1rem 0.25rem;
  }

  .repo-root {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1 1 auto;
    min-width: 0;
  }

  @media (max-width: 980px) {
    .repo-root {
      display: none;
    }
  }

  @media (max-width: 760px) {
    .preset-select {
      display: none;
    }

    .branch-chip {
      max-width: 9rem;
    }
  }

  .missing-panel {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.8125rem;
  }

  .conflict-banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.3rem 0.75rem;
    background: var(--m3-error-container, var(--m3-error));
    color: var(--m3-on-error-container, var(--m3-on-error));
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    font-size: 0.8125rem;
    flex: none;
  }

  .cb-icon {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.1rem;
    height: 1.1rem;
    border-radius: 50%;
    border: 1.5px solid currentColor;
    font-size: 0.6875rem;
    font-weight: 700;
  }

  .cb-text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .cb-actions {
    margin-left: auto;
    flex: none;
    display: flex;
    gap: 0.375rem;
  }

  .cb-resolve,
  .cb-abort {
    border: 1px solid currentColor;
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: inherit;
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.75rem;
    cursor: pointer;
  }

  .cb-resolve {
    background: var(--m3-error, var(--m3-primary));
    color: var(--m3-on-error, var(--m3-on-primary));
    border-color: transparent;
  }

  .cb-resolve:hover {
    filter: brightness(1.08);
  }

  .cb-abort:hover:not(:disabled) {
    background: rgb(0 0 0 / 0.08);
  }

  .cb-resolve:focus-visible,
  .cb-abort:focus-visible {
    outline: 2px solid currentColor;
    outline-offset: 1px;
  }

  .cb-abort:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .main-area {
    flex: 1;
    min-height: 0;
    min-width: 0;
    display: flex;
    /* Width of the collapsed-history rail. SplitContainer reads it to keep
     * the left panes at their size while the rail is out (see its rightRail
     * doc); the rail button itself uses the same variable. */
    --history-rail-width: 1.75rem;
  }

  /* The tree owns all remaining width; collapsed sidebar panes are hidden
   * inside it, so this area grows automatically. */
  .layout-area {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
  }

  /* Visual Studio-style auto-hide tab for the collapsed history sidebar. */
  .history-rail {
    flex: none;
    width: var(--history-rail-width);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0;
    border: none;
    border-left: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container, var(--m3-surface));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    cursor: pointer;
  }

  .history-rail svg {
    width: 1rem;
    height: 1rem;
    flex: none;
  }

  .history-rail-label {
    writing-mode: vertical-rl;
    transform: rotate(180deg);
    font-size: 0.6875rem;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .history-rail:hover,
  .history-rail:focus-visible {
    color: var(--m3-primary);
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .history-rail:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  /* Source-control column: commit box + scrolling changes list. */
  .changes-panel {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .changes-panel > :global(aside) {
    flex: 1;
    min-height: 0;
  }

  .conflict-overlay {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2rem;
    background: rgb(0 0 0 / 0.45);
  }

  .conflict-dialog {
    display: flex;
    flex-direction: column;
    width: min(64rem, 100%);
    height: min(48rem, 100%);
    border-radius: var(--m3-shape-large, 16px);
    background: var(--m3-surface);
    color: var(--m3-on-surface);
    box-shadow: 0 1rem 3rem rgb(0 0 0 / 0.35);
    overflow: hidden;
  }

  .conflict-dialog > :global(*) {
    flex: 1;
    min-height: 0;
  }
</style>
