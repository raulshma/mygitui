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
  class="sort-head flex h-6 flex-none items-center gap-3 px-3 text-ui-micro text-muted-foreground select-none"
  role="row"
  aria-label="Sort repositories"
>
  {#each columns as column (column.id)}
    <button
      class="sort-col flex cursor-pointer items-center gap-0.5 rounded-sm border-0 bg-transparent px-1 py-0 text-ui-micro {sort === column.id
        ? 'font-semibold text-foreground'
        : 'hover:bg-[color:var(--app-hover)] hover:text-foreground'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
      type="button"
      aria-label={`Sort by ${column.label}`}
      aria-pressed={sort === column.id}
      onclick={() => onSort(column.id)}
    >
      {column.label}
      {#if sort === column.id}
        <span class="inline-flex" aria-hidden="true">
          <Icon icon={sortDir === "asc" ? iconUp : iconDown} size={10} />
        </span>
      {/if}
    </button>
  {/each}
</div>
