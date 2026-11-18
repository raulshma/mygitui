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
  import type {
    OpenFileInfo,
    WorkspaceCapture,
  } from "$lib/components/files/FileWorkspace.svelte";
  import { diffHunkActions, diffScopeSides, type DiffScope } from "$lib/components/panels/statusModel";
  import { fileForDiffTab, type DiffTab } from "$lib/components/panels/diffTabs";
  import CommitDetail from "$lib/components/panels/CommitDetail.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { createImageLoader } from "$lib/components/diff/imageLoad";
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
  const iconClose = $derived(appIcon("close")!);
  const iconRetry = $derived(appIcon("refresh")!);

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
    diffTabs = [],
    status = null,
    diffFiles = [],
    diffLoading = false,
    diffError = null,
    diffScope = "unstaged",
    historyActions = null,
    workflowRunId = null,
    actionRunId = null,
    entryIndex = null,
    entryCount = 0,
    files = null,
    onClose = undefined,
    onActivateDiffTab = undefined,
    onCloseDiffTab = undefined,
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
    stageBusy = false,
    onRetryDiff = undefined,
    onDiscardEntry = undefined,
    onEditEntry = undefined,
    onActivateFile = undefined,
    onCloseFile = undefined,
    onOpenDiffFile = undefined,
    onOpenFirstChange = undefined,
    onBrowseHistory = undefined,
    onScopeChange = undefined,
    filesSeed = null,
    onFileScroll = undefined,
    attachWorkspace = undefined,
    diffInitialScroll = null,
    diffInitialRow = null,
    onDiffScroll = undefined,
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
    /**
     * Open working-copy diffs (multi-diff tab model). Empty or a single
     * tab renders the plain header; two or more add the tab strip above
     * it (modeled on the file workspace's).
     */
    diffTabs?: DiffTab[];
    /** Latest repo status (welcome summary when nothing is open). */
    status?: RepoStatus | null;
    /** Streamed diff for `entry`. */
    diffFiles?: FileDiff[];
    diffLoading?: boolean;
    /**
     * Failure message from the last diff load (null = none): renders a
     * distinct error empty state with Retry instead of the "No changes"
     * state, which would read as if the file had no diff.
     */
    diffError?: string | null;
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
    /** Activates (focuses) an open diff tab. */
    onActivateDiffTab?: (path: string) => void;
    /** Closes one open diff tab (the owner neighbor-activates the rest). */
    onCloseDiffTab?: (path: string) => void;
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
    /** True while that stage/unstage IPC is in flight (buttons disable). */
    stageBusy?: boolean;
    /** Re-runs the working-copy diff load after a failure. */
    onRetryDiff?: () => void;
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
    /** Re-opens the working-copy diff at the other scope (staged ↔ unstaged). */
    onScopeChange?: (scope: DiffScope) => void;
    /**
     * Workspace-session restore seed for the file workspace: dirty-buffer
     * doc text + saved scroll positions per `file:<path>` key, consumed by
     * the editors the restore mounts (null = plain open, behave as always).
     */
    filesSeed?: {
      docs: Record<string, string>;
      scrolls: Record<string, number>;
    } | null;
    /** Scroll capture from an open file's editor (per-path, px). */
    onFileScroll?: (path: string, scrollTop: number) => void;
    /** Attaches the file workspace's capture API (dirty docs at switch time). */
    attachWorkspace?: (api: WorkspaceCapture | null) => void;
    /** Scroll the next-mounted working-copy diff restores (null = top). */
    diffInitialScroll?: number | null;
    /** Saved first-visible-row seed (row restore beats px drift; null = top). */
    diffInitialRow?: number | null;
    /** Scroll capture from the open working-copy diff (px). */
    onDiffScroll?: (scrollTop: number, firstVisibleRow: number) => void;
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

  /**
   * Image-diff URL loader for the working-copy diff, keyed by repo +
   * scope: `$derived` keeps the identity stable across stream pages (a
   * fresh closure per render would reload every image on each page).
   */
  const imageLoader = $derived.by(() => {
    const { oldSide, newSide } = diffScopeSides(diffScope);
    return createImageLoader(repoId, { old: oldSide, new: newSide });
  });

  /**
   * The scope the empty state can offer instead (`null` when the shown side
   * is the only one with content): an empty diff means the file's change
   * lives entirely on the other side of the index, so the state points
   * there — one clear next action, not a dead end.
   */
  const emptyStateScope = $derived.by<DiffScope | null>(() => {
    if (!entry) return null;
    const other: DiffScope = diffScope === "staged" ? "unstaged" : "staged";
    const otherSideHasChanges =
      other === "staged"
        ? entry.index !== "unmodified"
        : entry.worktree !== "unmodified";
    return otherSideHasChanges ? other : null;
  });

  /**
   * The streamed files the ACTIVE diff tab shows — one diff visible at a
   * time; the stream covers every open path and each tab filters out its
   * own FileDiff list.
   */
  const activeDiffFiles = $derived(
    entry ? fileForDiffTab(diffFiles, entry.path) : [],
  );

  function basename(path: string): string {
    const idx = path.lastIndexOf("/");
    return idx === -1 ? path : path.slice(idx + 1);
  }
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
          class="estate estate-loading m-auto inline-flex items-center gap-ui-4 text-ui-base text-muted-foreground"
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
          initialDocs={filesSeed?.docs ?? null}
          initialScrolls={filesSeed?.scrolls ?? null}
          onScroll={(path, top) => onFileScroll?.(path, top)}
          attach={attachWorkspace}
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
        class="estate estate-loading m-auto inline-flex items-center gap-ui-4 text-ui-base text-muted-foreground"
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
        class="estate estate-loading m-auto inline-flex items-center gap-ui-4 text-ui-base text-muted-foreground"
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
      onCheckout={historyActions?.checkout}
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
      {#if diffTabs.length > 1}
        <!-- Multi-diff tab strip (modeled on the file workspace's): the
             close-all action lives OUTSIDE the scroll container so it
             stays reachable no matter how many tabs are open. -->
        <div class="dtabs-row flex flex-none items-stretch border-b border-border bg-card">
          <div
            class="dtabs flex min-w-0 flex-1 items-stretch overflow-x-auto [scrollbar-width:thin]"
            role="tablist"
            aria-label="Open diffs"
          >
            {#each diffTabs as tab (tab.entry.path)}
              {@const isActive = tab.entry.path === entry.path}
              <Hint label={tab.entry.path}>
                {#snippet child({ props })}
                  <!-- svelte-ignore a11y_click_events_have_key_events -- keyboard
                       handling lives on the diff header's buttons + prev/next -->
                  <div
                    {...props}
                    class="dtab flex h-8 max-w-56 flex-none cursor-pointer items-center gap-ui-3 border-r border-border py-0 pl-2.5 pr-ui-3 whitespace-nowrap select-none text-ui-meta {isActive
                      ? 'bg-card text-foreground shadow-[inset_0_2px_0_var(--primary)]'
                      : 'text-muted-foreground hover:bg-accent'}"
                    role="tab"
                    aria-selected={isActive}
                    tabindex="-1"
                    onclick={() => onActivateDiffTab?.(tab.entry.path)}
                    onauxclick={(event) => {
                      if (event.button === 1) onCloseDiffTab?.(tab.entry.path);
                    }}
                    onmousedown={(event) => {
                      // Middle only: suppress the browser's autoscroll.
                      if (event.button === 1) event.preventDefault();
                    }}
                  >
                    <span class="tname min-w-0 overflow-hidden text-ellipsis">{basename(tab.entry.path)}</span>
                    <Hint
                      label={tab.scope === "staged"
                        ? "Staged changes"
                        : "Unstaged changes"}
                    >
                      {#snippet child({ props: dotProps })}
                        <span
                          {...dotProps}
                          tabindex="-1"
                          class="dscope size-[7px] flex-none rounded-full {tab.scope === "staged"
                            ? "bg-primary"
                            : "bg-muted-foreground/60"}"
                        ></span>
                      {/snippet}
                    </Hint>
                    <Hint label="Close diff (middle-click)">
                      {#snippet child({ props })}
                        <Button
                          {...props}
                          class="tclose size-5 rounded-full text-muted-foreground opacity-65 hover:bg-accent hover:text-accent-foreground hover:opacity-100 [&_svg:not([class*='size-'])]:size-3"
                          variant="ghost"
                          size="icon-sm"
                          type="button"
                          aria-label={`Close ${basename(tab.entry.path)}`}
                          tabindex={-1}
                          onclick={(event) => {
                            event.stopPropagation();
                            onCloseDiffTab?.(tab.entry.path);
                          }}
                        >
                          <Icon icon={iconClose} size={12} />
                        </Button>
                      {/snippet}
                    </Hint>
                  </div>
                {/snippet}
              </Hint>
            {/each}
          </div>
          <div class="dtactions flex flex-none items-center gap-0.5 border-l border-border bg-card px-1">
            {#if onClose}
              <Hint label="Close all diffs">
                {#snippet child({ props })}
                  <Button
                    {...props}
                    class="text-muted-foreground hover:bg-accent hover:text-accent-foreground [&_svg:not([class*='size-'])]:size-3.5"
                    variant="ghost"
                    size="icon-xs"
                    type="button"
                    aria-label="Close all diffs"
                    onclick={onClose}
                  >
                    <Icon icon={iconClose} size={14} />
                  </Button>
                {/snippet}
              </Hint>
            {/if}
          </div>
        </div>
      {/if}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <!-- Middle-click anywhere on the header row closes the editor (the
           CommitDetail header idiom); the buttons inside keep their own
           click flow — their events simply bubble up through this row. -->
      <header
        class="ehead flex min-h-7 flex-none items-center gap-ui-4 border-b border-border bg-popover px-2.5 py-0.5 text-ui-base"
        onauxclick={(event) => {
          if (event.button === 1 && onClose) {
            event.preventDefault();
            onClose();
          }
        }}
        onmousedown={(event) => {
          // Middle only: suppress the browser's autoscroll affordance. The
          // primary button keeps its default so header buttons stay clickable
          // and focusable as usual.
          if (event.button === 1) event.preventDefault();
        }}
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
                  disabled={stageBusy}
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
                  disabled={stageBusy}
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
                class="h-5 flex-none rounded-sm px-1.5 text-ui-micro leading-[1.5] font-normal [&_svg:not([class*='size-'])]:size-3"
                variant="outline"
                type="button"
                {...props}
                aria-label="Close diff"
                onclick={onClose}
              >
                <Icon icon={iconClose} size={12} />
              </Button>
            {/snippet}
          </Hint>
        {/if}
      </header>
      <div class="ebody flex min-h-0 flex-1 flex-col overflow-hidden">
        {#if diffLoading}
          <p
            class="estate estate-loading m-auto inline-flex items-center gap-ui-4 text-ui-base text-muted-foreground"
            role="status"
          >
            <Spinner class="size-4" role="presentation" aria-hidden="true" />
            Loading diff…
          </p>
        {:else if diffError !== null}
          <!-- A failed diff load must not fall through to the "No changes"
               state (it would read as if the file had no diff): distinct
               error state + Retry, same affordance as the status panel's
               statusError block. -->
          <div
            class="flex-1 min-h-0 flex flex-col items-center justify-center gap-2 px-4 py-8 text-center text-muted-foreground select-none"
            role="alert"
          >
            <p class="empty-title m-0 text-ui-base font-medium text-foreground">Couldn't load diff</p>
            <p class="m-0 max-w-64 text-ui-xs break-words">{diffError}</p>
            {#if onRetryDiff}
              <Button variant="outline" size="sm" onclick={onRetryDiff}>
                <Icon icon={iconRetry} size={16} />
                Retry
              </Button>
            {/if}
          </div>
        {:else if activeDiffFiles.length === 0}
          <!-- The ACTIVE tab's file list, not the whole stream's: with
               several tabs open, another tab's files must not hide this
               tab's scope-specific empty state. -->
          <EmptyState
            title={diffScope === "staged"
              ? "No staged changes for this file."
              : "No unstaged changes for this file."}
          >
            {#if emptyStateScope !== null && onScopeChange}
              <Hint
                label={emptyStateScope === "staged"
                  ? "Show this file's HEAD → index diff"
                  : "Show this file's index → worktree diff"}
              >
                {#snippet child({ props })}
                  <Button
                    {...props}
                    class="h-6 rounded-sm px-2.5 text-ui-meta leading-[1.5] font-normal"
                    variant="outline"
                    type="button"
                    onclick={() => onScopeChange?.(emptyStateScope!)}
                  >
                    View{' '}
                    {emptyStateScope === "staged" ? "staged" : "unstaged"}{' '}
                    changes
                  </Button>
                {/snippet}
              </Hint>
            {/if}
          </EmptyState>
        {:else}
          <!-- Keyed by the active tab's path: a tab switch remounts the
               viewer so its one-shot `initialScrollTop` restore applies
               (the outgoing tab's scroll was captured to the workspace
               session while it was visible). -->
          {#key entry.path}
            <DiffViewer
              files={activeDiffFiles}
              {repoId}
              hunkStaging={hunkActions.staging}
              hunkDiscard={hunkActions.discard}
              {onMutated}
              onLoadImage={imageLoader}
              initialScrollTop={diffInitialScroll ?? undefined}
              initialRow={diffInitialRow ?? undefined}
              onScrollChange={onDiffScroll}
            />
          {/key}
        {/if}
      </div>
    </div>
  {:else}
    <div
      class="welcome flex min-h-0 flex-1 select-none flex-col items-center justify-center gap-ui-4 p-8 text-center text-muted-foreground"
    >
      <span
        class="wicon inline-flex size-11 items-center justify-center rounded-full border border-border bg-card text-primary"
        aria-hidden="true"
      ><Icon icon={iconTree} size={22} /></span>
      <p class="wtitle m-0 text-sm font-medium text-foreground">Nothing open</p>
      {#if status}
        <div
          class="wsummary mt-ui-15 flex flex-wrap items-center justify-center gap-ui-35"
          aria-label="Repository summary"
        >
          <span
            class="wchip wbranch inline-flex items-center gap-ui-2 whitespace-nowrap rounded-full border border-primary/35 px-2 py-px font-mono text-ui-micro leading-[1.4] tabular-nums text-primary"
          >{branchLabel}</span>
          <span
            class="wchip inline-flex items-center gap-ui-2 whitespace-nowrap rounded-full border border-border px-2 py-px text-ui-micro leading-[1.4] tabular-nums"
          >
            {changeCount === 0
              ? "Clean working tree"
              : `${changeCount} changed ${changeCount === 1 ? "file" : "files"}`}
          </span>
          {#if status.ahead > 0}
            <Hint label={`${status.ahead} ${status.ahead === 1 ? "commit" : "commits"} to push`}>
              {#snippet child({ props })}
                <span
                  class="wchip wahead inline-flex items-center gap-ui-2 whitespace-nowrap rounded-full border border-transparent bg-accent px-2 py-px text-ui-micro leading-[1.4] tabular-nums text-accent-foreground"
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
                  class="wchip wbehind inline-flex items-center gap-ui-2 whitespace-nowrap rounded-full border border-transparent bg-muted px-2 py-px text-ui-micro leading-[1.4] tabular-nums text-muted-foreground"
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
          class="wloading mt-ui-15 inline-flex items-center gap-ui-4 text-ui-micro"
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
        <li class="inline-flex items-center gap-ui-15"><Kbd>Ctrl</Kbd>+<Kbd>Shift</Kbd>+<Kbd>P</Kbd><span class="ml-ui-4">Command palette</span></li>
        <li class="inline-flex items-center gap-ui-15"><Kbd>F5</Kbd><span class="ml-ui-4">Refresh repository</span></li>
        <li class="inline-flex items-center gap-ui-15"><Kbd>Ctrl</Kbd>+<Kbd>1</Kbd><span class="ml-ui-4">Focus source control</span></li>
      </ul>
    </div>
  {/if}
  {/if}
</div>
