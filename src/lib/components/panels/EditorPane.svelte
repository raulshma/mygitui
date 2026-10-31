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
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Kbd } from "$lib/components/ui/kbd/index.js";
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

<div
  class="editor flex h-full min-h-0 min-w-0 flex-col bg-card [&>*]:min-h-0 [&>*]:flex-1"
  aria-label="Editor"
>
  {#if actionRunId !== null}
    {#await import("$lib/components/actions/ActionRunView.svelte")}
      <p
        class="estate estate-loading m-auto inline-flex items-center gap-[0.4rem] text-[0.8125rem] text-muted-foreground"
        role="status"
      >
        <Spinner class="size-[14px]" role="presentation" aria-hidden="true" />
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
      <p
        class="estate estate-loading m-auto inline-flex items-center gap-[0.4rem] text-[0.8125rem] text-muted-foreground"
        role="status"
      >
        <Spinner class="size-[14px]" role="presentation" aria-hidden="true" />
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
      <p
        class="estate estate-loading m-auto inline-flex items-center gap-[0.4rem] text-[0.8125rem] text-muted-foreground"
        role="status"
      >
        <Spinner class="size-[14px]" role="presentation" aria-hidden="true" />
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
      <p class="estate m-auto text-[0.8125rem] text-muted-foreground" role="alert">Failed to load the file workspace: {String(err)}</p>
    {/await}
  {:else if entry}
    <div class="file-editor flex min-h-0 min-w-0 flex-col">
      <header
        class="ehead flex min-h-[1.9rem] flex-none items-center gap-[0.4rem] border-b border-border bg-popover px-[0.6rem] py-[0.15rem] text-[0.8125rem]"
      >
        <span
          class="epath min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[0.78rem] text-foreground"
          title={entry.path}
        >
          {#if entry.old_path}{entry.old_path} → {/if}{entry.path}
        </span>
        {#if repoName}
          <span
            class="echip chip-repo flex-none whitespace-nowrap rounded-full bg-accent px-[0.45rem] text-[0.625rem] leading-[1.5] text-muted-foreground"
            title={`Repository / Submodule: ${repoName}`}
          >{repoName}</span>
        {/if}
        <span
          class="echip flex-none whitespace-nowrap rounded-full bg-accent px-[0.45rem] text-[0.625rem] leading-[1.5] {entry.index !==
          "unmodified"
            ? "text-accent-foreground"
            : "text-muted-foreground"}"
          title={entry.index !== "unmodified"
            ? "This file has staged changes"
            : "Changes are not staged"}
        >
          {entry.index !== "unmodified" ? "staged" : "unstaged"}
        </span>
        {#if entryCount > 1 && onPrevFile && onNextFile}
          <span class="enav inline-flex flex-none items-center gap-[0.05rem]">
            <Button
              class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-[14px]"
              variant="ghost"
              size="icon-sm"
              type="button"
              aria-label="Previous changed file"
              title="Previous changed file"
              disabled={entryIndex === null}
              onclick={onPrevFile}
            >
              <Icon icon={iconPrev} size={14} />
            </Button>
            <span
              class="epos min-w-[3.2rem] whitespace-nowrap text-center text-[0.65rem] tabular-nums text-muted-foreground"
              aria-label={`File ${entryIndex === null ? "?" : entryIndex + 1} of ${entryCount}`}
            >
              {(entryIndex ?? 0) + 1} / {entryCount}
            </span>
            <Button
              class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-[14px]"
              variant="ghost"
              size="icon-sm"
              type="button"
              aria-label="Next changed file"
              title="Next changed file"
              disabled={entryIndex === null}
              onclick={onNextFile}
            >
              <Icon icon={iconNext} size={14} />
            </Button>
          </span>
        {/if}
        {#if onStageEntry}
          {#if entry.index === "unmodified"}
            <Button
              class="h-[1.35rem] flex-none rounded-sm px-[0.4rem] text-[0.6875rem] leading-[1.5] font-normal"
              variant="outline"
              type="button"
              title="Stage this file (move its changes into the commit)"
              onclick={() => onStageEntry?.(false)}
            >
              Stage
            </Button>
          {:else}
            <Button
              class="h-[1.35rem] flex-none rounded-sm px-[0.4rem] text-[0.6875rem] leading-[1.5] font-normal"
              variant="outline"
              type="button"
              title="Unstage this file (keep the worktree changes)"
              onclick={() => onStageEntry?.(true)}
            >
              Unstage
            </Button>
          {/if}
        {/if}
        {#if onDiscardEntry && entry.worktree !== "unmodified"}
          <Button
            class="h-[1.35rem] flex-none rounded-sm px-[0.4rem] text-[0.6875rem] leading-[1.5] font-normal"
            variant="destructive"
            type="button"
            title="Discard this file's changes (a safety checkpoint is created first)"
            onclick={onDiscardEntry}
          >
            Discard
          </Button>
        {/if}
        {#if onEditEntry}
          <Button
            class="h-[1.35rem] flex-none rounded-sm px-[0.4rem] text-[0.6875rem] leading-[1.5] font-normal"
            variant="outline"
            type="button"
            title="Edit this file in the file workspace"
            onclick={onEditEntry}
          >
            Edit
          </Button>
        {/if}
        {#if onPopoutDiff}
          <Button
            class="h-[1.35rem] flex-none rounded-sm px-[0.4rem] text-[0.6875rem] leading-[1.5] font-normal"
            variant="outline"
            type="button"
            title="Open the diff panel in its own window"
            onclick={onPopoutDiff}
          >
            Pop out
          </Button>
        {/if}
        {#if onClose}
          <Button
            class="h-[1.35rem] flex-none rounded-sm px-[0.4rem] text-[0.6875rem] leading-[1.5] font-normal"
            variant="outline"
            type="button"
            aria-label="Close diff"
            title="Close diff"
            onclick={onClose}
          >
            ×
          </Button>
        {/if}
      </header>
      <div class="ebody flex min-h-0 flex-1 flex-col overflow-hidden">
        {#if diffLoading}
          <p
            class="estate estate-loading m-auto inline-flex items-center gap-[0.4rem] text-[0.8125rem] text-muted-foreground"
            role="status"
          >
            <Spinner class="size-[14px]" role="presentation" aria-hidden="true" />
            Loading diff…
          </p>
        {:else if diffFiles.length === 0}
          <p class="estate m-auto text-[0.8125rem] text-muted-foreground">No diff against HEAD for this file.</p>
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
    <div
      class="welcome flex min-h-0 flex-1 select-none flex-col items-center justify-center gap-[0.4rem] p-8 text-center text-muted-foreground"
    >
      <span
        class="wicon inline-flex h-[2.75rem] w-[2.75rem] items-center justify-center rounded-full border border-border bg-card text-primary"
        aria-hidden="true"
      ><Icon icon={iconTree} size={22} /></span>
      <p class="wtitle m-0 text-sm font-medium text-foreground">Nothing open</p>
      {#if status}
        <div
          class="wsummary mt-[0.15rem] flex flex-wrap items-center justify-center gap-[0.35rem]"
          aria-label="Repository summary"
        >
          <span
            class="wchip wbranch inline-flex items-center gap-[0.2rem] whitespace-nowrap rounded-full border border-primary/35 px-[0.5rem] py-[0.1rem] font-mono text-[0.6875rem] leading-[1.4] tabular-nums text-primary"
          >{branchLabel}</span>
          <span
            class="wchip inline-flex items-center gap-[0.2rem] whitespace-nowrap rounded-full border border-border px-[0.5rem] py-[0.1rem] text-[0.6875rem] leading-[1.4] tabular-nums"
          >
            {changeCount === 0
              ? "Clean working tree"
              : `${changeCount} changed ${changeCount === 1 ? "file" : "files"}`}
          </span>
          {#if status.ahead > 0}
            <span
              class="wchip wahead inline-flex items-center gap-[0.2rem] whitespace-nowrap rounded-full border border-transparent bg-accent px-[0.5rem] py-[0.1rem] text-[0.6875rem] leading-[1.4] tabular-nums text-accent-foreground"
              title={`${status.ahead} ${status.ahead === 1 ? "commit" : "commits"} to push`}
            >
              ↑ {status.ahead} to push
            </span>
          {/if}
          {#if status.behind > 0}
            <span
              class="wchip wbehind inline-flex items-center gap-[0.2rem] whitespace-nowrap rounded-full border border-transparent bg-muted px-[0.5rem] py-[0.1rem] text-[0.6875rem] leading-[1.4] tabular-nums text-muted-foreground"
              title={`${status.behind} ${status.behind === 1 ? "commit" : "commits"} to pull`}
            >
              ↓ {status.behind} to pull
            </span>
          {/if}
        </div>
      {:else}
        <p
          class="wloading mt-[0.15rem] inline-flex items-center gap-[0.4rem] text-[0.6875rem]"
          role="status"
        >
          <Spinner class="size-[13px]" role="presentation" aria-hidden="true" />
          Reading repository…
        </p>
      {/if}
      <p class="wtext m-0 max-w-[22rem] text-xs leading-[1.45]">
        Select a change in the source-control sidebar to see its diff, or pick
        a commit in history to inspect it.
      </p>
      {#if (changeCount > 0 && onOpenFirstChange) || onBrowseHistory}
        <div class="wactions mt-2 flex flex-wrap justify-center gap-2">
          {#if changeCount > 0 && onOpenFirstChange}
            <Button
              class="h-auto rounded-full px-[0.9rem] py-[0.3rem] text-[0.8125rem]"
              type="button"
              onclick={onOpenFirstChange}
            >
              Open first change
            </Button>
          {/if}
          {#if onBrowseHistory}
            <Button
              class="h-auto rounded-full px-[0.9rem] py-[0.3rem] text-[0.8125rem]"
              variant="outline"
              type="button"
              onclick={onBrowseHistory}
            >
              Browse history
            </Button>
          {/if}
        </div>
      {/if}
      <ul
        class="whints mt-[0.9rem] flex list-none flex-wrap justify-center gap-x-[1.1rem] gap-y-[0.4rem] border-t border-border pt-[0.7rem] text-[0.6875rem] text-muted-foreground"
        aria-label="Keyboard shortcuts"
      >
        <li class="inline-flex items-center gap-[0.15rem]"><Kbd>Ctrl</Kbd>+<Kbd>Shift</Kbd>+<Kbd>P</Kbd><span class="ml-[0.4rem]">Command palette</span></li>
        <li class="inline-flex items-center gap-[0.15rem]"><Kbd>F5</Kbd><span class="ml-[0.4rem]">Refresh repository</span></li>
        <li class="inline-flex items-center gap-[0.15rem]"><Kbd>Ctrl</Kbd>+<Kbd>1</Kbd><span class="ml-[0.4rem]">Focus source control</span></li>
      </ul>
    </div>
  {/if}
</div>
