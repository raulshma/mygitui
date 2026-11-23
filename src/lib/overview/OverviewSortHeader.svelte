<script lang="ts">
  /**
   * The Overview list's sort header: clickable column controls for name,
   * last commit, ahead, behind, changes, PRs — plus the default Attention
   * order first. The active column shows a direction arrow; re-clicking the
   * same column flips the direction. The choice persists via the overview
   * store.
   */
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import {
    OVERVIEW_SORTS,
    type OverviewSortId,
    type SortDirection,
  } from "./overviewModel";

  let {
    sort,
    sortDir,
    onSort,
  }: {
    sort: OverviewSortId;
    sortDir: SortDirection;
    onSort: (sort: OverviewSortId) => void;
  } = $props();

  const iconUp = $derived(appIcon("chevron-up")!);
  const iconDown = $derived(appIcon("chevron-down")!);

  const columns = $derived([
    { id: "attention" as OverviewSortId, label: "Attention" },
    ...OVERVIEW_SORTS,
  ]);
</script>

<div
  class="sort-head flex min-h-7 flex-none items-center gap-1.5 px-3 py-1 text-ui-micro select-none border-b border-border/40 bg-muted/20"
  role="row"
  aria-label="Sort repositories"
>
  <span class="mr-1 text-ui-micro font-medium uppercase tracking-wider text-muted-foreground/70">
    Sort:
  </span>
  <div class="flex flex-wrap items-center gap-1">
    {#each columns as column (column.id)}
      <button
        class="sort-col inline-flex cursor-pointer items-center gap-1 rounded-md px-2 py-0.5 text-ui-micro transition-colors {sort === column.id
          ? 'bg-primary/10 font-semibold text-primary-ink border border-primary/25'
          : 'text-muted-foreground hover:bg-[color:var(--app-hover)] hover:text-foreground'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        type="button"
        aria-label={`Sort by ${column.label}`}
        aria-pressed={sort === column.id}
        onclick={() => onSort(column.id)}
      >
        <span>{column.label}</span>
        {#if sort === column.id}
          <span class="inline-flex text-primary-ink" aria-hidden="true">
            <Icon icon={sortDir === "asc" ? iconUp : iconDown} size={11} />
          </span>
        {/if}
      </button>
    {/each}
  </div>
</div>
