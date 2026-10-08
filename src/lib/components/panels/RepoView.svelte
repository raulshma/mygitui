<script lang="ts">
  /**
   * Per-tab repo workspace (M2, extended in M3): header facts + a tabbed
   * left panel stack (Status | Branches | Remotes | Stashes | Worktrees |
   * Reflog | Undo) beside the history view, with the CommitBar fixed at
   * the bottom (above the diff pane). The AuthDialog is mounted once here;
   * op/auth event subscriptions and the per-repo auto-fetch interval start
   * on mount. Selected-file diffs still open in the DiffViewer below.
   *
   * M3 conflict flow: while `repo_status` reports a merge / rebase /
   * sequencer operation in progress, the per-repo conflicts cache polls
   * `conflicts(repoId)`; when files come back, a banner across the top of
   * the workspace offers Resolve (opens the ConflictEditor overlay) and
   * Abort (merge_abort, or rebase_abort when the rebase flag is up).
   */
  import type {
    ConflictFile,
    FileDiff,
    RepoStatus,
    StatusEntry,
  } from "$lib/ipc/types";
  import { mergeAbort, rebaseAbort, streamDiff } from "$lib/ipc/client";
  import StatusPanel from "$lib/components/panels/StatusPanel.svelte";
  import BranchPanel from "$lib/components/panels/BranchPanel.svelte";
  import RemotePanel from "$lib/components/panels/RemotePanel.svelte";
  import StashPanel from "$lib/components/panels/StashPanel.svelte";
  import WorktreePanel from "$lib/components/panels/WorktreePanel.svelte";
  import ReflogPanel from "$lib/components/panels/ReflogPanel.svelte";
  import UndoPanel from "$lib/components/safety/UndoPanel.svelte";
  import ConflictEditor from "$lib/components/merge/ConflictEditor.svelte";
  import CommitBar from "$lib/components/panels/CommitBar.svelte";
  import HistoryView from "$lib/components/panels/HistoryView.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import AuthDialog from "$lib/components/AuthDialog.svelte";
  import { conflictsStore } from "$lib/components/panels/conflictsStore.svelte";
  import {
    conflictAbortCommand,
    conflictSource,
    conflictSourceLabel,
  } from "$lib/components/panels/panelModel";
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

  type PanelTab =
    | "status"
    | "branches"
    | "remotes"
    | "stashes"
    | "worktrees"
    | "reflog"
    | "undo";

  /**
   * Compact icon+label tab registry; icons are stroke paths on a 24×24
   * grid (Feather-style geometry), rendered `aria-hidden` next to the
   * label. Arrow-key tablist semantics are handled in `onTablistKeydown`.
   */
  const PANEL_TABS: Array<{ id: PanelTab; label: string; icon: string }> = [
    {
      id: "status",
      label: "Status",
      icon: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
    },
    {
      id: "branches",
      label: "Branches",
      icon: "M6 3v12M21 6a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM18 9a9 9 0 0 1-9 9",
    },
    {
      id: "remotes",
      label: "Remotes",
      icon: "M18 10h-1.26A8 8 0 1 0 9 20h9a5 5 0 0 0 0-10z",
    },
    {
      id: "stashes",
      label: "Stashes",
      icon: "M21 8v13H3V8M1 3h22v5H1zM10 12h4",
    },
    {
      id: "worktrees",
      label: "Worktrees",
      icon: "M20 9h-9a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2v-9a2 2 0 0 0-2-2zM5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1",
    },
    {
      id: "reflog",
      label: "Reflog",
      icon: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 6v6l4 2",
    },
    {
      id: "undo",
      label: "Undo",
      icon: "M9 14L4 9l5-5M20 20v-7a4 4 0 0 0-4-4H4",
    },
  ];

  let panelTab = $state<PanelTab>("status");
  let selected: string[] = $state([]);
  let diffFiles: FileDiff[] = $state([]);

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

  // Reset view state when switching repos.
  $effect(() => {
    void repoId;
    panelTab = "status";
    selected = [];
    diffFiles = [];
    conflictEditorOpen = false;
  });

  function onTablistKeydown(event: KeyboardEvent): void {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const idx = PANEL_TABS.findIndex((t) => t.id === panelTab);
    const dir = event.key === "ArrowRight" ? 1 : -1;
    const next = PANEL_TABS[(idx + dir + PANEL_TABS.length) % PANEL_TABS.length];
    panelTab = next.id;
    // Move focus to the newly selected tab button.
    requestAnimationFrame(() => {
      document
        .getElementById(`panel-tab-${next.id}`)
        ?.focus();
    });
  }

  async function openDiff(entry: StatusEntry): Promise<void> {
    diffFiles = [];
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

  /** Aborts the in-progress operation (rebase → rebase_abort, else merge_abort). */
  async function onAbort(): Promise<void> {
    if (!opSource || aborting) return;
    const label = conflictSourceLabel(opSource);
    const command = conflictAbortCommand(opSource);
    if (
      !window.confirm(
        `Abort the ${label.toLowerCase()}? The repository returns to its pre-operation state.`,
      )
    ) {
      return;
    }
    aborting = true;
    try {
      if (command === "rebase_abort") {
        await rebaseAbort(repoId);
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

<div class="repo-workspace">
  <header class="repo-header">
    <span class="repo-name">{name}</span>
    {#if status}
      <span class="branch">{branchLabel(status)}</span>
      <span class="aheadbehind">↑{status.ahead} ↓{status.behind}</span>
    {/if}
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

  <div class="split">
    <div class="left">
      <div
        class="panel-tabs"
        role="tablist"
        aria-label="Repository panels"
        tabindex="-1"
        onkeydown={onTablistKeydown}
      >
        {#each PANEL_TABS as tab (tab.id)}
          <button
            id={`panel-tab-${tab.id}`}
            class="panel-tab"
            type="button"
            role="tab"
            aria-selected={panelTab === tab.id}
            aria-controls="panel-tabpanel"
            tabindex={panelTab === tab.id ? 0 : -1}
            onclick={() => (panelTab = tab.id)}
          >
            <svg
              class="tab-icon"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path d={tab.icon} />
            </svg>
            {tab.label}
          </button>
        {/each}
      </div>

      <div
        id="panel-tabpanel"
        class="panel-body"
        role="tabpanel"
        aria-labelledby={`panel-tab-${panelTab}`}
      >
        {#if panelTab === "status"}
          <StatusPanel
            {status}
            {repoId}
            onOpenDiff={(e) => void openDiff(e)}
            onAfterMutation={refresh}
            bind:selected
          />
        {:else if panelTab === "branches"}
          <BranchPanel {repoId} onMutated={refresh} />
        {:else if panelTab === "remotes"}
          <RemotePanel {repoId} onMutated={refresh} />
        {:else if panelTab === "stashes"}
          <StashPanel {repoId} onMutated={refresh} />
        {:else if panelTab === "worktrees"}
          <WorktreePanel {repoId} {root} onMutated={refresh} />
        {:else if panelTab === "reflog"}
          <ReflogPanel {repoId} />
        {:else}
          <UndoPanel {repoId} />
        {/if}
      </div>
    </div>

    <HistoryView {repoId} />
  </div>

  <div class="commit-pane">
    <CommitBar {repoId} onCommitted={refresh} />
  </div>

  {#if diffFiles.length > 0}
    <div class="diff-pane">
      <DiffViewer files={diffFiles} />
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

<AuthDialog />

<style>
  .repo-workspace {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
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
  }

  .repo-name {
    font-weight: 500;
  }

  .branch {
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-primary);
  }

  .aheadbehind {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .repo-root {
    margin-left: auto;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  .split {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  .left {
    width: 24rem;
    flex: none;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .panel-tabs {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    gap: 0.125rem;
    padding: 0.25rem 0.5rem 0;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container, var(--m3-surface));
  }

  .panel-tab {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-bottom: none;
    border-radius: var(--m3-shape-small, 8px) var(--m3-shape-small, 8px) 0 0;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.6875rem;
    padding: 0.25rem 0.5rem;
    cursor: pointer;
  }

  .tab-icon {
    width: 0.75rem;
    height: 0.75rem;
    flex: none;
  }

  .panel-tab:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .panel-tab:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .panel-tab[aria-selected="true"] {
    background: var(--m3-surface);
    color: var(--m3-primary);
    font-weight: 600;
    /* Visually merge with the panel body below. */
    padding-bottom: calc(0.25rem + 1px);
    margin-bottom: -1px;
  }

  .panel-body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .panel-body > :global(aside),
  .panel-body > :global(section) {
    flex: 1;
    min-height: 0;
  }

  .split > :global(section) {
    flex: 1;
    min-width: 0;
  }

  .commit-pane {
    flex: none;
  }

  .diff-pane {
    height: 40%;
    flex: none;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
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
