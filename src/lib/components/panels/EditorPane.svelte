<script lang="ts">
  /**
   * EditorPane — the workspace's center column (VSCode-style editor area).
   *
   * Renders exactly one of:
   * - a commit's detail (`CommitDetail`: metadata, changed files, diff),
   * - compare results (`CommitDetail` in compare mode — no commit info),
   * - a working-copy file's diff (`DiffViewer`, with hunk staging when the
   *   shown file has no staged changes),
   * - an empty invitation to pick something in the sidebars.
   *
   * Selection state lives in `RepoView` (the layout panel is a normal tree
   * leaf, so this component must be stateless across re-parenting): the
   * sidebar history list reports the commit, the changes list reports the
   * file, and both funnel through here.
   */
  import type { CommitInfo, FileDiff, StatusEntry } from "$lib/ipc/types";
  import type { CompareResult } from "$lib/components/panels/compare";
  import type { HistoryCommitActions } from "$lib/components/panels/historySidebar";
  import CommitDetail from "$lib/components/panels/CommitDetail.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import iconTree from "@ktibow/iconset-material-symbols/account-tree";

  let {
    repoId,
    root = "",
    commit = null,
    compare = null,
    entry = null,
    diffFiles = [],
    diffLoading = false,
    diffHunkable = false,
    historyActions = null,
    onClose = undefined,
    onCloseCompare = undefined,
    onSelectParent = undefined,
    onPopoutCommit = undefined,
    onPopoutDiff = undefined,
    onMutated = undefined,
  }: {
    repoId: string;
    /** Worktree root (bookmark persistence); empty in popouts. */
    root?: string;
    /** Selected commit (history sidebar). */
    commit?: CommitInfo | null;
    /** CompareBar result (history sidebar). */
    compare?: CompareResult | null;
    /** Working-copy change selected in the status sidebar. */
    entry?: StatusEntry | null;
    /** Streamed diff for `entry`. */
    diffFiles?: FileDiff[];
    diffLoading?: boolean;
    /** Hunk actions are only safe when the file has no staged changes. */
    diffHunkable?: boolean;
    /** Multi-selection action toolbar state from the history sidebar. */
    historyActions?: HistoryCommitActions | null;
    /** Clears the editor (closes the current commit/diff). */
    onClose?: () => void;
    onCloseCompare?: () => void;
    onSelectParent?: (sha: string) => void;
    onPopoutCommit?: () => void;
    onPopoutDiff?: () => void;
    /** Called after a hunk stage/unstage/discard mutated the repo. */
    onMutated?: () => void;
  } = $props();
</script>

<div class="editor" aria-label="Editor">
  {#if commit}
    <CommitDetail
      {repoId}
      {root}
      info={commit}
      actionCount={historyActions?.count ?? 1}
      actionBusy={historyActions?.busy ?? false}
      onClose={onClose}
      onSelectParent={onSelectParent}
      onPopout={onPopoutCommit}
      onCherryPick={historyActions?.cherryPick}
      onRevert={historyActions?.revert}
      onRebaseFromHere={historyActions?.rebaseFromHere}
    />
  {:else if compare}
    <CommitDetail
      {repoId}
      {root}
      info={null}
      {compare}
      onClose={onClose}
      onCloseCompare={onCloseCompare}
    />
  {:else if entry}
    <div class="file-editor">
      <header class="ehead">
        <span class="epath" title={entry.path}>
          {#if entry.old_path}{entry.old_path} → {/if}{entry.path}
        </span>
        <span class="echip">working copy</span>
        {#if onPopoutDiff}
          <button
            class="ebtn"
            type="button"
            title="Open the diff panel in its own window"
            onclick={onPopoutDiff}
          >
            Pop out
          </button>
        {/if}
        {#if onClose}
          <button
            class="ebtn"
            type="button"
            aria-label="Close diff"
            title="Close diff"
            onclick={onClose}
          >
            ×
          </button>
        {/if}
      </header>
      <div class="ebody">
        {#if diffLoading}
          <p class="estate" role="status">Loading diff…</p>
        {:else if diffFiles.length === 0}
          <p class="estate">No diff against HEAD for this file.</p>
        {:else}
          <DiffViewer
            files={diffFiles}
            {repoId}
            hunkStaging={diffHunkable ? "stage" : undefined}
            hunkDiscard={diffHunkable}
            {onMutated}
          />
        {/if}
      </div>
    </div>
  {:else}
    <div class="welcome">
      <span class="wicon" aria-hidden="true"><Icon icon={iconTree} size={22} /></span>
      <p class="wtitle">Nothing open</p>
      <p class="wtext">
        Select a change in the source-control sidebar to see its diff, or pick
        a commit in history to inspect it.
      </p>
    </div>
  {/if}
</div>

<style>
  .editor {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    min-width: 0;
    background: var(--m3-surface);
  }

  /* CommitDetail / file editor fill the column. */
  .editor > :global(*) {
    flex: 1;
    min-height: 0;
  }

  .file-editor {
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
  }

  .ehead {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-height: 1.9rem;
    padding: 0.15rem 0.6rem;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--app-border, var(--m3-outline-variant));
    font-size: 0.8125rem;
  }

  .epath {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.78rem;
    color: var(--m3-on-surface);
  }

  .echip {
    flex: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--app-active, var(--m3-surface-container-high));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.625rem;
    line-height: 1.5;
    padding: 0 0.45rem;
    white-space: nowrap;
  }

  .ebody {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .ebtn {
    flex: none;
    padding: 0.05rem 0.4rem;
    color: var(--m3-on-surface-variant);
    background: none;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-extra-small, 4px);
    font: inherit;
    font-size: 0.6875rem;
    line-height: 1.5;
    cursor: pointer;
  }

  .ebtn:hover {
    background: var(--app-hover, var(--m3-surface-container-high, var(--m3-surface)));
    color: var(--m3-on-surface);
  }

  .ebtn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .estate {
    color: var(--m3-on-surface-variant);
    font-size: 0.75rem;
  }

  p.estate {
    margin: auto;
    font-size: 0.8125rem;
  }

  .welcome {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    padding: 2rem;
    text-align: center;
    color: var(--m3-on-surface-variant);
    user-select: none;
  }

  .wicon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 2.75rem;
    height: 2.75rem;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-full, 9999px);
    color: var(--m3-primary);
    background: var(--m3-surface-container-low, var(--m3-surface));
  }

  .wtitle {
    margin: 0;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--m3-on-surface);
  }

  .wtext {
    margin: 0;
    max-width: 22rem;
    font-size: 0.75rem;
    line-height: 1.45;
  }
</style>
