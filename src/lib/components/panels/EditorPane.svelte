<script lang="ts">
  /**
   * EditorPane — the workspace's center column (VSCode-style editor area).
   *
   * Renders exactly one of:
   * - a custom action run (`ActionRunView`: command, live output, popout),
   * - a GitHub Actions run (`WorkflowRunView`: status, jobs, live logs),
   * - a commit's detail (`CommitDetail`: metadata, changed files, diff),
   * - compare results (`CommitDetail` in compare mode — no commit info),
   * - a working-copy file's diff (`DiffViewer`, with hunk staging when the
   *   shown file has no staged changes),
   * - the file workspace (`FileWorkspace`: open workdir files as tabs —
   *   viewer/editor via CodeMirror, image/binary previews),
   * - an empty invitation to pick something in the sidebars.
   *
   * Selection state lives in `RepoView` (the layout panel is a normal tree
   * leaf, so this component must be stateless across re-parenting): the
   * sidebar history list reports the commit, the changes list reports the
   * file, and both funnel through here.
   */
  import type { CommitInfo, FileDiff, RepoStatus, StatusEntry } from "$lib/ipc/types";
  import type { CompareResult } from "$lib/components/panels/compare";
  import type { HistoryCommitActions } from "$lib/components/panels/historySidebar";
  import type { OpenFileInfo } from "$lib/components/files/FileWorkspace.svelte";
  import CommitDetail from "$lib/components/panels/CommitDetail.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { IconButton, Spinner } from "$lib/components/m3";
  import iconTree from "@ktibow/iconset-material-symbols/account-tree";
  import iconPrev from "@ktibow/iconset-material-symbols/arrow-upward";
  import iconNext from "@ktibow/iconset-material-symbols/arrow-downward";

  let {
    repoId,
    root = "",
    repoName = "",
    commit = null,
    compare = null,
    entry = null,
    status = null,
    diffFiles = [],
    diffLoading = false,
    diffHunkable = false,
    historyActions = null,
    workflowRunId = null,
    actionRunId = null,
    entryIndex = null,
    entryCount = 0,
    files = null,
    onClose = undefined,
    onCloseCompare = undefined,
    onCloseWorkflowRun = undefined,
    onCloseActionRun = undefined,
    onSelectParent = undefined,
    onPopoutCommit = undefined,
    onPopoutDiff = undefined,
    onPopoutWorkflowRun = undefined,
    onPopoutActionRun = undefined,
    onMutated = undefined,
    onPrevFile = undefined,
    onNextFile = undefined,
    onStageEntry = undefined,
    onDiscardEntry = undefined,
    onEditEntry = undefined,
    onActivateFile = undefined,
    onCloseFile = undefined,
    onOpenDiffFile = undefined,
    onOpenFirstChange = undefined,
    onBrowseHistory = undefined,
  }: {
    repoId: string;
    /** Worktree root (bookmark persistence); empty in popouts. */
    root?: string;
    /** Submodule or repository display name (shown in diff header). */
    repoName?: string;
    /** Selected commit (history sidebar). */
    commit?: CommitInfo | null;
    /** CompareBar result (history sidebar). */
    compare?: CompareResult | null;
    /** Working-copy change selected in the status sidebar. */
    entry?: StatusEntry | null;
    /** Latest repo status (welcome summary when nothing is open). */
    status?: RepoStatus | null;
    /** Streamed diff for `entry`. */
    diffFiles?: FileDiff[];
    diffLoading?: boolean;
    /** Hunk actions are only safe when the file has no staged changes. */
    diffHunkable?: boolean;
    /** Multi-selection action toolbar state from the history sidebar. */
    historyActions?: HistoryCommitActions | null;
    /** Custom action run taking over the editor. */
    actionRunId?: string | null;
    /** GitHub Actions run taking over the editor. */
    workflowRunId?: number | null;
    /** 0-based position of `entry` in the changed-file list (null = n/a). */
    entryIndex?: number | null;
    /** Total changed files (drives prev/next navigation availability). */
    entryCount?: number;
    /**
     * File workspace state (open workdir files): renders the tabbed
     * viewer/editor instead of the diff/commit views when set.
     */
    files?: {
      repoId: string;
      root: string;
      openFiles: OpenFileInfo[];
      active: string | null;
    } | null;
    /** Clears the editor (closes the current commit/diff). */
    onClose?: () => void;
    onCloseCompare?: () => void;
    /** Closes the GitHub Actions run view. */
    onCloseWorkflowRun?: () => void;
    /** Closes the custom action run view. */
    onCloseActionRun?: () => void;
    onSelectParent?: (sha: string) => void;
    onPopoutCommit?: () => void;
    onPopoutDiff?: () => void;
    /** Opens the run view in its own window. */
    onPopoutWorkflowRun?: () => void;
    /** Opens the action run log in its own window. */
    onPopoutActionRun?: () => void;
    /** Called after a hunk stage/unstage/discard mutated the repo. */
    onMutated?: () => void;
    /** Steps to the previous changed file (wraps around). */
    onPrevFile?: () => void;
    /** Steps to the next changed file (wraps around). */
    onNextFile?: () => void;
    /** Stages (unstage=false) or unstages the open file. */
    onStageEntry?: (unstage: boolean) => void;
    /** Asks the owner to confirm discarding the open file's changes. */
    onDiscardEntry?: () => void;
    /** Opens the shown file in the file workspace (editor mode). */
    onEditEntry?: () => void;
    /** Activates (focuses) an open file tab. */
    onActivateFile?: (path: string) => void;
    /** Closes an open file tab (after any save guard). */
    onCloseFile?: (path: string) => void;
    /** Opens the working-copy diff for a file from the workspace. */
    onOpenDiffFile?: (path: string) => void;
    /** Welcome state: opens the first changed file. */
    onOpenFirstChange?: () => void;
    /** Welcome state: reveals the history sidebar. */
    onBrowseHistory?: () => void;
  } = $props();

  /** Welcome summary: unique changed paths + upstream position. */
  const changeCount = $derived(
    status ? new Set(status.entries.map((e) => e.path)).size : 0,
  );
  const branchLabel = $derived(
    status ? (status.branch ?? (status.detached ? "detached HEAD" : "unknown")) : "",
  );
</script>

<div class="editor" aria-label="Editor">
  {#if actionRunId !== null}
    {#await import("$lib/components/actions/ActionRunView.svelte")}
      <p class="estate estate-loading" role="status">
        <Spinner size={14} />
        Loading action run…
      </p>
    {:then ActionRunViewModule}
      <ActionRunViewModule.default
        {repoId}
        runId={actionRunId}
        onClose={onCloseActionRun}
        onPopout={onPopoutActionRun}
      />
    {/await}
  {:else if workflowRunId !== null}
    {#await import("$lib/components/forge/WorkflowRunView.svelte")}
      <p class="estate estate-loading" role="status">
        <Spinner size={14} />
        Loading run…
      </p>
    {:then WorkflowRunViewModule}
      <WorkflowRunViewModule.default
        {repoId}
        runId={workflowRunId}
        onClose={onCloseWorkflowRun}
        onPopout={onPopoutWorkflowRun}
      />
    {/await}
  {:else if commit}
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
  {:else if files && files.openFiles.length > 0}
    {#await import("$lib/components/files/FileWorkspace.svelte")}
      <p class="estate estate-loading" role="status">
        <Spinner size={14} />
        Loading workspace…
      </p>
    {:then FileWorkspaceModule}
      <FileWorkspaceModule.default
        repoId={files.repoId}
        root={files.root}
        openFiles={files.openFiles}
        active={files.active}
        onActivate={(path) => onActivateFile?.(path)}
        onClose={(path) => onCloseFile?.(path)}
        onMutated={onMutated}
        onOpenDiff={(path) => onOpenDiffFile?.(path)}
      />
    {:catch err}
      <p class="estate" role="alert">Failed to load the file workspace: {String(err)}</p>
    {/await}
  {:else if entry}
    <div class="file-editor">
      <header class="ehead">
        <span class="epath" title={entry.path}>
          {#if entry.old_path}{entry.old_path} → {/if}{entry.path}
        </span>
        {#if repoName}
          <span class="echip chip-repo" title={`Repository / Submodule: ${repoName}`}>{repoName}</span>
        {/if}
        <span
          class="echip"
          class:chip-staged={entry.index !== "unmodified"}
          title={entry.index !== "unmodified"
            ? "This file has staged changes"
            : "Changes are not staged"}
        >
          {entry.index !== "unmodified" ? "staged" : "unstaged"}
        </span>
        {#if entryCount > 1 && onPrevFile && onNextFile}
          <span class="enav">
            <IconButton
              icon={iconPrev}
              label="Previous changed file"
              title="Previous changed file"
              size={14}
              disabled={entryIndex === null}
              onclick={onPrevFile}
            />
            <span
              class="epos"
              aria-label={`File ${entryIndex === null ? "?" : entryIndex + 1} of ${entryCount}`}
            >
              {(entryIndex ?? 0) + 1} / {entryCount}
            </span>
            <IconButton
              icon={iconNext}
              label="Next changed file"
              title="Next changed file"
              size={14}
              disabled={entryIndex === null}
              onclick={onNextFile}
            />
          </span>
        {/if}
        {#if onStageEntry}
          {#if entry.index === "unmodified"}
            <button
              class="ebtn"
              type="button"
              title="Stage this file (move its changes into the commit)"
              onclick={() => onStageEntry?.(false)}
            >
              Stage
            </button>
          {:else}
            <button
              class="ebtn"
              type="button"
              title="Unstage this file (keep the worktree changes)"
              onclick={() => onStageEntry?.(true)}
            >
              Unstage
            </button>
          {/if}
        {/if}
        {#if onDiscardEntry && entry.worktree !== "unmodified"}
          <button
            class="ebtn ebtn-danger"
            type="button"
            title="Discard this file's changes (a safety checkpoint is created first)"
            onclick={onDiscardEntry}
          >
            Discard
          </button>
        {/if}
        {#if onEditEntry}
          <button
            class="ebtn"
            type="button"
            title="Edit this file in the file workspace"
            onclick={onEditEntry}
          >
            Edit
          </button>
        {/if}
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
          <p class="estate estate-loading" role="status">
            <Spinner size={14} />
            Loading diff…
          </p>
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
      {#if status}
        <div class="wsummary" aria-label="Repository summary">
          <span class="wchip wbranch">{branchLabel}</span>
          <span class="wchip">
            {changeCount === 0
              ? "Clean working tree"
              : `${changeCount} changed ${changeCount === 1 ? "file" : "files"}`}
          </span>
          {#if status.ahead > 0}
            <span
              class="wchip wahead"
              title={`${status.ahead} ${status.ahead === 1 ? "commit" : "commits"} to push`}
            >
              ↑ {status.ahead} to push
            </span>
          {/if}
          {#if status.behind > 0}
            <span
              class="wchip wbehind"
              title={`${status.behind} ${status.behind === 1 ? "commit" : "commits"} to pull`}
            >
              ↓ {status.behind} to pull
            </span>
          {/if}
        </div>
      {:else}
        <p class="wloading" role="status">
          <Spinner size={13} />
          Reading repository…
        </p>
      {/if}
      <p class="wtext">
        Select a change in the source-control sidebar to see its diff, or pick
        a commit in history to inspect it.
      </p>
      {#if (changeCount > 0 && onOpenFirstChange) || onBrowseHistory}
        <div class="wactions">
          {#if changeCount > 0 && onOpenFirstChange}
            <button class="wact wact-primary" type="button" onclick={onOpenFirstChange}>
              Open first change
            </button>
          {/if}
          {#if onBrowseHistory}
            <button class="wact" type="button" onclick={onBrowseHistory}>
              Browse history
            </button>
          {/if}
        </div>
      {/if}
      <ul class="whints" aria-label="Keyboard shortcuts">
        <li><kbd class="kbd">Ctrl</kbd>+<kbd class="kbd">Shift</kbd>+<kbd class="kbd">P</kbd><span>Command palette</span></li>
        <li><kbd class="kbd">F5</kbd><span>Refresh repository</span></li>
        <li><kbd class="kbd">Ctrl</kbd>+<kbd class="kbd">1</kbd><span>Focus source control</span></li>
      </ul>
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

  /* Staged state in the header (was only visible via row color). */
  .echip.chip-staged {
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-primary));
  }

  /* Prev/next changed-file navigation. */
  .enav {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 0.05rem;
  }

  .epos {
    min-width: 3.2rem;
    text-align: center;
    font-size: 0.65rem;
    font-variant-numeric: tabular-nums;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
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

  .ebtn-danger {
    color: var(--m3-error, var(--m3-on-surface-variant));
  }

  .ebtn-danger:hover {
    color: var(--m3-error);
    background: var(--m3-error-container, var(--m3-surface-container-high));
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

  p.estate-loading {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
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

  .wsummary {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: center;
    gap: 0.35rem;
    margin-top: 0.15rem;
  }

  .wchip {
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    padding: 0.1rem 0.5rem;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-full, 9999px);
    font-size: 0.6875rem;
    line-height: 1.4;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .wbranch {
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    color: var(--m3-primary);
    border-color: color-mix(in srgb, var(--m3-primary) 35%, transparent);
  }

  .wahead {
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-primary));
    border-color: transparent;
  }

  .wbehind {
    background: var(--m3-tertiary-container, var(--m3-surface-container-high));
    color: var(--m3-on-tertiary-container, var(--m3-tertiary));
    border-color: transparent;
  }

  .wloading {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    margin: 0.15rem 0 0;
    font-size: 0.6875rem;
  }

  /* Empty-state actions (UI review P2 #8): give the welcome pane something
     to do instead of only pointing at the sidebars. */
  .wactions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }

  .wact {
    padding: 0.3rem 0.9rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.8125rem;
    cursor: pointer;
  }

  .wact:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .wact-primary {
    background: var(--m3-primary);
    border-color: transparent;
    color: var(--m3-on-primary);
  }

  .wact-primary:hover {
    background: var(--m3-primary);
    filter: brightness(1.06);
  }

  .wact:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .whints {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 0.4rem 1.1rem;
    margin: 0.9rem 0 0;
    padding: 0.7rem 0 0;
    border-top: 1px solid var(--app-border, var(--m3-outline-variant));
    list-style: none;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant);
  }

  .whints li {
    display: inline-flex;
    align-items: center;
    gap: 0.15rem;
  }

  .whints li span {
    margin-left: 0.4rem;
  }
</style>
