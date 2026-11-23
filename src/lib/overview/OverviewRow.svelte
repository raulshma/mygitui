<script lang="ts">
  /**
   * One Overview row — a slim card per repository. The header line (click =
   * expand/collapse) carries name, branch, sync counts, change counts, PR
   * chips, the last commit and the "updated Xs ago" stamp. The FILE LIST and
   * PR metadata are ALWAYS visible below the header (no click needed);
   * expanding additionally mounts the inline DIFFS (lazy: only the expanded
   * row streams). Repos missing on disk render dimmed with a "missing"
   * badge and only the Remove action. Context menu: Open + Remove from
   * library.
   */
  import type { PrInfo, StatusEntry } from "$lib/ipc/types";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Skeleton } from "$lib/components/ui/skeleton/index.js";
  import { formatRelativeTime } from "$lib/git/format";
  import { openExternal } from "$lib/stores/forge.svelte";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import {
    groupStatusEntries,
    kindLetter,
    openSide,
    refreshedAgo,
    rowKind,
    shortSha,
    splitPath,
    type OverviewRowData,
  } from "./overviewModel";
  import OverviewFileDiff from "./OverviewFileDiff.svelte";

  let {
    row,
    expanded = false,
    now,
    onToggle,
    onOpen,
    onRemove,
  }: {
    row: OverviewRowData;
    /** The row's diffs are mounted (parent decides; conflicts default on). */
    expanded?: boolean;
    /** Wall-clock ms (the View's lightweight ticker) for the stamps. */
    now: number;
    onToggle: (root: string) => void;
    onOpen: (root: string) => void;
    onRemove: (root: string) => void;
  } = $props();

  const iconChevron = $derived(appIcon("chevron-down")!);
  const iconOpen = $derived(appIcon("open-in-new")!);
  const iconFolder = $derived(appIcon("folder-code")!);
  const iconDraft = $derived(appIcon("edit")!);
  const iconApproved = $derived(appIcon("check-circle")!);
  const iconChanges = $derived(appIcon("triangle-alert")!);
  const iconCiFail = $derived(appIcon("error")!);
  const iconStar = $derived(appIcon("star")!);

  let selectedPath = $state<string | null>(null);

  /** Display cap per file group — a huge worktree must not mount thousands
   *  of buttons per row; the header's count chips carry the real totals. */
  const FILE_GROUP_CAP = 30;

  const groups = $derived(groupStatusEntries(row.entries));

  /** First changed file is the default diff selection; keeps a user pick
   *  while that file still exists in the entries. */
  const diffPath = $derived.by(() => {
    if (selectedPath !== null && row.entries.some((e) => e.path === selectedPath)) {
      return selectedPath;
    }
    return row.entries[0]?.path ?? null;
  });

  const diffSide = $derived.by(() => {
    const entry =
      diffPath !== null
        ? (row.entries.find((e) => e.path === diffPath) ?? null)
        : null;
    return entry ? openSide(entry) : "worktree";
  });

  function selectFile(entry: StatusEntry): void {
    selectedPath = entry.path;
  }

  /** Per-file selection uses openSide; the derived `selectedSide` only
   *  follows the explicit pick (defaulting to the entry's own side). */
  function onFileClick(entry: StatusEntry, event: MouseEvent): void {
    event.stopPropagation();
    selectFile(entry);
  }

  const updatedLabel = $derived(refreshedAgo(row.refreshedAt, now));
  const openPrs = $derived<PrInfo[]>(row.prs ?? []);

  /** CI dot per PR chip: the row's failing-checks aggregate is the v1
   *  signal (per-PR check caches only exist for PRs the forge panel
   *  opened). null = unknown → no dot. */
  const ciState = $derived.by<"fail" | "ok" | null>(() => {
    if (row.failingChecks === null) return null;
    return row.failingChecks > 0 ? "fail" : "ok";
  });

  function rowMenu(event: MouseEvent): void {
    const entries: MenuEntry[] = [];
    // Missing repos have nothing to open (the header button is hidden too).
    if (!row.missing) {
      entries.push({ id: "open", label: "Open", run: () => onOpen(row.root) });
      entries.push({ separator: true });
    }
    entries.push({
      id: "remove",
      label: "Remove from library",
      danger: true,
      run: () => onRemove(row.root),
    });
    showMenuAt(event, entries);
  }

  async function openPr(pr: PrInfo): Promise<void> {
    await openExternal(pr.url);
  }
</script>

<article
  class="overview-row flex min-w-0 flex-col overflow-hidden rounded-md border border-border bg-card {row.missing
    ? 'opacity-60'
    : ''}"
  data-root={row.root}
  aria-label={`Repository ${row.name}`}
>
  <!-- Header line: click toggles the inline diffs -->
  <div
    class="row-head flex h-7 min-w-0 cursor-pointer items-center gap-2 border-b border-border/60 px-2 text-ui-base select-none hover:bg-[color:var(--app-hover)]"
    class:expanded={expanded}
    role="button"
    tabindex="0"
    aria-expanded={expanded}
    aria-label={`Toggle diffs for ${row.name}`}
    onclick={() => onToggle(row.root)}
    onkeydown={(event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        onToggle(row.root);
      }
    }}
    oncontextmenu={rowMenu}
  >
    <span
      class="inline-flex flex-none text-muted-foreground transition-transform {expanded
        ? 'rotate-180'
        : ''}"
      aria-hidden="true"
    >
      <Icon icon={iconChevron} size={14} />
    </span>
    <span class="row-name min-w-0 max-w-[16rem] truncate font-semibold">{row.name}</span>
    {#if row.favorite}
      <span class="inline-flex flex-none text-primary-ink" aria-label="Favorite">
        <Icon icon={iconStar} size={12} />
      </span>
    {/if}
    {#if row.pinned}
      <Badge variant="outline" class="flex-none rounded-full px-1.5 py-px text-ui-micro text-muted-foreground">pinned</Badge>
    {/if}
    {#if row.missing}
      <Badge class="flex-none rounded-full px-1.5 py-px text-ui-micro" variant="destructive">missing</Badge>
    {:else if !row.loaded}
      <Skeleton class="h-3.5 w-16 flex-none rounded-full" />
    {:else if row.detached}
      <Badge variant="outline" class="flex-none rounded-full px-1.5 py-px text-ui-micro text-muted-foreground">detached</Badge>
    {:else if row.branch}
      <span class="flex-none rounded-full border border-border px-1.5 py-px font-mono text-ui-micro text-muted-foreground">
        {row.branch}
      </span>
    {/if}
    <span class="sync flex flex-none items-center gap-1 font-mono text-ui-micro tabular-nums">
      {#if row.ahead > 0}
        <span class="text-primary-ink" title={`${row.ahead} ahead`} aria-label={`${row.ahead} ahead`}>↑{row.ahead}</span>
      {/if}
      {#if row.behind > 0}
        <span class="text-primary-ink" title={`${row.behind} behind`} aria-label={`${row.behind} behind`}>↓{row.behind}</span>
      {/if}
    </span>
    <span class="counts flex flex-none items-center gap-1 font-mono text-ui-micro tabular-nums" aria-label="Change counts">
      {#if row.counts.staged > 0}
        <span class="text-primary-ink" title={`${row.counts.staged} staged`}>+{row.counts.staged}</span>
      {/if}
      {#if row.counts.unstaged > 0}
        <span title={`${row.counts.unstaged} unstaged`} class="text-foreground/80">~{row.counts.unstaged}</span>
      {/if}
      {#if row.counts.untracked > 0}
        <span title={`${row.counts.untracked} untracked`} class="text-muted-foreground">?{row.counts.untracked}</span>
      {/if}
      {#if row.counts.conflicts > 0}
        <span class="font-semibold text-destructive" title={`${row.counts.conflicts} conflicts`}>!{row.counts.conflicts}</span>
      {/if}
    </span>
    {#if openPrs.length > 0}
      <span class="prs flex flex-none items-center gap-1" aria-label={`${openPrs.length} open pull requests`}>
        {#each openPrs.slice(0, 3) as pr (pr.number)}
          <span
            class="inline-flex items-center gap-0.5 rounded-full border border-border px-1.5 py-px font-mono text-ui-micro text-muted-foreground"
            title={`#${pr.number} ${pr.title}`}
          >
            #{pr.number}
            {#if pr.is_draft}
              <Icon icon={iconDraft} size={10} />
            {:else if pr.review_decision === "APPROVED"}
              <span class="text-primary-ink"><Icon icon={iconApproved} size={10} /></span>
            {:else if pr.review_decision === "CHANGES_REQUESTED"}
              <span class="text-destructive"><Icon icon={iconChanges} size={10} /></span>
            {/if}
          </span>
        {/each}
        {#if openPrs.length > 3}
          <span class="font-mono text-ui-micro text-muted-foreground">+{openPrs.length - 3}</span>
        {/if}
        {#if ciState === "fail"}
          <span class="text-destructive" title="Failing CI checks on the branch head">
            <Icon icon={iconCiFail} size={11} />
          </span>
        {/if}
      </span>
    {/if}
    <span class="min-w-0 flex-1"></span>
    {#if row.lastCommit}
      <span class="last-commit hidden min-w-0 items-center gap-1.5 text-ui-micro text-muted-foreground md:flex" title={`${shortSha(row.lastCommit.sha)} ${row.lastCommit.summary}`}>
        <span class="flex-none font-mono tabular-nums">{shortSha(row.lastCommit.sha)}</span>
        <span class="min-w-0 max-w-[14rem] truncate">{row.lastCommit.summary}</span>
        <span class="flex-none whitespace-nowrap">{formatRelativeTime(row.lastCommit.timestamp)}</span>
      </span>
    {/if}
    {#if updatedLabel}
      <span class="flex-none whitespace-nowrap text-ui-micro text-muted-foreground">{updatedLabel}</span>
    {/if}
    {#if !row.missing}
      <Hint label={`Open ${row.name} in the workspace`}>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon-sm"
            type="button"
            class="row-open h-5 w-5 [&_svg:not([class*='size-'])]:size-3.5"
            aria-label={`Open ${row.name}`}
            onclick={(event) => {
              event.stopPropagation();
              onOpen(row.root);
            }}
          >
            <Icon icon={iconOpen} size={13} />
          </Button>
        {/snippet}
      </Hint>
    {/if}
  </div>

  <!-- Always visible: file list (grouped) + PR metadata -->
  <div class="row-detail flex min-w-0 flex-col gap-1 px-2 py-1.5 text-ui-meta">
    {#if row.missing}
      <p class="m-0 flex items-center gap-1.5 text-muted-foreground" role="note">
        <Icon icon={iconFolder} size={12} />
        Folder not found on disk. Remove it from the library, or re-scan after
        restoring the folder.
      </p>
    {:else if !row.loaded}
      <div class="flex flex-col gap-1" aria-hidden="true">
        <Skeleton class="h-4 w-2/3 rounded-sm" />
        <Skeleton class="h-4 w-1/3 rounded-sm" />
      </div>
    {:else}
      <div class="files flex min-w-0 flex-col gap-0.5">
        {#if groups.staged.length > 0}
          <div class="file-group" role="group" aria-label="Staged files">
            {#each groups.staged.slice(0, FILE_GROUP_CAP) as entry (entry.path)}
              <button
                class="file-row flex h-6 w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-sm px-1 text-left text-ui-meta hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {diffPath === entry.path
                  ? 'bg-[color:var(--app-hover)]'
                  : ''}"
                type="button"
                aria-label={`Show diff for ${entry.path} (staged)`}
                onclick={(event) => onFileClick(entry, event)}
              >
                <span class="kind w-3 flex-none text-center font-mono text-ui-micro font-semibold text-primary-ink" aria-hidden="true">{kindLetter(rowKind(entry))}</span>
                {#if entry.old_path}
                  <span class="min-w-0 truncate font-mono" title={`${entry.old_path} → ${entry.path}`}>
                    {splitPath(entry.old_path).base} → {splitPath(entry.path).base}
                  </span>
                {:else}
                  <span class="dir flex-none font-mono text-muted-foreground" aria-hidden="true">{splitPath(entry.path).dir !== "" ? `${splitPath(entry.path).dir}/` : ""}</span>
                  <span class="min-w-0 truncate font-mono">{splitPath(entry.path).base}</span>
                {/if}
                <span class="flex-none text-ui-micro text-muted-foreground">staged</span>
              </button>
            {/each}
            {#if groups.staged.length > FILE_GROUP_CAP}
              <p class="m-0 px-1 text-ui-micro text-muted-foreground">
                +{groups.staged.length - FILE_GROUP_CAP} more staged
              </p>
            {/if}
          </div>
        {/if}
        {#if groups.changed.length > 0}
          <div class="file-group" role="group" aria-label="Changed files">
            {#each groups.changed.slice(0, FILE_GROUP_CAP) as entry (entry.path)}
              <button
                class="file-row flex h-6 w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-sm px-1 text-left text-ui-meta hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {diffPath === entry.path
                  ? 'bg-[color:var(--app-hover)]'
                  : ''}"
                type="button"
                aria-label={`Show diff for ${entry.path}`}
                onclick={(event) => onFileClick(entry, event)}
              >
                <span class="kind w-3 flex-none text-center font-mono text-ui-micro font-semibold" aria-hidden="true">{kindLetter(rowKind(entry))}</span>
                {#if entry.old_path}
                  <span class="min-w-0 truncate font-mono" title={`${entry.old_path} → ${entry.path}`}>
                    {splitPath(entry.old_path).base} → {splitPath(entry.path).base}
                  </span>
                {:else}
                  <span class="dir flex-none font-mono text-muted-foreground" aria-hidden="true">{splitPath(entry.path).dir !== "" ? `${splitPath(entry.path).dir}/` : ""}</span>
                  <span class="min-w-0 truncate font-mono">{splitPath(entry.path).base}</span>
                {/if}
              </button>
            {/each}
            {#if groups.changed.length > FILE_GROUP_CAP}
              <p class="m-0 px-1 text-ui-micro text-muted-foreground">
                +{groups.changed.length - FILE_GROUP_CAP} more changed
              </p>
            {/if}
          </div>
        {/if}
        {#if groups.untracked.length > 0}
          <div class="file-group" role="group" aria-label="Untracked files">
            {#each groups.untracked.slice(0, FILE_GROUP_CAP) as entry (entry.path)}
              <button
                class="file-row flex h-6 w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-sm px-1 text-left text-ui-meta hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {diffPath === entry.path
                  ? 'bg-[color:var(--app-hover)]'
                  : ''}"
                type="button"
                aria-label={`Show diff for ${entry.path} (untracked)`}
                onclick={(event) => onFileClick(entry, event)}
              >
                <span class="kind w-3 flex-none text-center font-mono text-ui-micro font-semibold text-muted-foreground" aria-hidden="true">{kindLetter(rowKind(entry))}</span>
                <span class="dir flex-none font-mono text-muted-foreground" aria-hidden="true">{splitPath(entry.path).dir !== "" ? `${splitPath(entry.path).dir}/` : ""}</span>
                <span class="min-w-0 truncate font-mono">{splitPath(entry.path).base}</span>
                <span class="flex-none text-ui-micro text-muted-foreground">untracked</span>
              </button>
            {/each}
            {#if groups.untracked.length > FILE_GROUP_CAP}
              <p class="m-0 px-1 text-ui-micro text-muted-foreground">
                +{groups.untracked.length - FILE_GROUP_CAP} more untracked
              </p>
            {/if}
          </div>
        {/if}
        {#if row.entries.length === 0}
          <p class="m-0 text-muted-foreground">Working copy clean</p>
        {/if}
      </div>

      <div class="pr-list flex min-w-0 flex-col gap-0.5">
        {#if row.prs === null}
          <!-- No gh data yet (non-open non-warm repo without a cache). -->
          <span
            class="pr-unknown w-fit cursor-help rounded-sm border-b border-dashed border-border font-mono text-muted-foreground"
            title="open or expand for live GitHub data"
            aria-label="GitHub data not loaded"
          >—</span>
        {:else if openPrs.length === 0}
          <span class="text-muted-foreground">No open PRs</span>
        {:else}
          {#each openPrs as pr (pr.number)}
            <button
              class="pr-row flex h-6 w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-sm px-1 text-left text-ui-meta hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
              type="button"
              title={pr.title}
              aria-label={`Open pull request #${pr.number}: ${pr.title}`}
              onclick={() => void openPr(pr)}
            >
              <span class="flex-none font-mono text-muted-foreground">#{pr.number}</span>
              <span class="min-w-0 flex-1 truncate">{pr.title}</span>
              {#if pr.is_draft}
                <span class="flex-none rounded-full border border-border px-1.5 text-ui-micro font-semibold text-muted-foreground">Draft</span>
              {/if}
              {#if pr.review_decision === "APPROVED"}
                <span class="flex-none text-primary-ink" title="Approved">
                  <Icon icon={iconApproved} size={12} />
                </span>
              {:else if pr.review_decision === "CHANGES_REQUESTED"}
                <span class="flex-none text-destructive" title="Changes requested">
                  <Icon icon={iconChanges} size={12} />
                </span>
              {/if}
              {#if pr.author}
                <span class="max-w-[8rem] flex-none truncate text-muted-foreground">{pr.author}</span>
              {/if}
            </button>
          {/each}
        {/if}
      </div>
    {/if}
  </div>

  <!-- Expanded: the inline diffs (lazy — mounted only here). -->
  {#if expanded && !row.missing && row.loaded}
    <div class="row-diffs min-h-0 border-t border-border bg-background p-1.5">
      {#if row.repoId !== null && diffPath !== null}
        <div class="flex h-[16rem] flex-col">
          <OverviewFileDiff repoId={row.repoId} path={diffPath} side={diffSide} />
        </div>
      {:else if row.repoId === null}
        <p class="m-0 p-2 text-ui-meta text-muted-foreground" role="note">
          Expanding this repository… its diff opens once the session is ready.
        </p>
      {:else}
        <p class="m-0 p-2 text-ui-meta text-muted-foreground">No changes to diff.</p>
      {/if}
    </div>
  {/if}
</article>
