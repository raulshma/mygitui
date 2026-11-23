<script lang="ts">
  /**
   * Executive command dashboard & summary strip for Overview:
   * 1. KPI summary cards (Total Repos, Attention Needed, Out of Sync, Open PRs, In Sync & Clean)
   * 2. Filter preset chips (Needs push, Needs pull, Dirty, Has PRs, Failing checks, Missing, Local only)
   * 3. Actions: Add folder, Refresh, and Chat rail toggle.
   */
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import type { Banner } from "$lib/components/forge/forgeModel";
  import {
    OVERVIEW_FILTERS,
    type OverviewFilterId,
    type OverviewTotals,
  } from "./overviewModel";

  let {
    totals,
    filter,
    banner,
    refreshing = false,
    refreshedLabel = null,
    showChatToggle = false,
    chatRailOpen = true,
    onFilter,
    onRefresh,
    onAddFolder,
    onToggleChatRail,
  }: {
    totals: OverviewTotals;
    filter: OverviewFilterId | null;
    /** gh availability for the whole view (null = still probing). */
    banner: Banner;
    refreshing?: boolean;
    /** "Refreshed Xs ago" for the whole list (null = never refreshed). */
    refreshedLabel?: string | null;
    showChatToggle?: boolean;
    chatRailOpen?: boolean;
    onFilter: (filter: OverviewFilterId | null) => void;
    onRefresh: () => void;
    onAddFolder: () => void;
    onToggleChatRail?: () => void;
  } = $props();

  const iconAdd = $derived(appIcon("add")!);
  const iconRefresh = $derived(appIcon("refresh")!);
  const iconRail = $derived(appIcon("right-panel-close")!);
  const iconRepos = $derived(appIcon("folder-code")!);
  const iconAttention = $derived(appIcon("triangle-alert")!);
  const iconSync = $derived(appIcon("sync")!);
  const iconPr = $derived(appIcon("pull-request")!);
  const iconClean = $derived(appIcon("check-circle")!);
  const iconInfo = $derived(appIcon("info")!);

  function toggleFilter(id: OverviewFilterId): void {
    onFilter(filter === id ? null : id);
  }
</script>

<header
  class="overview-top flex flex-none flex-col gap-3 border-b border-border bg-card/75 backdrop-blur-sm px-4 py-3"
>
  <!-- Top row: Heading, timestamp, and actions -->
  <div class="flex flex-wrap items-center justify-between gap-3">
    <div class="flex items-center gap-2.5">
      <div class="flex items-center gap-2">
        <h1 class="text-sm font-semibold tracking-tight text-foreground">
          Repository Overview
        </h1>
        {#if refreshedLabel}
          <span class="inline-flex items-center rounded-full bg-muted/60 px-2 py-0.5 text-ui-micro font-medium text-muted-foreground">
            {refreshedLabel}
          </span>
        {/if}
      </div>
    </div>

    <!-- Actions toolbar -->
    <div class="flex flex-none items-center gap-1.5">
      {#if showChatToggle}
        <Hint label={chatRailOpen ? "Hide the chat rail" : "Show the chat rail"}>
          {#snippet child({ props })}
            <Button
              {...props}
              variant="ghost"
              size="icon-sm"
              type="button"
              class="h-7 w-7 text-muted-foreground hover:text-foreground"
              aria-label={chatRailOpen ? "Hide the chat rail" : "Show the chat rail"}
              aria-pressed={chatRailOpen}
              onclick={onToggleChatRail}
            >
              <Icon icon={iconRail} size={15} />
            </Button>
          {/snippet}
        </Hint>
      {/if}
      <Hint label="Add a repository to the library without opening a tab">
        {#snippet child({ props })}
          <Button
            {...props}
            variant="outline"
            size="sm"
            type="button"
            class="h-7 gap-1.5 px-2.5 text-ui-meta font-medium shadow-xs"
            onclick={onAddFolder}
          >
            <Icon icon={iconAdd} size={13} />
            <span>Add folder…</span>
          </Button>
        {/snippet}
      </Hint>
      <Hint label="Rescan every repository that is not open as a tab">
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon-sm"
            type="button"
            class="h-7 w-7 text-muted-foreground hover:text-foreground"
            aria-label="Refresh the Overview"
            disabled={refreshing}
            onclick={onRefresh}
          >
            <span class={refreshing ? "inline-flex animate-spin" : "inline-flex"}>
              <Icon icon={iconRefresh} size={14} />
            </span>
          </Button>
        {/snippet}
      </Hint>
    </div>
  </div>

  <!-- KPI summary cards -->
  <div
    class="kpi-grid grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-5"
    role="region"
    aria-label="Repository summary metrics"
  >
    <!-- 1. Total Repositories -->
    <div
      class="kpi-card flex flex-col justify-between rounded-lg border border-border/80 bg-background/50 p-2.5 transition-all hover:bg-background/80"
    >
      <div class="flex items-center justify-between text-muted-foreground">
        <span class="text-ui-micro font-medium uppercase tracking-wider text-muted-foreground">Total Repos</span>
        <span class="text-muted-foreground/70"><Icon icon={iconRepos} size={14} /></span>
      </div>
      <div class="mt-1 flex items-baseline gap-1.5">
        <span class="text-xl font-bold tracking-tight text-foreground tabular-nums">{totals.repos}</span>
        <span class="text-ui-micro text-muted-foreground">{totals.repos === 1 ? "repo" : "repos"}</span>
      </div>
      <span class="mt-0.5 text-ui-micro text-muted-foreground/80 truncate">in library</span>
    </div>

    <!-- 2. Attention Needed -->
    <button
      type="button"
      class="kpi-card text-left cursor-pointer flex flex-col justify-between rounded-lg border p-2.5 transition-all {totals.attentionNeeded > 0
        ? 'border-amber-500/30 bg-amber-500/[0.04] hover:bg-amber-500/[0.08]'
        : 'border-border/80 bg-background/50 hover:bg-background/80'}"
      onclick={() => toggleFilter("dirty")}
      title="Click to filter repos requiring attention"
    >
      <div class="flex items-center justify-between {totals.attentionNeeded > 0 ? 'text-amber-500 dark:text-amber-400' : 'text-muted-foreground'}">
        <span class="text-ui-micro font-medium uppercase tracking-wider">Attention</span>
        <Icon icon={iconAttention} size={14} />
      </div>
      <div class="mt-1 flex items-baseline gap-1.5">
        <span class="text-xl font-bold tracking-tight tabular-nums {totals.attentionNeeded > 0 ? 'text-amber-600 dark:text-amber-400' : 'text-foreground'}">
          {totals.attentionNeeded}
        </span>
        <span class="text-ui-micro {totals.attentionNeeded > 0 ? 'text-amber-600/80 dark:text-amber-400/80' : 'text-muted-foreground'}">
          {totals.attentionNeeded === 1 ? "repo" : "repos"}
        </span>
      </div>
      <span class="mt-0.5 text-ui-micro truncate {totals.attentionNeeded > 0 ? 'text-amber-600/80 dark:text-amber-400/80' : 'text-muted-foreground/80'}">
        {#if totals.dirty > 0}
          {totals.dirty} dirty worktree
        {:else if totals.attentionNeeded > 0}
          conflicts or sync debt
        {:else}
          all clean
        {/if}
      </span>
    </button>

    <!-- 3. Out of Sync -->
    <button
      type="button"
      class="kpi-card text-left cursor-pointer flex flex-col justify-between rounded-lg border p-2.5 transition-all {totals.outOfSync > 0
        ? 'border-sky-500/30 bg-sky-500/[0.04] hover:bg-sky-500/[0.08]'
        : 'border-border/80 bg-background/50 hover:bg-background/80'}"
      onclick={() => toggleFilter("needs-push")}
      title="Click to filter out-of-sync repos"
    >
      <div class="flex items-center justify-between {totals.outOfSync > 0 ? 'text-sky-500 dark:text-sky-400' : 'text-muted-foreground'}">
        <span class="text-ui-micro font-medium uppercase tracking-wider">Out of Sync</span>
        <Icon icon={iconSync} size={14} />
      </div>
      <div class="mt-1 flex items-baseline gap-1.5">
        <span class="text-xl font-bold tracking-tight tabular-nums {totals.outOfSync > 0 ? 'text-sky-600 dark:text-sky-400' : 'text-foreground'}">
          {totals.outOfSync}
        </span>
        <span class="text-ui-micro {totals.outOfSync > 0 ? 'text-sky-600/80 dark:text-sky-400/80' : 'text-muted-foreground'}">
          {totals.outOfSync === 1 ? "repo" : "repos"}
        </span>
      </div>
      <span class="mt-0.5 text-ui-micro truncate font-mono {totals.outOfSync > 0 ? 'text-sky-600/80 dark:text-sky-400/80' : 'text-muted-foreground/80'}">
        ↑{totals.ahead} push · ↓{totals.behind} pull
      </span>
    </button>

    <!-- 4. Open PRs & Checks -->
    <button
      type="button"
      class="kpi-card text-left cursor-pointer flex flex-col justify-between rounded-lg border p-2.5 transition-all {totals.prs > 0
        ? 'border-purple-500/30 bg-purple-500/[0.04] hover:bg-purple-500/[0.08]'
        : 'border-border/80 bg-background/50 hover:bg-background/80'}"
      onclick={() => toggleFilter("has-prs")}
      title="Click to filter repos with open PRs"
    >
      <div class="flex items-center justify-between {totals.prs > 0 ? 'text-purple-500 dark:text-purple-400' : 'text-muted-foreground'}">
        <span class="text-ui-micro font-medium uppercase tracking-wider">Open PRs</span>
        <Icon icon={iconPr} size={14} />
      </div>
      <div class="mt-1 flex items-baseline gap-1.5">
        <span class="text-xl font-bold tracking-tight tabular-nums {totals.prs > 0 ? 'text-purple-600 dark:text-purple-400' : 'text-foreground'}">
          {totals.prs}
        </span>
        <span class="text-ui-micro {totals.prs > 0 ? 'text-purple-600/80 dark:text-purple-400/80' : 'text-muted-foreground'}">
          {totals.prs === 1 ? "PR" : "PRs"}
        </span>
      </div>
      <span class="mt-0.5 text-ui-micro truncate {totals.prs > 0 ? 'text-purple-600/80 dark:text-purple-400/80' : 'text-muted-foreground/80'}">
        {totals.prs > 0 ? "active review" : "none pending"}
      </span>
    </button>

    <!-- 5. In Sync & Clean -->
    <div
      class="kpi-card flex flex-col justify-between rounded-lg border p-2.5 transition-all {totals.inSync > 0
        ? 'border-emerald-500/25 bg-emerald-500/[0.03] hover:bg-emerald-500/[0.06]'
        : 'border-border/80 bg-background/50 hover:bg-background/80'}"
    >
      <div class="flex items-center justify-between {totals.inSync > 0 ? 'text-emerald-500 dark:text-emerald-400' : 'text-muted-foreground'}">
        <span class="text-ui-micro font-medium uppercase tracking-wider">In Sync</span>
        <Icon icon={iconClean} size={14} />
      </div>
      <div class="mt-1 flex items-baseline gap-1.5">
        <span class="text-xl font-bold tracking-tight tabular-nums {totals.inSync > 0 ? 'text-emerald-600 dark:text-emerald-400' : 'text-foreground'}">
          {totals.inSync}
        </span>
        <span class="text-ui-micro {totals.inSync > 0 ? 'text-emerald-600/80 dark:text-emerald-400/80' : 'text-muted-foreground'}">
          {totals.inSync === 1 ? "repo" : "repos"}
        </span>
      </div>
      <span class="mt-0.5 text-ui-micro truncate {totals.inSync > 0 ? 'text-emerald-600/80 dark:text-emerald-400/80' : 'text-muted-foreground/80'}">
        clean & up to date
      </span>
    </div>
  </div>

  <!-- Filter chips bar -->
  <div class="flex flex-wrap items-center gap-2 pt-0.5">
    <span class="text-ui-micro font-semibold uppercase tracking-wider text-muted-foreground/80">
      Filter
    </span>
    <div
      class="flex flex-wrap items-center gap-1.5"
      role="group"
      aria-label="Filter repositories"
    >
      <button
        class="filter-chip cursor-pointer rounded-full border px-2.5 py-0.5 text-ui-micro font-medium transition-colors {filter === null
          ? 'border-primary/60 bg-primary/10 text-primary-ink font-semibold'
          : 'border-border/80 text-muted-foreground hover:bg-[color:var(--app-hover)] hover:text-foreground'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        type="button"
        aria-pressed={filter === null}
        onclick={() => onFilter(null)}
      >
        All ({totals.repos})
      </button>
      {#each OVERVIEW_FILTERS as preset (preset.id)}
        <button
          class="filter-chip cursor-pointer rounded-full border px-2.5 py-0.5 text-ui-micro font-medium transition-colors {filter === preset.id
            ? 'border-primary/60 bg-primary/10 text-primary-ink font-semibold shadow-xs'
            : 'border-border/80 text-muted-foreground hover:bg-[color:var(--app-hover)] hover:text-foreground'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
          type="button"
          aria-pressed={filter === preset.id}
          onclick={() => toggleFilter(preset.id)}
        >
          {preset.label}
        </button>
      {/each}
    </div>
  </div>

  <!-- Forge banner if gh missing / unauthed -->
  {#if banner.kind === "missing" || banner.kind === "unauthed"}
    <div
      class="overview-gh-banner m-0 flex items-center gap-2 rounded-lg border border-dashed border-border/80 bg-muted/30 px-3 py-1.5 text-ui-micro text-muted-foreground"
      role="note"
    >
      <span class="text-muted-foreground flex-none"><Icon icon={iconInfo} size={13} /></span>
      <span>
        {banner.text}
        GitHub columns show cached data only — open or expand a repository for live GitHub data.
      </span>
    </div>
  {/if}
</header>
