<script lang="ts">
  /**
   * Per-tab repo workspace (M1 simple split; layout presets replace this in M4):
   * header facts + status panel (left) + history view (right).
   * Selected-file diff opens in the DiffViewer below the split.
   */
  import type { RepoStatus, StatusEntry } from "$lib/ipc/types";
  import { streamDiff } from "$lib/ipc/client";
  import type { FileDiff } from "$lib/ipc/types";
  import StatusPanel from "$lib/components/panels/StatusPanel.svelte";
  import HistoryView from "$lib/components/panels/HistoryView.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";

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

  function branchLabel(s: RepoStatus): string {
    if (s.branch) return s.branch;
    if (s.detached && s.head) return `detached @ ${s.head.slice(0, 7)}`;
    return s.detached ? "detached" : "unknown";
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
    <StatusPanel {status} onOpenDiff={(e) => void openDiff(e)} bind:selected />
    <HistoryView {repoId} />
  </div>

  {#if diffFiles.length > 0}
    <div class="diff-pane">
      <DiffViewer files={diffFiles} />
    </div>
  {/if}
</div>

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

  .split > :global(aside) {
    width: 22rem;
    flex: none;
    overflow-y: auto;
    border-right: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .split > :global(section) {
    flex: 1;
    min-width: 0;
  }

  .diff-pane {
    height: 40%;
    flex: none;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    overflow: hidden;
  }
</style>
