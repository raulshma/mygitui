<script lang="ts">
  /**
   * EditorPane — the workspace's center column (VSCode-style editor area).
   *
   * Renders exactly one of:
   * - a custom action run (`ActionRunView`: command, live output, popout),
   * - a GitHub Actions run (`WorkflowRunView`: status, jobs, live logs),
   * - a commit's detail (`CommitDetail`: metadata, changed files, diff),
   * - compare results (`CommitDetail` in compare mode — no commit info),
   * - a working-copy file's diff (`DiffViewer`, with scope-correct hunk
   *   actions: unstaged hunks stage/discard, staged hunks unstage),
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
  import { diffHunkActions, type DiffScope } from "$lib/components/panels/statusModel";
  import CommitDetail from "$lib/components/panels/CommitDetail.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import EmptyState from "../layout/EmptyState.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Kbd } from "$lib/components/ui/kbd/index.js";
  import { appIcon } from "$lib/components/icons/library.svelte";

  const iconTree = $derived(appIcon("account-tree")!);
  const iconPrev = $derived(appIcon("arrow-upward")!);
  const iconNext = $derived(appIcon("arrow-downward")!);

  // The lazy views must keep the *same* promise across updates: `{#await
  // import(...)}` would hand Svelte a fresh promise on every parent update,
  // re-entering the pending branch and remounting the view (losing dirty
  // editor buffers, log scroll, etc.).
  let fileWorkspaceImport: Promise<typeof import("$lib/components/files/FileWorkspace.svelte")> | null =
    null;
  let actionRunImport: Promise<typeof import("$lib/components/actions/ActionRunView.svelte")> | null =
    null;
  let workflowRunImport: Promise<typeof import("$lib/components/forge/WorkflowRunView.svelte")> | null =
    null;
  function loadFileWorkspace() {
    return (fileWorkspaceImport ??= import("$lib/components/files/FileWorkspace.svelte"));
  }
  function loadActionRun() {
    return (actionRunImport ??= import("$lib/components/actions/ActionRunView.svelte"));
  }
  function loadWorkflowRun() {
    return (workflowRunImport ??= import("$lib/components/forge/WorkflowRunView.svelte"));
  }

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
    diffScope = "unstaged",
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
    /**
     * Which working-copy comparison the streamed diff shows (VSCode
     * semantics): `staged` = HEAD→index, `unstaged` = index→worktree.
     * Decides the hunk actions and the empty-state wording.
     */
    diffScope?: DiffScope;
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
      /** Whether the workspace owns the center pane right now. */
      visible: boolean;
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

  /**
   * Hunk actions mirror the shown scope (see {@link diffHunkActions}): the
   * staged view's hunks are HEAD→index, so they *unstage*; the unstaged
   * view's hunks are index→worktree, so they stage — and only there does
   * hunk discard exist (the backend's discard engine reads index→workdir
   * hunks). Replaces the old `diffHunkable` gate, which approximated the
   * unstaged view with head→worktree and only coincided when nothing was
   * staged.
   */
  const hunkActions = $derived(
    entry
      ? diffHunkActions(entry, diffScope)
      : { staging: undefined, discard: false },
  );
</script>

<div
  class="editor flex h-full min-h-0 min-w-0 flex-col bg-card [&>*]:min-h-0 [&>*]:flex-1"
  aria-label="Editor"
>
  {#if files}
    <!-- Kept mounted while another center view is shown: unmounting the
         workspace would discard dirty CodeMirror buffers and undo history. -->
    <div class="min-h-0 min-w-0 flex-1" class:hidden={!files.visible}>
      {#await loadFileWorkspace()}
        <p
          class="estate estate-loading m-auto inline-flex items-center gap-[0.4rem] text-ui-base text-muted-foreground"
          role="status"
        >
          <Spinner class="size-4" role="presentation" aria-hidden="true" />
          Loading workspace…
        </p>
      {:then FileWorkspaceModule}
        <FileWorkspaceModule.default
          repoId={files.repoId}
          root={files.root}
          openFiles={files.openFiles}
          active={files.active}
          visible={files.visible}
          onActivate={(path) => onActivateFile?.(path)}
          onClose={(path) => onCloseFile?.(path)}
          onMutated={onMutated}
          onOpenDiff={(path) => onOpenDiffFile?.(path)}
        />
      {:catch err}
        <p class="estate m-auto text-ui-base text-muted-foreground" role="alert">Failed to load the file workspace: {String(err)}</p>
      {/await}
    </div>
  {/if}
  {#if !(files?.visible)}
  {#if actionRunId !== null}
    {#await loadActionRun()}
      <p
        class="estate estate-loading m-auto inline-flex items-center gap-[0.4rem] text-ui-base text-muted-foreground"
        role="status"
      >
        <Spinner class="size-4" role="presentation" aria-hidden="true" />
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
    {#await loadWorkflowRun()}
      <p
        class="estate estate-loading m-auto inline-flex items-center gap-[0.4rem] text-ui-base text-muted-foreground"
        role="status"
      >
        <Spinner class="size-4" role="presentation" aria-hidden="true" />
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
  {:else if entry}
    <div class="file-editor flex min-h-0 min-w-0 flex-col">
      <header
        class="ehead flex min-h-7 flex-none items-center gap-[0.4rem] border-b border-border bg-popover px-2.5 py-0.5 text-ui-base"
      >
        <Hint label={entry.path}>
          {#snippet child({ props })}
            <span
              class="epath min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-ui-meta text-foreground"
              {...props}
            >
              {#if entry.old_path}{entry.old_path} → {/if}{entry.path}
            </span>
          {/snippet}
        </Hint>
        {#if repoName}
          <Hint label={`Repository / Submodule: ${repoName}`}>
            {#snippet child({ props })}
              <span
                class="echip chip-repo flex-none whitespace-nowrap rounded-full bg-accent px-2 text-ui-tiny leading-[1.5] text-muted-foreground"
                {...props}
              >{repoName}</span>
            {/snippet}
          </Hint>
        {/if}
        <Hint
          label={entry.index !== "unmodified"
            ? "This file has staged changes"
            : "Changes are not staged"}
        >
          {#snippet child({ props })}
            <span
              class="echip flex-none whitespace-nowrap rounded-full bg-accent px-2 text-ui-tiny leading-[1.5] {entry.index !==
              "unmodified"
                ? "text-accent-foreground"
                : "text-muted-foreground"}"
              {...props}
            >
              {entry.index !== "unmodified" ? "staged" : "unstaged"}
            </span>
          {/snippet}
        </Hint>
        {#if entryCount > 1 && onPrevFile && onNextFile}
          <span class="enav inline-flex flex-none items-center gap-[0.05rem]">
            <Button
              class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-3.5"
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
              class="epos min-w-[3.2rem] whitespace-nowrap text-center text-ui-tiny tabular-nums text-muted-foreground"
              aria-label={`File ${entryIndex === null ? "?" : entryIndex + 1} of ${entryCount}`}
            >
              {(entryIndex ?? 0) + 1} / {entryCount}
            </span>
            <Button
              class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-3.5"
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
            <Hint label="Stage this file (move its changes into the commit)">
              {#snippet child({ props })}
                <Button
                  class="h-5 flex-none rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
                  variant="outline"
                  type="button"
                  {...props}
                  onclick={() => onStageEntry?.(false)}
                >
                  Stage
                </Button>
              {/snippet}
            </Hint>
          {:else}
            <Hint label="Unstage this file (keep the worktree changes)">
              {#snippet child({ props })}
                <Button
                  class="h-5 flex-none rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
                  variant="outline"
                  type="button"
                  {...props}
                  onclick={() => onStageEntry?.(true)}
                >
                  Unstage
                </Button>
              {/snippet}
            </Hint>
          {/if}
        {/if}
        {#if onDiscardEntry && entry.worktree !== "unmodified"}
          <Hint label="Discard this file's changes (a safety checkpoint is created first)">
            {#snippet child({ props })}
              <Button
                class="h-5 flex-none rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
                variant="destructive"
                type="button"
                {...props}
                onclick={onDiscardEntry}
              >
                Discard
              </Button>
            {/snippet}
          </Hint>
        {/if}
        {#if onEditEntry}
          <Hint label="Edit this file in the file workspace">
            {#snippet child({ props })}
              <Button
                class="h-5 flex-none rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
                variant="outline"
                type="button"
                {...props}
                onclick={onEditEntry}
              >
                Edit
              </Button>
            {/snippet}
          </Hint>
        {/if}
        {#if onPopoutDiff}
          <Hint label="Open the diff panel in its own window">
            {#snippet child({ props })}
              <Button
                class="h-5 flex-none rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
                variant="outline"
                type="button"
                {...props}
                onclick={onPopoutDiff}
              >
                Pop out
              </Button>
            {/snippet}
          </Hint>
        {/if}
        {#if onClose}
          <Hint label="Close diff">
            {#snippet child({ props })}
              <Button
                class="h-5 flex-none rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
                variant="outline"
                type="button"
                {...props}
                aria-label="Close diff"
                onclick={onClose}
              >
                ×
              </Button>
            {/snippet}
          </Hint>
        {/if}
      </header>
      <div class="ebody flex min-h-0 flex-1 flex-col overflow-hidden">
        {#if diffLoading}
          <p
            class="estate estate-loading m-auto inline-flex items-center gap-[0.4rem] text-ui-base text-muted-foreground"
            role="status"
          >
            <Spinner class="size-4" role="presentation" aria-hidden="true" />
            Loading diff…
          </p>
        {:else if diffFiles.length === 0}
          <EmptyState
            title={diffScope === "staged"
              ? "No staged changes for this file."
              : "No unstaged changes for this file."}
          />
        {:else}
          <DiffViewer
            files={diffFiles}
            {repoId}
            hunkStaging={hunkActions.staging}
            hunkDiscard={hunkActions.discard}
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
        class="wicon inline-flex size-11 items-center justify-center rounded-full border border-border bg-card text-primary"
        aria-hidden="true"
      ><Icon icon={iconTree} size={22} /></span>
      <p class="wtitle m-0 text-sm font-medium text-foreground">Nothing open</p>
      {#if status}
        <div
          class="wsummary mt-[0.15rem] flex flex-wrap items-center justify-center gap-[0.35rem]"
          aria-label="Repository summary"
        >
          <span
            class="wchip wbranch inline-flex items-center gap-[0.2rem] whitespace-nowrap rounded-full border border-primary/35 px-2 py-px font-mono text-ui-micro leading-[1.4] tabular-nums text-primary"
          >{branchLabel}</span>
          <span
            class="wchip inline-flex items-center gap-[0.2rem] whitespace-nowrap rounded-full border border-border px-2 py-px text-ui-micro leading-[1.4] tabular-nums"
          >
            {changeCount === 0
              ? "Clean working tree"
              : `${changeCount} changed ${changeCount === 1 ? "file" : "files"}`}
          </span>
          {#if status.ahead > 0}
            <Hint label={`${status.ahead} ${status.ahead === 1 ? "commit" : "commits"} to push`}>
              {#snippet child({ props })}
                <span
                  class="wchip wahead inline-flex items-center gap-[0.2rem] whitespace-nowrap rounded-full border border-transparent bg-accent px-2 py-px text-ui-micro leading-[1.4] tabular-nums text-accent-foreground"
                  {...props}
                >
                  ↑ {status.ahead} to push
                </span>
              {/snippet}
            </Hint>
          {/if}
          {#if status.behind > 0}
            <Hint label={`${status.behind} ${status.behind === 1 ? "commit" : "commits"} to pull`}>
              {#snippet child({ props })}
                <span
                  class="wchip wbehind inline-flex items-center gap-[0.2rem] whitespace-nowrap rounded-full border border-transparent bg-muted px-2 py-px text-ui-micro leading-[1.4] tabular-nums text-muted-foreground"
                  {...props}
                >
                  ↓ {status.behind} to pull
                </span>
              {/snippet}
            </Hint>
          {/if}
        </div>
      {:else}
        <p
          class="wloading mt-[0.15rem] inline-flex items-center gap-[0.4rem] text-ui-micro"
          role="status"
        >
          <Spinner class="size-3" role="presentation" aria-hidden="true" />
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
              class="h-auto rounded-full px-[0.9rem] py-1 text-ui-base"
              type="button"
              onclick={onOpenFirstChange}
            >
              Open first change
            </Button>
          {/if}
          {#if onBrowseHistory}
            <Button
              class="h-auto rounded-full px-[0.9rem] py-1 text-ui-base"
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
        class="whints mt-[0.9rem] flex list-none flex-wrap justify-center gap-x-[1.1rem] gap-y-[0.4rem] border-t border-border pt-[0.7rem] text-ui-micro text-muted-foreground"
        aria-label="Keyboard shortcuts"
      >
        <li class="inline-flex items-center gap-[0.15rem]"><Kbd>Ctrl</Kbd>+<Kbd>Shift</Kbd>+<Kbd>P</Kbd><span class="ml-[0.4rem]">Command palette</span></li>
        <li class="inline-flex items-center gap-[0.15rem]"><Kbd>F5</Kbd><span class="ml-[0.4rem]">Refresh repository</span></li>
        <li class="inline-flex items-center gap-[0.15rem]"><Kbd>Ctrl</Kbd>+<Kbd>1</Kbd><span class="ml-[0.4rem]">Focus source control</span></li>
      </ul>
    </div>
  {/if}
  {/if}
</div>
