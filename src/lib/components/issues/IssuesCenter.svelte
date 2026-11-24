<script lang="ts">
  import { tick } from "svelte";
  import type { Issue, IssueQuery } from "$lib/issues/model";
  import { DEFAULT_ISSUE_QUERY, normalizeIssueQuery } from "$lib/issues/query";
  import { issueViewPrefs } from "$lib/issues/prefs";
  import { createWidthTracker } from "$lib/layout/containerWidth.svelte";
  import SplitPane from "$lib/components/layout/SplitPane.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import IssueList from "./IssueList.svelte";
  import IssueDetail from "./IssueDetail.svelte";
  import IssueEditorDialog from "./IssueEditorDialog.svelte";
  import IssueActivityPopover from "./IssueActivityPopover.svelte";

  let {
    repoId,
    root,
    selectedIssueNumber = null,
    commentId = undefined,
    requestNonce = undefined,
    onSelectIssue,
    onBack,
    onClose,
  }: {
    repoId: string;
    root: string;
    selectedIssueNumber?: number | null;
    commentId?: number | string;
    requestNonce?: number;
    onSelectIssue: (issueNumber: number) => void;
    onBack: () => void;
    onClose: () => void;
  } = $props();

  const iconAdd = $derived(appIcon("add"));
  const iconRefresh = $derived(appIcon("refresh"));
  const iconClose = $derived(appIcon("close"));
  const iconBack = $derived(appIcon("arrow-back-rounded"));
  const tracker = createWidthTracker();
  const wide = $derived(!tracker.narrow);
  let query = $state<IssueQuery>({ ...DEFAULT_ISSUE_QUERY, labels: [], assignees: [] });
  let splitRatio = $state(1 / 3);
  let queryRoot = $state("");
  let listRef = $state<ReturnType<typeof IssueList> | null>(null);
  let detailRef = $state<ReturnType<typeof IssueDetail> | null>(null);
  let editorOpen = $state(false);
  let editorIssue = $state<Issue | null>(null);
  let refreshing = $state(false);

  $effect(() => {
    if (queryRoot === root) return;
    queryRoot = root;
    const stored = issueViewPrefs.read(root);
    query = stored === null
      ? { ...DEFAULT_ISSUE_QUERY, labels: [], assignees: [] }
      : normalizeIssueQuery(stored.query);
    splitRatio = stored?.splitRatio ?? 1 / 3;
    editorOpen = false;
    editorIssue = null;
  });

  function changeQuery(patch: Partial<IssueQuery>): void {
    query = normalizeIssueQuery({ ...query, ...patch });
    issueViewPrefs.setQuery(root, patch);
  }

  function changeRatio(ratio: number): void {
    if (!Number.isFinite(ratio)) return;
    splitRatio = Math.max(0.15, Math.min(0.85, ratio));
    issueViewPrefs.setSplitRatio(root, splitRatio);
  }

  function newIssue(): void {
    editorIssue = null;
    editorOpen = true;
  }

  function editIssue(issue: Issue): void {
    editorIssue = issue;
    editorOpen = true;
  }

  function issueChanged(): void {
    void listRef?.refresh();
  }

  function issueSaved(issue: Issue): void {
    onSelectIssue(issue.number);
    void tick().then(() => void listRef?.refresh());
  }

  async function refresh(): Promise<void> {
    if (refreshing) return;
    refreshing = true;
    try {
      await Promise.all([listRef?.refresh(), selectedIssueNumber === null ? undefined : detailRef?.refresh()]);
    } finally {
      refreshing = false;
    }
  }

  async function focusSearch(): Promise<void> {
    if (tracker.narrow && selectedIssueNumber !== null) onBack();
    await tick();
    listRef?.focusSearch();
  }

  function back(): void {
    onBack();
    void tick().then(() => listRef?.focus());
  }

  function onKeydown(event: KeyboardEvent): void {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "f") {
      event.preventDefault();
      void focusSearch();
      return;
    }
    if (event.key !== "Escape" || event.defaultPrevented) return;
    event.preventDefault();
    event.stopPropagation();
    if (selectedIssueNumber !== null) back();
    else onClose();
  }

  $effect(() => {
    const element = tracker.el;
    if (!element) return;
    element.addEventListener("keydown", onKeydown);
    return () => element.removeEventListener("keydown", onKeydown);
  });
</script>

<div
  class="issues-center flex h-full min-h-0 min-w-0 flex-col bg-card"
  role="region"
  aria-label="Issues"
  bind:this={tracker.el}
>
  <header class="flex flex-none flex-wrap items-center gap-1 border-b border-border bg-popover px-2 py-1">
    {#if tracker.narrow && selectedIssueNumber !== null}
      <Button
        class="h-7 w-7 rounded-sm"
        variant="ghost"
        size="icon-sm"
        type="button"
        aria-label="Back to issues"
        title="Back to issues"
        onclick={back}
      >
        {#if iconBack}<Icon icon={iconBack} size={14} />{/if}
      </Button>
    {/if}
    <h1 class="m-0 min-w-0 flex-1 truncate text-ui-base font-semibold text-foreground" aria-label="Issues">Issues</h1>
    <Button
      class="h-7 gap-1 rounded-sm px-2 text-ui-meta"
      variant="outline"
      type="button"
      aria-label="New issue"
      onclick={newIssue}
    >
      {#if iconAdd}<Icon icon={iconAdd} size={13} />{/if}
      New issue
    </Button>
    <IssueActivityPopover {repoId} {root} />
    <Button
      class="h-7 gap-1 rounded-sm px-2 text-ui-meta"
      variant="ghost"
      type="button"
      aria-label="Refresh issues"
      title="Refresh issues"
      disabled={refreshing}
      onclick={() => void refresh()}
    >
      {#if iconRefresh}<Icon icon={iconRefresh} size={13} />{/if}
      <span class="@max-[38rem]:hidden">Refresh</span>
    </Button>
    <Button
      class="h-7 w-7 rounded-sm"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Close issues"
      title="Close issues"
      onclick={onClose}
    >
      {#if iconClose}<Icon icon={iconClose} size={14} />{/if}
    </Button>
  </header>

  <div class="relative min-h-0 min-w-0 flex-1">
    {#if wide}
      <SplitPane axis="x" ratio={splitRatio} onRatio={changeRatio} label="Issues list width">
        {#snippet a()}
          {@render issueList()}
        {/snippet}
        {#snippet b()}
          {#if selectedIssueNumber !== null}
            <IssueDetail
              bind:this={detailRef}
              {repoId}
              issueNumber={selectedIssueNumber}
              {commentId}
              {requestNonce}
              onEdit={editIssue}
              onChanged={issueChanged}
            />
          {:else}
            <div class="flex min-h-0 flex-1 items-center justify-center p-6 text-center text-ui-meta text-muted-foreground">
              Select an issue to view its description and comments.
            </div>
          {/if}
        {/snippet}
      </SplitPane>
    {:else}
      <div class="h-full min-h-0 min-w-0" class:hidden={selectedIssueNumber !== null} aria-hidden={selectedIssueNumber !== null ? true : undefined}>
        {@render issueList()}
      </div>
      {#if selectedIssueNumber !== null}
        <IssueDetail
          bind:this={detailRef}
          {repoId}
          issueNumber={selectedIssueNumber}
          {commentId}
          {requestNonce}
          onEdit={editIssue}
          onChanged={issueChanged}
        />
      {/if}
    {/if}
  </div>

  <IssueEditorDialog
    {repoId}
    open={editorOpen}
    issue={editorIssue}
    onClose={() => (editorOpen = false)}
    onSaved={issueSaved}
  />
</div>

{#snippet issueList()}
  <IssueList
    bind:this={listRef}
    {repoId}
    {query}
    selectedNumber={selectedIssueNumber}
    onQueryChange={changeQuery}
    onSelect={onSelectIssue}
  />
{/snippet}
