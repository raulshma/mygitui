<script lang="ts">
  /**
   * Overview summary strip: the totals (repos, dirty, ahead, behind, open
   * PRs), the filter presets, the Add-folder / Refresh actions, the chat-rail
   * toggle and — shown ONCE for the whole view — the gh availability banner
   * (the per-row cells degrade to em-dashes; no per-row banners).
   */
  import { Badge } from "$lib/components/ui/badge/index.js";
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

  function toggleFilter(id: OverviewFilterId): void {
    onFilter(filter === id ? null : id);
  }
</script>

<header
  class="overview-top flex flex-none flex-col gap-2 border-b border-border bg-card px-3 py-2"
>
  <div class="flex flex-wrap items-center gap-2">
    <span class="min-w-0 flex-1 text-sm font-semibold tracking-[-0.01em]">
      Overview
      {#if refreshedLabel}
        <span class="ml-1.5 font-normal text-ui-micro text-muted-foreground">
          {refreshedLabel}
        </span>
      {/if}
    </span>
    <span class="flex flex-none items-center gap-1">
      {#if showChatToggle}
        <Hint label={chatRailOpen ? "Hide the chat rail" : "Show the chat rail"}>
          {#snippet child({ props })}
            <Button
              {...props}
              variant="ghost"
              size="icon-sm"
              type="button"
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
            variant="ghost"
            size="sm"
            type="button"
            class="gap-1 px-2 text-ui-meta"
            onclick={onAddFolder}
          >
            <Icon icon={iconAdd} size={14} />
            Add folder…
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
            aria-label="Refresh the Overview"
            disabled={refreshing}
            onclick={onRefresh}
          >
            <span class={refreshing ? "inline-flex animate-spin" : "inline-flex"}>
              <Icon icon={iconRefresh} size={15} />
            </span>
          </Button>
        {/snippet}
      </Hint>
    </span>
  </div>

  <div class="flex flex-wrap items-center gap-1.5">
    <span class="flex flex-wrap items-center gap-1" aria-label="Repository totals">
      <Badge variant="secondary" class="gap-1 rounded-full px-2 py-px text-ui-micro tabular-nums">
        {totals.repos}
        {totals.repos === 1 ? "repo" : "repos"}
      </Badge>
      <Badge variant="secondary" class="gap-1 rounded-full px-2 py-px text-ui-micro tabular-nums">
        {totals.dirty} dirty
      </Badge>
      <Badge variant="secondary" class="gap-1 rounded-full px-2 py-px text-ui-micro tabular-nums">
        ↑{totals.ahead}
      </Badge>
      <Badge variant="secondary" class="gap-1 rounded-full px-2 py-px text-ui-micro tabular-nums">
        ↓{totals.behind}
      </Badge>
      <Badge variant="secondary" class="gap-1 rounded-full px-2 py-px text-ui-micro tabular-nums">
        {totals.prs} PRs
      </Badge>
    </span>
    <span class="mx-1 h-4 w-px flex-none bg-border" aria-hidden="true"></span>
    <span
      class="flex flex-wrap items-center gap-1"
      role="group"
      aria-label="Filter repositories"
    >
      {#each OVERVIEW_FILTERS as preset (preset.id)}
        <button
          class="filter-chip cursor-pointer rounded-full border px-2 py-px text-ui-micro transition-colors {filter === preset.id
            ? 'border-primary/60 bg-primary/10 font-medium text-foreground'
            : 'border-border text-muted-foreground hover:bg-[color:var(--app-hover)] hover:text-foreground'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
          type="button"
          aria-pressed={filter === preset.id}
          onclick={() => toggleFilter(preset.id)}
        >
          {preset.label}
        </button>
      {/each}
    </span>
  </div>

  {#if banner.kind === "missing" || banner.kind === "unauthed"}
    <p
      class="overview-gh-banner m-0 flex items-center gap-1.5 rounded-md border border-dashed border-border px-2 py-1 text-ui-micro text-muted-foreground"
      role="note"
    >
      <Icon icon={appIcon("info")!} size={12} />
      {banner.text}
      GitHub columns show cached data only — open or expand a repository for
      live GitHub data.
    </p>
  {/if}
</header>
