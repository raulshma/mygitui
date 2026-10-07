<script lang="ts">
  /**
   * Per-tab repo workspace (M2): header facts + a tabbed left panel stack
   * (Status | Branches | Remotes — full layout presets replace this in M4)
   * beside the history view, with the CommitBar fixed at the bottom (above
   * the diff pane). The AuthDialog is mounted once here; op/auth event
   * subscriptions and the per-repo auto-fetch interval start on mount.
   * Selected-file diffs still open in the DiffViewer below everything.
   */
  import type { RepoStatus, StatusEntry } from "$lib/ipc/types";
  import { streamDiff } from "$lib/ipc/client";
  import type { FileDiff } from "$lib/ipc/types";
  import StatusPanel from "$lib/components/panels/StatusPanel.svelte";
  import BranchPanel from "$lib/components/panels/BranchPanel.svelte";
  import RemotePanel from "$lib/components/panels/RemotePanel.svelte";
  import CommitBar from "$lib/components/panels/CommitBar.svelte";
  import HistoryView from "$lib/components/panels/HistoryView.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import AuthDialog from "$lib/components/AuthDialog.svelte";
  import { refreshStatus } from "$lib/stores/tabs.svelte";
  import { startAuthEvents, startOpsEvents } from "$lib/stores/ops.svelte";
  import { autofetch } from "$lib/stores/autofetch.svelte";

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

  type PanelTab = "status" | "branches" | "remotes";
  const PANEL_TABS: Array<{ id: PanelTab; label: string }> = [
    { id: "status", label: "Status" },
    { id: "branches", label: "Branches" },
    { id: "remotes", label: "Remotes" },
  ];

  let panelTab = $state<PanelTab>("status");
  let selected: string[] = $state([]);
  let diffFiles: FileDiff[] = $state([]);

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
            tabindex={panelTab === tab.id ? 0 : -1}
            onclick={() => (panelTab = tab.id)}
          >
            {tab.label}
          </button>
        {/each}
      </div>

      <div class="panel-body">
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
        {:else}
          <RemotePanel {repoId} onMutated={refresh} />
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
    gap: 0.125rem;
    padding: 0.25rem 0.5rem 0;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container, var(--m3-surface));
  }

  .panel-tab {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-bottom: none;
    border-radius: var(--m3-shape-small, 8px) var(--m3-shape-small, 8px) 0 0;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.75rem;
    padding: 0.25rem 0.875rem;
    cursor: pointer;
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
</style>
