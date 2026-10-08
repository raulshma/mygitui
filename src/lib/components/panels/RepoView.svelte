<script lang="ts">
  /**
   * Per-tab repo workspace (M2, extended in M3, layout-configurable in M4):
   * header facts + the configurable main splitter tree (SplitContainer) +
   * the CommitBar fixed at the bottom + the diff pane as a fixed bottom
   * pane (collapse / pop out). The AuthDialog is mounted once here;
   * op/auth event subscriptions and the per-repo auto-fetch interval start
   * on mount.
   *
   * M4 layout: the main area renders the active layout tree from the
   * `layouts` store (per-repo overlay > preset > default). Panels are
   * mapped PanelId → `{#snippet}` here — the only place that knows both
   * sides of the registry (see `$lib/layout/index.ts`). Tab groups get
   * context menus (move / new group / hide) and a "+ Add panel" menu; the
   * header exposes the preset select, "Save layout as…", "Add panel" and
   * per-repo reset.
   *
   * M3 conflict flow (unchanged): while `repo_status` reports a merge /
   * rebase / sequencer operation in progress, the per-repo conflicts cache
   * polls `conflicts(repoId)`; when files come back, a banner across the
   * top offers Resolve (ConflictEditor overlay) and Abort (rebase_abort /
   * merge_abort / sequencer_abort, chosen by the in-progress op).
   */
  import type { Snippet } from "svelte";
  import type {
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
  import BranchPanel from "$lib/components/panels/BranchPanel.svelte";
  import RemotePanel from "$lib/components/panels/RemotePanel.svelte";
  import StashPanel from "$lib/components/panels/StashPanel.svelte";
  import WorktreePanel from "$lib/components/panels/WorktreePanel.svelte";
  import SubmodulePanel from "$lib/components/panels/SubmodulePanel.svelte";
  import ReflogPanel from "$lib/components/panels/ReflogPanel.svelte";
  import UndoPanel from "$lib/components/safety/UndoPanel.svelte";
  import ConflictEditor from "$lib/components/merge/ConflictEditor.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import PromptDialog from "$lib/components/safety/PromptDialog.svelte";
  import CommitBar from "$lib/components/panels/CommitBar.svelte";
  import HistoryView from "$lib/components/panels/HistoryView.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import AuthDialog from "$lib/components/AuthDialog.svelte";
  import TerminalPanel from "$lib/components/terminal/TerminalPanel.svelte";
  import ForgePanel from "$lib/components/forge/ForgePanel.svelte";
  import ActionsPanel from "$lib/components/actions/ActionsPanel.svelte";
  import StatsPanel from "$lib/components/panels/StatsPanel.svelte";
  import CleanDialog from "$lib/components/actions/CleanDialog.svelte";
  import CommitMessageButton from "$lib/components/ai/CommitMessageButton.svelte";
  import { ai } from "$lib/ai/ai.svelte";
  import SplitContainer from "$lib/components/layout/SplitContainer.svelte";
  import { conflictsStore } from "$lib/components/panels/conflictsStore.svelte";
  import { onUiEvent } from "$lib/palette/events";
  import {
    conflictAbortCommand,
    conflictSource,
    conflictSourceLabel,
  } from "$lib/components/panels/panelModel";
  import {
    firstTabsId,
    findPanelNode,
    moveToRightGroup,
    panelLabel,
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
  let diffFiles: FileDiff[] = $state([]);
  /** Diff pane body collapsed (header stays for restore/pop out). */
  let diffCollapsed = $state(false);

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
    if (action === "new-group-right") {
      layouts.movePanelTo(root, panel, { kind: "after", nodeId: tabsId });
      return;
    }
    // move-right-group: merge into the sibling group right of the holder.
    let failed: string | null = null;
    layouts.updateTree(root, (tree) => {
      const outcome = moveToRightGroup(tree, panel);
      if (!outcome.ok) {
        failed = outcome.reason ?? "failed";
        return null;
      }
      return outcome.tree;
    });
    if (failed === "sibling-not-a-group") {
      toast("The area to the right is not a tab group");
    } else if (failed !== null) {
      toast("No group to the right");
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

  /** Header "＋ panel" select: adds into the first tab group (or wraps). */
  function onHeaderAddPanel(event: Event): void {
    const select = event.currentTarget as HTMLSelectElement;
    const panel = select.value as PanelId;
    select.value = "";
    if (!TREE_PANELS.includes(panel)) return;
    const target = firstTabsId(resolved.layout.main) ?? undefined;
    layouts.addPanel(root, panel, target);
  }

  /**
   * Header "＋ Terminal" button: focuses the terminal tab when the panel is
   * already placed, otherwise adds it into the first tab group.
   */
  function onNewTerminal(): void {
    const holder = findPanelNode(resolved.layout.main, "terminal");
    if (holder) {
      if (holder.kind === "tabs") {
        layouts.setActiveTab(root, holder.id, holder.tabs.indexOf("terminal"));
      }
      return;
    }
    layouts.addPanel(root, "terminal", firstTabsId(resolved.layout.main) ?? undefined);
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
      case "submodules":
        return submodulesPanel;
      case "reflog":
        return reflogPanel;
      case "undo":
        return undoPanel;
      case "history":
        return historyPanel;
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

  // H2's CreatePrDialog asks for AI generation via a window event; H1's
  // runFeature answers it. One bridge, registered once per workspace mount.
  $effect(() => {
    const onGenerate = (event: Event) => {
      const detail = (event as CustomEvent).detail as
        | { repoId?: string }
        | undefined;
      const target = detail?.repoId ?? repoId;
      void (async () => {
        try {
          const { ai } = await import("$lib/ai/ai.svelte");
          const outcome = await ai.run("pr-title-body", { repoId: target });
          window.dispatchEvent(
            new CustomEvent("ai-pr-result", {
              detail: {
                subject: outcome.message?.subject ?? outcome.result.text,
                body: outcome.message?.body ?? "",
              },
            }),
          );
        } catch (err) {
          const message = err instanceof Error ? err.message : String(err);
          toast(`AI PR description failed: ${message}`, { kind: "error" });
        }
      })();
    };
    window.addEventListener("ai-generate-pr", onGenerate);
    return () => window.removeEventListener("ai-generate-pr", onGenerate);
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

  $effect(() => {
    const unlisteners = [
      onUiEvent("focus-panel", ({ panel, repoId: target }) => {
        if (target !== null && target !== repoId) return;
        // Activate the panel where it lives; add it when hidden.
        const holder = findPanelNode(resolved.layout.main, panel as PanelId);
        if (holder) {
          if (holder.kind === "tabs") {
            layouts.setActiveTab(root, holder.id, holder.tabs.indexOf(panel as PanelId));
          }
        } else {
          layouts.addPanel(root, panel as PanelId, firstTabsId(resolved.layout.main) ?? undefined);
        }
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
    diffFiles = [];
    diffCollapsed = false;
    conflictEditorOpen = false;
  });

  /**
   * Whether the working-copy diff pane may offer hunk staging/discard: only
   * when the shown file has NO staged changes, so the pane's head→worktree
   * hunks coincide with the index→workdir hunks the backend stages against.
   */
  let diffHunkable = $state(false);

  async function openDiff(entry: StatusEntry): Promise<void> {
    diffFiles = [];
    diffHunkable = entry.index === "unmodified";
    try {
      await streamDiff(repoId, "head", "worktree", (page) => {
        const match = page.filter(
          (f) => f.path === entry.path || f.old_path === entry.path,
        );
        diffFiles = match;
      }, [entry.path]);
    } catch {
      diffFiles = [];
    }
  }

  // --- Popouts ---------------------------------------------------------------

  function popOutDiff(): void {
    void openPanelPopout("diff", repoId, `${name} — diff`);
  }

  function popOutHistory(): void {
    void openPanelPopout("history", repoId, `${name} — history`);
  }

  // --- Diff pane ---------------------------------------------------------------

  const diffHeightStyle = $derived(
    diffCollapsed ? "auto" : `${Math.round(resolved.layout.diffRatio * 100)}%`,
  );

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
  <StatusPanel
    {status}
    {repoId}
    onOpenDiff={(e) => void openDiff(e)}
    onAfterMutation={refresh}
    bind:selected
  />
{/snippet}
{#snippet branchesPanel()}
  <BranchPanel {repoId} onMutated={refresh} />
{/snippet}
{#snippet remotesPanel()}
  <RemotePanel {repoId} onMutated={refresh} />
{/snippet}
{#snippet stashesPanel()}
  <StashPanel {repoId} onMutated={refresh} />
{/snippet}
{#snippet worktreesPanel()}
  <WorktreePanel {repoId} {root} onMutated={refresh} />
{/snippet}
{#snippet submodulesPanel()}
  <SubmodulePanel {repoId} {root} onMutated={refresh} />
{/snippet}
{#snippet reflogPanel()}
  <ReflogPanel {repoId} />
{/snippet}
{#snippet undoPanel()}
  <UndoPanel {repoId} />
{/snippet}
{#snippet historyPanel()}
  <HistoryView
    {repoId}
    {root}
    onPopout={popOutHistory}
    bookmarks={bookmarks.shas(root)}
    branchColors={(refs) => branchColorForRefs(refs, branchColorStore.rules(root))}
  />
{/snippet}
{#snippet terminalPanel()}
  <TerminalPanel {repoId} />
{/snippet}
{#snippet forgePanel()}
  <ForgePanel {repoId} />
{/snippet}
{#snippet actionsPanel()}
  <ActionsPanel {repoId} />
{/snippet}
{#snippet statsPanel()}
  <StatsPanel {repoId} {root} />
{/snippet}
{#snippet missingPanel()}
  <aside class="missing-panel">This panel is not available.</aside>
{/snippet}

<div class="repo-workspace">
  <header class="repo-header">
    <span class="repo-name">{name}</span>
    {#if status}
      <span class="branch">{branchLabel(status)}</span>
      <span class="aheadbehind">↑{status.ahead} ↓{status.behind}</span>
    {/if}
    <span class="layout-controls">
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
      <button
        class="layout-btn"
        type="button"
        title="Save the current layout as a preset"
        onclick={onSaveLayout}
      >
        Save layout…
      </button>
      <button
        class="layout-btn"
        type="button"
        title="Open (or focus) this repository's terminal panel"
        onclick={onNewTerminal}
      >
        ＋ Terminal
      </button>
      <button
        class="layout-btn"
        type="button"
        title="Clean untracked files (preview + undo checkpoint)"
        onclick={() => (cleanOpen = true)}
      >
        Clean…
      </button>
      {#if hiddenPanels.length > 0}
        <select
          class="preset-select"
          aria-label="Add panel"
          title="Add panel"
          value=""
          onchange={onHeaderAddPanel}
        >
          <option value="" disabled>＋ panel…</option>
          {#each hiddenPanels as panel (panel)}
            <option value={panel}>{panelLabel(panel)}</option>
          {/each}
        </select>
      {/if}
      {#if resolved.hasOverlay}
        <button
          class="layout-btn"
          type="button"
          title="Reset this repository's layout to the preset"
          onclick={onResetLayout}
        >
          Reset
        </button>
      {/if}
    </span>
    <span class="repo-root" title={root}>{root}</span>
  </header>

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

  <div class="main-area">
    <SplitContainer
      node={resolved.layout.main}
      renderPanel={renderPanel}
      {hiddenPanels}
      canMoveRightFor={canMoveRightFor}
      onRatio={onRatio}
      onActivateTab={onActivateTab}
      onAddPanel={onAddPanel}
      onPanelAction={onPanelAction}
    />
  </div>

  <div class="commit-pane" class:zen={resolved.layout.autoHideCommitBar}>
    <CommitBar {repoId} onCommitted={refresh} />
  </div>

  {#if diffFiles.length > 0}
    <div class="diff-pane" style:height={diffHeightStyle}>
      <header class="diff-head">
        <span class="diff-title">Working copy diff</span>
        <span class="diff-count">
          {diffFiles.length} file{diffFiles.length === 1 ? "" : "s"}
        </span>
        <span class="diff-actions">
          <button
            class="layout-btn"
            type="button"
            onclick={() => (diffCollapsed = !diffCollapsed)}
          >
            {diffCollapsed ? "Show" : "Collapse"}
          </button>
          <button class="layout-btn" type="button" onclick={popOutDiff}>
            Pop out
          </button>
          <button
            class="layout-btn"
            type="button"
            aria-label="Close diff"
            title="Close diff"
            onclick={() => (diffFiles = [])}
          >
            ×
          </button>
        </span>
      </header>
      {#if !diffCollapsed}
        <div class="diff-body">
          <DiffViewer
            files={diffFiles}
            {repoId}
            hunkStaging={diffHunkable ? "stage" : undefined}
            hunkDiscard={diffHunkable}
            onMutated={refresh}
          />
        </div>
      {/if}
    </div>
  {/if}
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
    gap: 0.75rem;
    padding: 0.375rem 0.75rem;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container, var(--m3-surface));
    font-size: 0.8125rem;
    flex: none;
    min-width: 0;
  }

  .repo-name {
    font-weight: 500;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .branch {
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-primary);
  }

  .aheadbehind {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .layout-controls {
    margin-left: auto;
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.375rem;
  }

  .preset-select {
    max-width: 9rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface);
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.6875rem;
    padding: 0.1rem 0.25rem;
  }

  .layout-btn {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.6875rem;
    padding: 0.1rem 0.45rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .layout-btn:hover,
  .layout-btn:focus-visible {
    color: var(--m3-primary);
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .repo-root {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 0 1 auto;
    min-width: 0;
    max-width: 22rem;
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
    display: flex;
  }

  .commit-pane {
    flex: none;
  }

  /* Zen layouts collapse the commit bar to a slim strip; hovering the
     area expands it again (no state — pure CSS). */
  .commit-pane.zen {
    height: 1.25rem;
    overflow: hidden;
  }

  .commit-pane.zen:hover,
  .commit-pane.zen:focus-within {
    height: auto;
  }

  .diff-pane {
    flex: none;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    overflow: hidden;
  }

  .diff-head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.2rem 0.75rem;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    font-size: 0.75rem;
  }

  .diff-title {
    font-weight: 500;
  }

  .diff-count {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .diff-actions {
    margin-left: auto;
    display: flex;
    gap: 0.25rem;
  }

  .diff-body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
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
