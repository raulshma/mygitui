<script lang="ts">
  import { untrack } from "svelte";
  import type { Issue, IssueQuery } from "$lib/issues/model";
  import { issueQueryKey, normalizeIssueQuery } from "$lib/issues/query";
  import { issuesStore } from "$lib/stores/issues.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { describeError } from "$lib/errors";

  let {
    repoId,
    query,
    selectedNumber = null,
    onQueryChange,
    onSelect,
  }: {
    repoId: string;
    query: IssueQuery;
    selectedNumber?: number | null;
    onQueryChange: (patch: Partial<IssueQuery>) => void;
    onSelect: (issueNumber: number) => void;
  } = $props();

  const iconSearch = $derived(appIcon("search"));
  const iconRefresh = $derived(appIcon("refresh"));
  let listEl = $state<HTMLDivElement | null>(null);
  let scrollerEl = $state<HTMLDivElement | null>(null);
  let searchEl = $state<HTMLInputElement | null>(null);
  let searchDraft = $state(untrack(() => query.search ?? ""));
  let draftSearch = $state(untrack(() => query.search ?? ""));
  let draftRepoId = $state(untrack(() => repoId));
  let page = $state(1);
  let loadedQueryKey = $state("");
  let activeNumber = $state<number | null>(untrack(() => selectedNumber));

  const identityEntry = $derived(issuesStore.identity(repoId));
  const listEntry = $derived(issuesStore.list(repoId, query, page));
  const metadataEntry = $derived(issuesStore.metadata(repoId));
  const metadata = $derived(
    metadataEntry.state === "ready"
      ? metadataEntry.data
      : metadataEntry.state === "loading" || metadataEntry.state === "error"
        ? metadataEntry.previous
        : null,
  );
  const data = $derived(
    listEntry.state === "ready"
      ? listEntry.data
      : listEntry.state === "loading" || listEntry.state === "error"
        ? listEntry.previous
        : null,
  );
  const items = $derived(data?.items ?? []);
  const loading = $derived(listEntry.state === "loading" && data === null);
  const refreshing = $derived(listEntry.state === "loading" && data !== null);
  const listError = $derived(
    listEntry.state === "error"
      ? listEntry.message
      : identityEntry.state === "error"
        ? identityEntry.message
        : null,
  );
  const activeIndex = $derived(items.findIndex((issue) => issue.number === activeNumber));
  const optionId = (issue: Issue): string =>
    `issue-option-${encodeURIComponent(repoId)}-${encodeURIComponent(issue.id)}`;
  const activeDescendant = $derived(
    activeIndex >= 0 ? optionId(items[activeIndex]!) : undefined,
  );
  const activeFilterCount = $derived(
    query.labels.length + query.assignees.length + (query.milestone === null ? 0 : 1),
  );

  $effect(() => {
    const currentSearch = query.search ?? "";
    if (draftRepoId !== repoId || draftSearch !== currentSearch) {
      draftRepoId = repoId;
      draftSearch = currentSearch;
      searchDraft = currentSearch;
      activeNumber = selectedNumber;
      scrollerEl?.scrollTo({ top: 0 });
    }
  });

  $effect(() => {
    const targetRepo = repoId;
    const currentQuery = query;
    const key = issueQueryKey(currentQuery);
    if (loadedQueryKey !== key) {
      loadedQueryKey = key;
      page = 1;
    }
    const requestedPage = page;
    untrack(() => void issuesStore.loadList(targetRepo, currentQuery, requestedPage));
  });

  $effect(() => {
    const targetRepo = repoId;
    untrack(() => void issuesStore.loadMetadata(targetRepo));
  });

  // Preserve a valid selection across refreshes and recover gracefully when a
  // state/filter mutation removes the selected row from the current page.
  $effect(() => {
    if (items.length === 0) return;
    if (activeNumber !== null && items.some((issue) => issue.number === activeNumber)) return;
    activeNumber = selectedNumber !== null && items.some((issue) => issue.number === selectedNumber)
      ? selectedNumber
      : items[0]!.number;
  });

  function patchQuery(patch: Partial<IssueQuery>): void {
    if (patch.search === undefined && patch.state === undefined && patch.sort === undefined &&
      patch.direction === undefined && patch.labels === undefined && patch.assignees === undefined &&
      patch.milestone === undefined) return;
    page = 1;
    scrollerEl?.scrollTo({ top: 0 });
    onQueryChange(patch);
  }

  function submitSearch(): void {
    const normalized = normalizeIssueQuery({ ...query, search: searchDraft });
    searchDraft = normalized.search ?? "";
    patchQuery({ search: normalized.search });
  }

  function clearSearch(): void {
    searchDraft = "";
    patchQuery({ search: null });
  }

  function toggleValue(field: "labels" | "assignees", value: string, checked: boolean): void {
    const values = query[field];
    const next = checked
      ? [...new Set([...values, value])]
      : values.filter((entry) => entry !== value);
    patchQuery({ [field]: next });
  }

  function moveActive(index: number): void {
    const issue = items[index];
    if (issue === undefined) return;
    activeNumber = issue.number;
    requestAnimationFrame(() => {
      document.getElementById(optionId(issue))?.scrollIntoView({ block: "nearest" });
    });
  }

  function onKeydown(event: KeyboardEvent): void {
    const current = activeIndex >= 0 ? activeIndex : 0;
    if (event.key === "ArrowDown" || event.key === "j") {
      event.preventDefault();
      moveActive(Math.min(items.length - 1, current + 1));
    } else if (event.key === "ArrowUp" || event.key === "k") {
      event.preventDefault();
      moveActive(Math.max(0, current - 1));
    } else if (event.key === "Home" || event.key === "g") {
      event.preventDefault();
      moveActive(0);
    } else if (event.key === "End" || event.key === "G") {
      event.preventDefault();
      moveActive(items.length - 1);
    } else if (event.key === "Enter" || event.key === " ") {
      const issue = items[current];
      if (issue !== undefined) {
        event.preventDefault();
        onSelect(issue.number);
      }
    }
  }

  function turnPage(next: number): void {
    if (next < 1 || (data !== null && next > data.page && !data.has_next)) return;
    page = next;
    scrollerEl?.scrollTo({ top: 0 });
  }

  export function focusSearch(): void {
    searchEl?.focus();
    searchEl?.select();
  }

  export function focus(): void {
    listEl?.focus();
  }

  export async function refresh(): Promise<void> {
    await issuesStore.loadList(repoId, query, page, { force: true });
  }

  function labelColor(value: string | null): string | undefined {
    return value !== null && /^#[0-9a-f]{6}$/i.test(value) ? value : undefined;
  }

  function dateLabel(value: string): string {
    const date = new Date(value);
    return Number.isFinite(date.getTime())
      ? date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })
      : value;
  }
</script>

<div class="issue-list flex min-h-0 min-w-0 flex-1 flex-col bg-card" aria-busy={loading || refreshing}>
  <div class="flex flex-none flex-wrap items-center gap-1 border-b border-border px-2 py-1.5">
    <form
      class="flex min-w-32 flex-1 items-center gap-1"
      role="search"
      onsubmit={(event) => {
        event.preventDefault();
        submitSearch();
      }}
    >
      <span class="relative min-w-0 flex-1">
        <span class="pointer-events-none absolute left-2 top-1/2 inline-flex -translate-y-1/2 text-muted-foreground" aria-hidden="true">
          {#if iconSearch}<Icon icon={iconSearch} size={13} />{/if}
        </span>
        <input
          bind:this={searchEl}
          class="h-7 w-full min-w-0 rounded-sm border border-border bg-background pl-6 pr-2 text-ui-meta text-foreground outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-1 focus-visible:ring-ring"
          type="text"
          value={searchDraft}
          placeholder="Search issues"
          aria-label="Search issues"
          oninput={(event) => (searchDraft = event.currentTarget.value)}
          onkeydown={(event) => {
            if (event.key === "Escape" && searchDraft !== "") {
              event.preventDefault();
              event.stopPropagation();
              clearSearch();
            }
          }}
        />
      </span>
      <Button class="h-7 rounded-sm px-2 text-ui-meta" variant="outline" type="submit">Search</Button>
      <Button
        class="h-7 rounded-sm px-2 text-ui-meta"
        variant="ghost"
        type="button"
        disabled={searchDraft === "" && query.search === null}
        onclick={clearSearch}
      >Clear</Button>
    </form>
    <select
      class="h-7 rounded-sm border border-border bg-background px-1 text-ui-meta text-foreground"
      aria-label="Issue state"
      value={query.state}
      onchange={(event) => patchQuery({ state: event.currentTarget.value as IssueQuery["state"] })}
    >
      <option value="open">Open</option>
      <option value="closed">Closed</option>
      <option value="all">All states</option>
    </select>
    <select
      class="h-7 rounded-sm border border-border bg-background px-1 text-ui-meta text-foreground"
      aria-label="Sort issues"
      value={query.sort}
      onchange={(event) => patchQuery({ sort: event.currentTarget.value as IssueQuery["sort"] })}
    >
      <option value="updated">Updated</option>
      <option value="created">Created</option>
      <option value="comments">Comments</option>
    </select>
    <select
      class="h-7 rounded-sm border border-border bg-background px-1 text-ui-meta text-foreground"
      aria-label="Sort direction"
      value={query.direction}
      onchange={(event) => patchQuery({ direction: event.currentTarget.value as IssueQuery["direction"] })}
    >
      <option value="desc">Descending</option>
      <option value="asc">Ascending</option>
    </select>
    <details class="relative">
      <summary class="inline-flex h-7 cursor-pointer list-none items-center rounded-sm border border-border px-2 text-ui-meta text-muted-foreground hover:bg-accent [&::-webkit-details-marker]:hidden">
        Filters{activeFilterCount > 0 ? ` (${activeFilterCount})` : ""}
      </summary>
      <div class="absolute right-0 top-8 z-30 grid w-64 max-w-[calc(100vw-2rem)] gap-2 rounded-md border border-border bg-popover p-2 text-ui-meta shadow-lg">
        <fieldset class="min-w-0">
          <legend class="mb-1 font-medium text-foreground">Labels</legend>
          <div class="max-h-32 overflow-y-auto">
            {#if metadata !== null && metadata.labels.length > 0}
              {#each metadata.labels as label (label.id)}
                <label class="flex cursor-pointer items-center gap-2 py-0.5">
                  <Checkbox
                    checked={query.labels.includes(label.name)}
                    onCheckedChange={(checked) => toggleValue("labels", label.name, checked)}
                    aria-label={label.name}
                  />
                  <span
                    class="size-2 shrink-0 rounded-full border border-border"
                    style:background={labelColor(label.color)}
                    aria-hidden="true"
                  ></span>
                  <span class="min-w-0 truncate">{label.name}</span>
                </label>
              {/each}
            {:else}
              <p class="m-0 text-muted-foreground">No label metadata available.</p>
            {/if}
          </div>
        </fieldset>
        <fieldset class="min-w-0">
          <legend class="mb-1 font-medium text-foreground">Assignees</legend>
          <div class="max-h-32 overflow-y-auto">
            {#if metadata !== null && metadata.assignees.length > 0}
              {#each metadata.assignees as assignee (assignee.id)}
                <label class="flex cursor-pointer items-center gap-2 py-0.5">
                  <Checkbox
                    checked={query.assignees.includes(assignee.login)}
                    onCheckedChange={(checked) => toggleValue("assignees", assignee.login, checked)}
                    aria-label={assignee.login}
                  />
                  <span class="min-w-0 truncate">{assignee.login}</span>
                </label>
              {/each}
            {:else}
              <p class="m-0 text-muted-foreground">No assignee metadata available.</p>
            {/if}
          </div>
        </fieldset>
        <label class="grid gap-1">
          <span class="font-medium text-foreground">Milestone</span>
          <select
            class="h-7 min-w-0 rounded-sm border border-border bg-background px-1 text-foreground"
            aria-label="Milestone"
            value={query.milestone ?? ""}
            onchange={(event) => patchQuery({ milestone: event.currentTarget.value || null })}
          >
            <option value="">Any milestone</option>
            {#each metadata?.milestones ?? [] as milestone (milestone.number)}
              <option value={String(milestone.number)}>{milestone.title}</option>
            {/each}
          </select>
        </label>
        {#if activeFilterCount > 0}
          <Button
            class="h-7 rounded-sm text-ui-meta"
            variant="ghost"
            type="button"
            onclick={() => patchQuery({ labels: [], assignees: [], milestone: null })}
          >Clear filters</Button>
        {/if}
      </div>
    </details>
    <Button
      class="h-7 w-7 flex-none rounded-sm"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Refresh issue list"
      title="Refresh issue list"
      disabled={refreshing}
      onclick={() => void refresh()}
    >
      {#if iconRefresh}<Icon icon={iconRefresh} size={14} />{/if}
    </Button>
  </div>

  {#if refreshing}
    <div class="flex flex-none items-center justify-center gap-1.5 border-b border-border/60 bg-muted/40 px-2 py-1 text-ui-micro text-muted-foreground" role="status">
      Refreshing issues…
    </div>
  {/if}
  {#if listError !== null && data !== null}
    <div class="flex flex-none flex-wrap items-center gap-2 border-b border-border/60 bg-muted/40 px-2 py-1 text-ui-micro text-destructive" role="alert">
      <span class="min-w-0 flex-1">Showing saved results. {listError}</span>
      <Button class="h-6 rounded-sm text-ui-micro" variant="outline" type="button" onclick={() => void refresh()}>Retry</Button>
    </div>
  {/if}

  <div
    bind:this={scrollerEl}
    class="min-h-0 min-w-0 flex-1 overflow-y-auto [scrollbar-width:thin]"
  >
    <div
      bind:this={listEl}
      class="min-w-0 outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
      role="listbox"
      aria-label="Issues"
      aria-activedescendant={activeDescendant}
      tabindex={0}
      onkeydown={onKeydown}
    >
      {#each items as issue (issue.id)}
        <div
          id={optionId(issue)}
          class="group flex cursor-pointer items-start gap-2 border-b border-border/45 px-2.5 py-2 text-ui-meta hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
          class:bg-secondary={issue.number === selectedNumber}
          class:text-secondary-foreground={issue.number === selectedNumber}
          role="option"
          aria-selected={issue.number === selectedNumber}
          tabindex={-1}
          onclick={() => onSelect(issue.number)}
          onkeydown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              onSelect(issue.number);
            }
          }}
        >
          <span
            class="mt-0.5 inline-flex size-3.5 flex-none items-center justify-center rounded-full border"
            class:border-[var(--app-added)]={issue.state === "open"}
            class:border-muted-foreground={issue.state === "closed"}
            class:text-[var(--app-added)]={issue.state === "open"}
            aria-label={issue.state}
          >
            {#if issue.state === "closed"}<span class="size-1.5 rounded-full bg-current"></span>{/if}
          </span>
          <span class="min-w-0 flex-1">
            <span class="flex min-w-0 items-baseline gap-1.5">
              <span class="min-w-0 flex-1 truncate font-medium text-foreground">{issue.title}</span>
              <span class="flex-none font-mono text-ui-micro text-muted-foreground">#{issue.number}</span>
            </span>
            {#if issue.labels.length > 0}
              <span class="mt-1 flex flex-wrap gap-1">
                {#each issue.labels as label (label.id)}
                  <span
                    class="max-w-40 truncate rounded-full border border-border px-1.5 text-ui-micro text-muted-foreground"
                    style:border-color={labelColor(label.color)}
                    title={label.description ?? label.name}
                  >{label.name}</span>
                {/each}
              </span>
            {/if}
            <span class="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-ui-micro text-muted-foreground">
              <span>by {issue.author.login || "ghost"}</span>
              <span>{issue.comments_count} comment{issue.comments_count === 1 ? "" : "s"}</span>
              <span>updated {dateLabel(issue.updated_at)}</span>
              {#if issue.assignees.length > 0}
                <span class="truncate">assigned to {issue.assignees.map((entry) => entry.login).join(", ")}</span>
              {/if}
              {#if issue.milestone !== null}<span class="truncate">{issue.milestone.title}</span>{/if}
            </span>
          </span>
        </div>
      {/each}
    </div>

    {#if loading}
      <div class="grid gap-2 p-2" role="status" aria-label="Loading issues">
        {#each Array.from({ length: 8 }, (_, index) => index) as index (index)}
          <div class="animate-pulse rounded-sm border border-border/50 p-2">
            <div class="h-3 w-4/5 rounded bg-muted"></div>
            <div class="mt-2 h-2 w-1/2 rounded bg-muted"></div>
          </div>
        {/each}
      </div>
    {:else if listError !== null && data === null}
      <div class="flex min-h-48 flex-col items-center justify-center gap-2 p-4 text-center text-ui-meta" role="alert">
        <p class="m-0 font-medium text-foreground">Couldn't load issues</p>
        <p class="m-0 max-w-sm break-words text-muted-foreground">{listError || describeError(listError)}</p>
        <Button class="h-7 rounded-sm" variant="outline" type="button" onclick={() => void refresh()}>Retry</Button>
      </div>
    {:else if !loading && items.length === 0}
      <div class="flex min-h-48 flex-col items-center justify-center gap-1 p-4 text-center text-muted-foreground">
        <p class="m-0 text-ui-base font-medium text-foreground">No issues found</p>
        <p class="m-0 max-w-sm text-ui-meta">Try another search or clear the filters.</p>
      </div>
    {/if}
  </div>

  <footer class="flex flex-none items-center justify-between gap-2 border-t border-border px-2 py-1 text-ui-micro text-muted-foreground">
    <span>
      {#if data?.total_count !== null && data?.total_count !== undefined}{data.total_count} issue{data.total_count === 1 ? "" : "s"}{:else}Page {data?.page ?? page}{/if}
      {#if data?.incomplete_results}<span class="text-destructive"> · partial results</span>{/if}
    </span>
    <span class="inline-flex items-center gap-1">
      <Button
        class="h-6 rounded-sm px-2 text-ui-micro"
        variant="outline"
        type="button"
        disabled={data === null || !data.has_previous || page <= 1}
        onclick={() => turnPage(page - 1)}
      >Previous</Button>
      <span class="min-w-12 text-center tabular-nums">Page {data?.page ?? page}</span>
      <Button
        class="h-6 rounded-sm px-2 text-ui-micro"
        variant="outline"
        type="button"
        disabled={data === null || !data.has_next}
        onclick={() => turnPage(page + 1)}
      >Next</Button>
    </span>
  </footer>
</div>
