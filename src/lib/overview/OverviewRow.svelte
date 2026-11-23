<script lang="ts">
  /**
   * Modern, visually striking Overview row card per repository.
   *
   * Features:
   * - Prominent repository identity with provider icon & badge (GitHub, GitLab, SSH Device, Local only)
   * - Favorite star & Pinned badge
   * - Branch badge with git-branch icon, or amber detached badge
   * - Remote Sync Status pill (In sync with remote, ↑ push, ↓ pull, diverged, local-only)
   * - Git Changes status (Clean soft pill vs color-coded +staged, ~unstaged, ?untracked, !conflicts)
   * - GitHub / CI signals (Open PRs pills with draft/approved icons, CI failing/passing badge)
   * - Relative timestamps (Last commit snippet, Last accessed time, Updated time)
   * - Quick "Open" button & context menu
   * - Only shows file list / PR list when expanded or when attention is needed (no empty dead text!)
   * - Lazy inline diff streaming when expanded.
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
    lastOpenedAgo,
    openSide,
    refreshedAgo,
    remoteSyncStatus,
    repoProvider,
    rowDirty,
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
  const iconDevice = $derived(appIcon("devices")!);
  const iconCloud = $derived(appIcon("cloud")!);
  const iconBranch = $derived(appIcon("account-tree")!);
  const iconDraft = $derived(appIcon("edit")!);
  const iconApproved = $derived(appIcon("check-circle")!);
  const iconChanges = $derived(appIcon("triangle-alert")!);
  const iconCiFail = $derived(appIcon("error")!);
  const iconStar = $derived(appIcon("star")!);
  const iconCheck = $derived(appIcon("check")!);

  let selectedPath = $state<string | null>(null);

  /** Display cap per file group — a huge worktree must not mount thousands
   *  of buttons per row; the header's count chips carry the real totals. */
  const FILE_GROUP_CAP = 30;

  const groups = $derived(groupStatusEntries(row.entries));
  const isDirty = $derived(rowDirty(row) || row.entries.length > 0);
  const provider = $derived(repoProvider(row));
  const syncInfo = $derived(remoteSyncStatus(row));
  const accessedLabel = $derived(lastOpenedAgo(row.lastOpened, now));
  const updatedLabel = $derived(refreshedAgo(row.refreshedAt, now));
  const openPrs = $derived<PrInfo[]>(row.prs ?? []);

  /** True when the repo has attention items (dirty files, conflicts, PRs, failing CI) */
  const hasAttention = $derived(
    row.counts.conflicts > 0 || isDirty || openPrs.length > 0 || (row.failingChecks ?? 0) > 0
  );

  /** Show details section if expanded, missing, or attention is needed */
  const shouldShowDetails = $derived(expanded || hasAttention || row.missing);

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

  function onFileClick(entry: StatusEntry, event: MouseEvent): void {
    event.stopPropagation();
    selectFile(entry);
  }

  /** CI dot per PR chip: the row's failing-checks aggregate is the v1 signal. */
  const ciState = $derived.by<"fail" | "ok" | null>(() => {
    if (row.failingChecks === null) return null;
    return row.failingChecks > 0 ? "fail" : "ok";
  });

  function rowMenu(event: MouseEvent): void {
    const entries: MenuEntry[] = [];
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
  class="overview-row group flex min-w-0 flex-col overflow-hidden rounded-lg border border-border/80 bg-card/90 shadow-2xs transition-all duration-150 hover:border-border hover:shadow-xs {row.missing
    ? 'border-dashed opacity-60'
    : ''}"
  data-root={row.root}
  aria-label={`Repository ${row.name}`}
>
  <!-- Card Header Line: click toggles expand/collapse -->
  <div
    class="row-head flex min-h-10 min-w-0 cursor-pointer flex-wrap items-center justify-between gap-x-3 gap-y-1.5 px-3 py-2 text-ui-base select-none transition-colors hover:bg-[color:var(--app-hover)]/60"
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
    <!-- Left Section: Expand icon, Provider icon, Repo Name, Badges, Branch -->
    <div class="flex min-w-0 flex-wrap items-center gap-2">
      <span
        class="inline-flex flex-none text-muted-foreground transition-transform duration-200 {expanded
          ? 'rotate-180'
          : ''}"
        aria-hidden="true"
      >
        <Icon icon={iconChevron} size={15} />
      </span>

      <!-- Provider Icon -->
      <span class="inline-flex flex-none items-center text-muted-foreground" aria-hidden="true">
        {#if provider === "github"}
          <svg class="size-4" viewBox="0 0 16 16" fill="currentColor">
            <path fill-rule="evenodd" d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" clip-rule="evenodd"/>
          </svg>
        {:else if provider === "gitlab"}
          <svg class="size-4 text-orange-500" viewBox="0 0 16 16" fill="currentColor">
            <path d="m15.97 9.058-1.8-5.529a.488.488 0 0 0-.928 0l-1.8 5.529H4.558l-1.8-5.529a.488.488 0 0 0-.928 0l-1.8 5.529a.489.489 0 0 0 .177.547l7.447 5.412a.611.611 0 0 0 .72 0l7.447-5.412a.489.489 0 0 0 .177-.547"/>
          </svg>
        {:else if provider === "remote-device"}
          <Icon icon={iconDevice} size={15} />
        {:else if provider === "remote"}
          <Icon icon={iconCloud} size={15} />
        {:else}
          <Icon icon={iconFolder} size={15} />
        {/if}
      </span>

      <!-- Repository Name -->
      <span class="row-name min-w-0 max-w-[16rem] truncate text-sm font-semibold tracking-tight text-foreground">
        {row.name}
      </span>

      <!-- Favorite & Pinned -->
      {#if row.favorite}
        <span class="inline-flex flex-none text-amber-500" aria-label="Favorite" title="Favorite">
          <Icon icon={iconStar} size={13} />
        </span>
      {/if}
      {#if row.pinned}
        <Badge variant="outline" class="flex-none rounded-full px-2 py-0 text-ui-micro font-medium text-muted-foreground">
          pinned
        </Badge>
      {/if}

      <!-- Provider Type Badge -->
      {#if provider === "github"}
        <span class="inline-flex flex-none items-center rounded-full border border-border/70 bg-muted/50 px-2 py-0.5 text-ui-micro font-medium text-muted-foreground">
          GitHub
        </span>
      {:else if provider === "gitlab"}
        <span class="inline-flex flex-none items-center rounded-full border border-orange-500/25 bg-orange-500/10 px-2 py-0.5 text-ui-micro font-medium text-orange-600 dark:text-orange-400">
          GitLab
        </span>
      {:else if provider === "remote-device"}
        <span class="inline-flex flex-none items-center rounded-full border border-sky-500/25 bg-sky-500/10 px-2 py-0.5 text-ui-micro font-medium text-sky-600 dark:text-sky-400">
          SSH Device
        </span>
      {:else if provider === "local"}
        <span class="inline-flex flex-none items-center rounded-full border border-border/60 bg-muted/40 px-2 py-0.5 text-ui-micro font-medium text-muted-foreground">
          Local only
        </span>
      {/if}

      <!-- Branch Badge -->
      {#if row.missing}
        <Badge class="flex-none rounded-full px-2 py-0.5 text-ui-micro" variant="destructive">missing</Badge>
      {:else if !row.loaded}
        <Skeleton class="h-4 w-16 flex-none rounded-full" />
      {:else if row.detached}
        <Badge variant="outline" class="flex-none rounded-full border-amber-500/40 bg-amber-500/10 px-2 py-0.5 text-ui-micro font-medium text-amber-600 dark:text-amber-400">
          detached
        </Badge>
      {:else if row.branch}
        <span class="inline-flex flex-none items-center gap-1 rounded-full border border-border/80 bg-muted/40 px-2 py-0.5 font-mono text-ui-micro text-muted-foreground">
          <Icon icon={iconBranch} size={11} />
          {row.branch}
        </span>
      {/if}
    </div>

    <!-- Middle Section: Sync Status, Changes, PRs, CI -->
    <div class="flex flex-wrap items-center gap-2">
      <!-- Remote Sync Status pill -->
      {#if row.loaded && !row.missing}
        {#if (row.ahead > 0 && row.behind > 0) || syncInfo.status === "diverged"}
          <span
            class="inline-flex items-center gap-1.5 rounded-full border border-amber-500/30 bg-amber-500/10 px-2.5 py-0.5 font-mono text-ui-micro font-medium text-amber-600 dark:text-amber-400"
          >
            <span title={`${row.ahead} ahead`} aria-label={`${row.ahead} ahead`}>↑ {row.ahead}</span>
            <span title={`${row.behind} behind`} aria-label={`${row.behind} behind`}>↓ {row.behind}</span>
          </span>
        {:else if row.ahead > 0 || syncInfo.status === "needs-push"}
          <span
            class="inline-flex items-center gap-1 rounded-full border border-sky-500/30 bg-sky-500/10 px-2.5 py-0.5 font-mono text-ui-micro font-medium text-sky-600 dark:text-sky-400"
            title={`${row.ahead} ahead`}
            aria-label={`${row.ahead} ahead`}
          >
            ↑ {row.ahead} to push
          </span>
        {:else if row.behind > 0 || syncInfo.status === "needs-pull"}
          <span
            class="inline-flex items-center gap-1 rounded-full border border-orange-500/30 bg-orange-500/10 px-2.5 py-0.5 font-mono text-ui-micro font-medium text-orange-600 dark:text-orange-400"
            title={`${row.behind} behind`}
            aria-label={`${row.behind} behind`}
          >
            ↓ {row.behind} to pull
          </span>
        {:else if syncInfo.status === "in-sync" || row.hasRemote}
          <span
            class="inline-flex items-center gap-1 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-0.5 text-ui-micro font-medium text-emerald-600 dark:text-emerald-400"
            title="In sync with remote"
          >
            <Icon icon={iconCheck} size={11} />
            In sync with remote
          </span>
        {:else if provider !== "local"}
          <span class="inline-flex items-center rounded-full border border-border/70 bg-muted/40 px-2.5 py-0.5 text-ui-micro font-medium text-muted-foreground">
            Local only
          </span>
        {/if}
      {/if}

      <!-- Git Changes status -->
      {#if row.loaded && !row.missing}
        {#if !isDirty}
          <span class="inline-flex items-center gap-1 rounded-full border border-emerald-500/25 bg-emerald-500/[0.08] px-2 py-0.5 text-ui-micro font-medium text-emerald-600 dark:text-emerald-400">
            <Icon icon={iconCheck} size={10} />
            Clean
          </span>
        {:else}
          <span class="counts flex flex-none items-center gap-1 font-mono text-ui-micro tabular-nums" aria-label="Change counts">
            {#if row.counts.staged > 0}
              <span
                class="inline-flex items-center rounded-md border border-emerald-500/30 bg-emerald-500/10 px-1.5 py-0.2 font-semibold text-emerald-600 dark:text-emerald-400"
                title={`${row.counts.staged} staged`}
              >
                +{row.counts.staged}
              </span>
            {/if}
            {#if row.counts.unstaged > 0}
              <span
                class="inline-flex items-center rounded-md border border-amber-500/30 bg-amber-500/10 px-1.5 py-0.2 font-semibold text-amber-600 dark:text-amber-400"
                title={`${row.counts.unstaged} unstaged`}
              >
                ~{row.counts.unstaged}
              </span>
            {/if}
            {#if row.counts.untracked > 0}
              <span
                class="inline-flex items-center rounded-md border border-border/80 bg-muted/50 px-1.5 py-0.2 font-medium text-muted-foreground"
                title={`${row.counts.untracked} untracked`}
              >
                ?{row.counts.untracked}
              </span>
            {/if}
            {#if row.counts.conflicts > 0}
              <span
                class="inline-flex items-center rounded-md border border-destructive/40 bg-destructive/10 px-1.5 py-0.2 font-bold text-destructive animate-pulse"
                title={`${row.counts.conflicts} conflicts`}
              >
                !{row.counts.conflicts}
              </span>
            {/if}
          </span>
        {/if}
      {/if}

      <!-- Open PRs signals -->
      {#if openPrs.length > 0}
        <span class="prs flex flex-none items-center gap-1" aria-label={`${openPrs.length} open pull requests`}>
          {#each openPrs.slice(0, 3) as pr (pr.number)}
            <span
              class="inline-flex items-center gap-1 rounded-full border border-purple-500/30 bg-purple-500/10 px-2 py-0.5 font-mono text-ui-micro font-medium text-purple-600 dark:text-purple-400"
              title={`#${pr.number} ${pr.title}`}
            >
              #{pr.number}
              {#if pr.is_draft}
                <Icon icon={iconDraft} size={10} />
              {:else if pr.review_decision === "APPROVED"}
                <span class="text-emerald-500" title="Approved"><Icon icon={iconApproved} size={10} /></span>
              {:else if pr.review_decision === "CHANGES_REQUESTED"}
                <span class="text-destructive" title="Changes requested"><Icon icon={iconChanges} size={10} /></span>
              {/if}
            </span>
          {/each}
          {#if openPrs.length > 3}
            <span class="font-mono text-ui-micro text-muted-foreground">+{openPrs.length - 3}</span>
          {/if}
        </span>
      {/if}

      <!-- CI checks signals -->
      {#if ciState === "fail"}
        <span
          class="inline-flex items-center gap-1 rounded-full border border-destructive/30 bg-destructive/10 px-2 py-0.5 text-ui-micro font-medium text-destructive"
          title="Failing CI checks on the branch head"
        >
          <Icon icon={iconCiFail} size={11} />
          {row.failingChecks ?? 1} failing
        </span>
      {/if}

      <!-- GitHub data indicator when null -->
      {#if row.prs === null}
        <span
          class="pr-unknown inline-flex cursor-help rounded-sm border-b border-dashed border-border px-1 font-mono text-ui-micro text-muted-foreground"
          title="open or expand for live GitHub data"
          aria-label="GitHub data not loaded"
        >—</span>
      {/if}
    </div>

    <!-- Right Section: Timestamps & Quick Open Button -->
    <div class="flex min-w-0 flex-none items-center gap-2.5">
      <!-- Last commit pill/snippet -->
      {#if row.lastCommit}
        <span
          class="last-commit hidden min-w-0 items-center gap-1.5 text-ui-micro text-muted-foreground lg:flex"
          title={`${shortSha(row.lastCommit.sha)} ${row.lastCommit.summary}`}
        >
          <span class="flex-none rounded bg-muted/70 px-1.5 py-0.5 font-mono text-[11px] font-medium text-foreground tabular-nums">
            {shortSha(row.lastCommit.sha)}
          </span>
          <span class="min-w-0 max-w-[13rem] truncate text-foreground/90">{row.lastCommit.summary}</span>
          <span class="flex-none whitespace-nowrap text-muted-foreground/80">{formatRelativeTime(row.lastCommit.timestamp)}</span>
        </span>
      {/if}

      <!-- Last accessed timestamp -->
      {#if accessedLabel}
        <span class="hidden whitespace-nowrap text-ui-micro text-muted-foreground/80 xl:inline-flex" title="Last opened in mygitui">
          {accessedLabel}
        </span>
      {/if}

      <!-- Updated timestamp -->
      {#if updatedLabel}
        <span class="flex-none whitespace-nowrap text-ui-micro text-muted-foreground/70">{updatedLabel}</span>
      {/if}

      <!-- Quick Open button -->
      {#if !row.missing}
        <Hint label={`Open ${row.name} in the workspace`}>
          {#snippet child({ props })}
            <Button
              {...props}
              variant="outline"
              size="sm"
              type="button"
              class="row-open h-6 gap-1 px-2 text-ui-meta font-medium shadow-2xs hover:bg-primary hover:text-primary-foreground transition-colors"
              aria-label={`Open ${row.name}`}
              onclick={(event) => {
                event.stopPropagation();
                onOpen(row.root);
              }}
            >
              <span>Open</span>
              <Icon icon={iconOpen} size={11} />
            </Button>
          {/snippet}
        </Hint>
      {/if}
    </div>
  </div>

  <!-- Details / File List & PR List Section: Only shown when expanded, missing, or attention needed -->
  {#if shouldShowDetails}
    <div class="row-detail flex min-w-0 flex-col gap-1.5 border-t border-border/50 bg-background/30 px-3 py-2 text-ui-meta">
      {#if row.missing}
        <p class="m-0 flex items-center gap-2 py-1 text-ui-meta text-muted-foreground" role="note">
          <Icon icon={iconFolder} size={14} />
          Folder not found on disk. Remove it from the library, or re-scan after restoring the folder.
        </p>
      {:else if !row.loaded}
        <div class="flex flex-col gap-1.5 py-1" aria-hidden="true">
          <Skeleton class="h-4 w-2/3 rounded-sm" />
          <Skeleton class="h-4 w-1/3 rounded-sm" />
        </div>
      {:else}
        <!-- File list grouped by Staged, Changed, Untracked -->
        {#if isDirty}
          <div class="files flex min-w-0 flex-col gap-0.5">
            {#if groups.staged.length > 0}
              <div class="file-group" role="group" aria-label="Staged files">
                {#each groups.staged.slice(0, FILE_GROUP_CAP) as entry (entry.path)}
                  <button
                    class="file-row flex h-6 w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-sm px-1.5 text-left text-ui-meta hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {diffPath === entry.path
                      ? 'bg-[color:var(--app-hover)] font-medium'
                      : ''}"
                    type="button"
                    aria-label={`Show diff for ${entry.path} (staged)`}
                    onclick={(event) => onFileClick(entry, event)}
                  >
                    <span class="kind w-3 flex-none text-center font-mono text-ui-micro font-semibold text-emerald-600 dark:text-emerald-400" aria-hidden="true">
                      {kindLetter(rowKind(entry))}
                    </span>
                    {#if entry.old_path}
                      <span class="min-w-0 truncate font-mono" title={`${entry.old_path} → ${entry.path}`}>
                        {splitPath(entry.old_path).base} → {splitPath(entry.path).base}
                      </span>
                    {:else}
                      <span class="dir flex-none font-mono text-muted-foreground" aria-hidden="true">
                        {splitPath(entry.path).dir !== "" ? `${splitPath(entry.path).dir}/` : ""}
                      </span>
                      <span class="min-w-0 truncate font-mono">{splitPath(entry.path).base}</span>
                    {/if}
                    <span class="flex-none text-ui-micro text-muted-foreground/80">staged</span>
                  </button>
                {/each}
                {#if groups.staged.length > FILE_GROUP_CAP}
                  <p class="m-0 px-1.5 text-ui-micro text-muted-foreground">
                    +{groups.staged.length - FILE_GROUP_CAP} more staged
                  </p>
                {/if}
              </div>
            {/if}

            {#if groups.changed.length > 0}
              <div class="file-group" role="group" aria-label="Changed files">
                {#each groups.changed.slice(0, FILE_GROUP_CAP) as entry (entry.path)}
                  <button
                    class="file-row flex h-6 w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-sm px-1.5 text-left text-ui-meta hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {diffPath === entry.path
                      ? 'bg-[color:var(--app-hover)] font-medium'
                      : ''}"
                    type="button"
                    aria-label={`Show diff for ${entry.path}`}
                    onclick={(event) => onFileClick(entry, event)}
                  >
                    <span class="kind w-3 flex-none text-center font-mono text-ui-micro font-semibold text-amber-600 dark:text-amber-400" aria-hidden="true">
                      {kindLetter(rowKind(entry))}
                    </span>
                    {#if entry.old_path}
                      <span class="min-w-0 truncate font-mono" title={`${entry.old_path} → ${entry.path}`}>
                        {splitPath(entry.old_path).base} → {splitPath(entry.path).base}
                      </span>
                    {:else}
                      <span class="dir flex-none font-mono text-muted-foreground" aria-hidden="true">
                        {splitPath(entry.path).dir !== "" ? `${splitPath(entry.path).dir}/` : ""}
                      </span>
                      <span class="min-w-0 truncate font-mono">{splitPath(entry.path).base}</span>
                    {/if}
                  </button>
                {/each}
                {#if groups.changed.length > FILE_GROUP_CAP}
                  <p class="m-0 px-1.5 text-ui-micro text-muted-foreground">
                    +{groups.changed.length - FILE_GROUP_CAP} more changed
                  </p>
                {/if}
              </div>
            {/if}

            {#if groups.untracked.length > 0}
              <div class="file-group" role="group" aria-label="Untracked files">
                {#each groups.untracked.slice(0, FILE_GROUP_CAP) as entry (entry.path)}
                  <button
                    class="file-row flex h-6 w-full min-w-0 cursor-pointer items-center gap-1.5 rounded-sm px-1.5 text-left text-ui-meta hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {diffPath === entry.path
                      ? 'bg-[color:var(--app-hover)] font-medium'
                      : ''}"
                    type="button"
                    aria-label={`Show diff for ${entry.path} (untracked)`}
                    onclick={(event) => onFileClick(entry, event)}
                  >
                    <span class="kind w-3 flex-none text-center font-mono text-ui-micro font-semibold text-muted-foreground" aria-hidden="true">
                      {kindLetter(rowKind(entry))}
                    </span>
                    <span class="dir flex-none font-mono text-muted-foreground" aria-hidden="true">
                      {splitPath(entry.path).dir !== "" ? `${splitPath(entry.path).dir}/` : ""}
                    </span>
                    <span class="min-w-0 truncate font-mono">{splitPath(entry.path).base}</span>
                    <span class="flex-none text-ui-micro text-muted-foreground/80">untracked</span>
                  </button>
                {/each}
                {#if groups.untracked.length > FILE_GROUP_CAP}
                  <p class="m-0 px-1.5 text-ui-micro text-muted-foreground">
                    +{groups.untracked.length - FILE_GROUP_CAP} more untracked
                  </p>
                {/if}
              </div>
            {/if}
          </div>
        {/if}

        <!-- PR metadata list -->
        {#if openPrs.length > 0 || (expanded && row.prs !== null && openPrs.length === 0)}
          <div class="pr-list flex min-w-0 flex-col gap-0.5">
            {#if openPrs.length === 0}
              <span class="px-1 text-ui-micro text-muted-foreground">No open PRs</span>
            {:else}
              {#each openPrs as pr (pr.number)}
                <button
                  class="pr-row flex h-6 w-full min-w-0 cursor-pointer items-center gap-2 rounded-sm px-1.5 text-left text-ui-meta hover:bg-[color:var(--app-hover)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                  type="button"
                  title={pr.title}
                  aria-label={`Open pull request #${pr.number}: ${pr.title}`}
                  onclick={() => void openPr(pr)}
                >
                  <span class="flex-none font-mono font-medium text-purple-600 dark:text-purple-400">#{pr.number}</span>
                  <span class="min-w-0 flex-1 truncate">{pr.title}</span>
                  {#if pr.is_draft}
                    <span class="flex-none rounded-full border border-border px-1.5 text-ui-micro font-medium text-muted-foreground">Draft</span>
                  {/if}
                  {#if pr.review_decision === "APPROVED"}
                    <span class="flex-none text-emerald-500" title="Approved">
                      <Icon icon={iconApproved} size={12} />
                    </span>
                  {:else if pr.review_decision === "CHANGES_REQUESTED"}
                    <span class="flex-none text-destructive" title="Changes requested">
                      <Icon icon={iconChanges} size={12} />
                    </span>
                  {/if}
                  {#if pr.author}
                    <span class="max-w-[8rem] flex-none truncate text-ui-micro text-muted-foreground">{pr.author}</span>
                  {/if}
                </button>
              {/each}
            {/if}
          </div>
        {/if}
      {/if}
    </div>
  {/if}

  <!-- Expanded: Inline diff viewer (lazy — mounted only when expanded) -->
  {#if expanded && !row.missing && row.loaded}
    <div class="row-diffs min-h-0 border-t border-border bg-background p-2">
      {#if row.repoId !== null && diffPath !== null}
        <div class="flex h-[18rem] flex-col rounded-md border border-border/80 overflow-hidden">
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
