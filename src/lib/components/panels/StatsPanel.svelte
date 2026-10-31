<script lang="ts">
  /**
   * StatsPanel (M7 lane I1) — contribution statistics for one repo.
   *
   * Layout, top to bottom:
   *   1. header: author filter (contributor datalist → re-fetch with the
   *      author substring), a refresh button and the "Colors" popover
   *      toggle (per-repo branch color rules — this panel hosts the editor,
   *      see docs/contracts.md M7),
   *   2. summary cards: commits, active days, avg/active-day, current and
   *      longest streak,
   *   3. the contribution heatmap: a GitHub-style 53-week grid (weeks as
   *      columns, Sun..Sat rows) rendered as a real <table> for screen
   *      readers, one aria-label per cell ("N commits on DATE"), month
   *      labels along the top, intensity via primary color-mix
   *      buckets, horizontally scrollable when narrow,
   *   4. the contributor list: name, commit-count bar and active range.
   *
   * Data comes from the `statsStore` cache (`commit_activity` /
   * `contributor_stats` IPC reads); branch color rules live in
   * `branchColorStore` (localStorage per repo root). Outside a Tauri
   * webview the panel renders an explanatory note (documented non-Tauri
   * behavior — the IPC layer rejects one-shot commands).
   */
  import { isTauri } from "$lib/entry/dragdrop";
  import { floating } from "$lib/components/overlay/floating";
  import { Button } from "$lib/components/ui/button/index.js";
  import {
    branchColorStore,
    type BranchColorRule,
  } from "$lib/stats/branchColors.svelte";
  import { buildHeatmap, streaks, todayISO, totals } from "$lib/stats/heatmapModel";
  import { statsStore } from "$lib/stats/statsStore.svelte";

  let { repoId, root }: { repoId: string; root: string } = $props();

  const supported = isTauri();

  /** Author filter input (committed on change/Enter, cleared explicitly). */
  let authorInput = $state("");
  /** Colors popover open? */
  let colorsOpen = $state(false);
  /** Anchor for the (portaled) Colors popover. */
  let colorsButton = $state<HTMLButtonElement | null>(null);
  /** New-rule draft (Colors popover). */
  let newPattern = $state("");
  let newColor = $state("");

  // Hydrate the branch-color rules OUTSIDE derived reads (store contract),
  // and (re)load stats whenever the repo changes (never per author keystroke
  // — the filter re-fetches explicitly onchange).
  $effect(() => {
    branchColorStore.ensure(root);
  });
  $effect(() => {
    void repoId;
    authorInput = "";
    if (supported) void statsStore.load(repoId);
  });

  const entry = $derived(statsStore.entry(repoId));
  const activity = $derived(statsStore.activity(repoId));
  const contributors = $derived(statsStore.contributors(repoId));
  const error = $derived(statsStore.error(repoId));
  const loading = $derived(statsStore.isLoading(repoId));

  const grid = $derived(buildHeatmap(activity));
  const summary = $derived(totals(activity));
  const streakSummary = $derived(streaks(activity, todayISO()));
  /** Longest contributor count — the bar scale (0-safe). */
  const maxCount = $derived(
    contributors.reduce((max, c) => Math.max(max, c.count), 0),
  );
  const rules = $derived(branchColorStore.rules(root));

  function applyAuthorFilter(): void {
    void statsStore.load(repoId, { author: authorInput });
  }

  function clearAuthorFilter(): void {
    authorInput = "";
    void statsStore.load(repoId, { author: null });
  }

  function refresh(): void {
    void statsStore.load(repoId, {
      author: authorInput.trim() ? authorInput : null,
    });
  }

  function addRule(): void {
    const rule = branchColorStore.addRule(root, newPattern, newColor);
    if (rule) {
      newPattern = "";
      newColor = "";
    }
  }

  function removeRule(rule: BranchColorRule): void {
    const index = rules.indexOf(rule);
    if (index >= 0) branchColorStore.removeRule(root, index);
  }

  /** Intensity background for one heatmap level (0..4). */
  function levelBackground(level: number): string {
    switch (level) {
      case 1:
        return "color-mix(in srgb, var(--primary) 25%, transparent)";
      case 2:
        return "color-mix(in srgb, var(--primary) 50%, transparent)";
      case 3:
        return "color-mix(in srgb, var(--primary) 75%, transparent)";
      case 4:
        return "var(--primary)";
      default:
        return "var(--muted)";
    }
  }

  function cellLabel(day: string | null, count: number): string {
    if (!day) return "No commits";
    return count === 1
      ? `1 commit on ${day}`
      : `${count} commits on ${day}`;
  }
</script>

<section
  class="stats relative flex min-h-0 flex-1 flex-col gap-2 overflow-x-hidden overflow-y-auto bg-background px-3 py-2 text-xs text-foreground"
  aria-label="Contribution statistics"
>
  {#if !supported}
    <p class="note m-0 flex-none text-muted-foreground" role="note">
      Contribution stats need the mygitui desktop app (IPC unavailable here).
    </p>
  {:else}
    <header class="stats-head flex min-w-0 flex-none flex-wrap items-center gap-1.5">
      <span class="title mr-1 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap font-medium">Contribution stats</span>
      <input
        class="author w-44 min-w-20 flex-[0_1_auto] rounded-sm border border-border bg-background px-[7px] py-[2px] text-[11.5px] text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
        type="search"
        placeholder="Filter by author"
        aria-label="Filter stats by author"
        list="stats-contributors-datalist"
        value={authorInput}
        onchange={applyAuthorFilter}
        onkeydown={(e) => {
          if (e.key === "Enter") applyAuthorFilter();
        }}
      />
      <Button
        class="h-auto whitespace-nowrap rounded-sm px-2 py-[2px] text-[11px] font-normal"
        variant="secondary"
        type="button"
        title="Clear the author filter"
        onclick={clearAuthorFilter}
      >
        All authors
      </Button>
      <span class="spacer flex-1"></span>
      <Button
        class="h-auto whitespace-nowrap rounded-sm px-2 py-[2px] text-[11px] font-normal"
        variant="secondary"
        type="button"
        bind:ref={colorsButton}
        aria-expanded={colorsOpen}
        title="Branch color rules (applied to the commit graph)"
        onclick={() => (colorsOpen = !colorsOpen)}
      >
        Colors…
      </Button>
      <Button
        class="h-auto whitespace-nowrap rounded-sm px-2 py-[2px] text-[11px] font-normal"
        variant="secondary"
        type="button"
        title="Refresh stats"
        onclick={refresh}
      >
        ↻
      </Button>
    </header>

    {#if error}
      <p class="error m-0 flex flex-none items-center gap-2 rounded-md bg-destructive/10 px-2 py-1 text-destructive" role="alert">
        {error}
        <Button class="h-auto whitespace-nowrap rounded-sm px-2 py-[2px] text-[11px] font-normal" variant="secondary" type="button" onclick={refresh}>Retry</Button>
      </p>
    {/if}

    {#if loading && activity.length === 0 && contributors.length === 0}
      <p class="note m-0 flex-none text-muted-foreground" role="status">Loading stats…</p>
    {:else}
      <!-- 2. summary cards -->
      <div class="cards flex flex-none flex-wrap gap-1.5">
        <div class="card flex min-w-[4.5rem] flex-col gap-[1px] rounded-md border border-border bg-card px-2.5 py-[5px]">
          <span class="card-value text-[15px] font-semibold tabular-nums">{summary.commits}</span>
          <span class="card-label text-[10px] uppercase tracking-[0.03em] text-muted-foreground">commits</span>
        </div>
        <div class="card flex min-w-[4.5rem] flex-col gap-[1px] rounded-md border border-border bg-card px-2.5 py-[5px]">
          <span class="card-value text-[15px] font-semibold tabular-nums">{summary.activeDays}</span>
          <span class="card-label text-[10px] uppercase tracking-[0.03em] text-muted-foreground">active days</span>
        </div>
        <div class="card flex min-w-[4.5rem] flex-col gap-[1px] rounded-md border border-border bg-card px-2.5 py-[5px]">
          <span class="card-value text-[15px] font-semibold tabular-nums">{summary.avgPerDay}</span>
          <span class="card-label text-[10px] uppercase tracking-[0.03em] text-muted-foreground">avg / active day</span>
        </div>
        <div class="card flex min-w-[4.5rem] flex-col gap-[1px] rounded-md border border-border bg-card px-2.5 py-[5px]">
          <span class="card-value text-[15px] font-semibold tabular-nums">{streakSummary.current}</span>
          <span class="card-label text-[10px] uppercase tracking-[0.03em] text-muted-foreground">current streak</span>
        </div>
        <div class="card flex min-w-[4.5rem] flex-col gap-[1px] rounded-md border border-border bg-card px-2.5 py-[5px]">
          <span class="card-value text-[15px] font-semibold tabular-nums">{streakSummary.longest}</span>
          <span class="card-label text-[10px] uppercase tracking-[0.03em] text-muted-foreground">longest streak</span>
        </div>
      </div>

      <!-- 3. heatmap (table semantics; weeks are columns, rows Sun..Sat).
           tabindex makes the scrollable region keyboard-scrollable. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div class="heatmap-wrap flex-none overflow-x-auto pb-[2px]" role="region" aria-label="Contribution heatmap" tabindex="0">
        <table class="heatmap border-separate border-spacing-[2px] text-[10px] text-muted-foreground">
          <thead>
            <tr>
              {#each grid.weeks as _, week (week)}
                <th class="min-w-[11px] whitespace-nowrap pb-[2px] text-left font-normal" scope="col">
                  {grid.monthLabels.find((m) => m.week === week)?.label ?? ""}
                </th>
              {/each}
            </tr>
          </thead>
          <tbody>
            {#each [0, 1, 2, 3, 4, 5, 6] as dow (dow)}
              <tr>
                {#each grid.weeks as column, week (week)}
                  {@const cell = column[dow]}
                  <td
                    class="cell h-[11px] w-[11px] min-w-[11px] rounded-[2px]"
                    aria-label={cell ? cellLabel(cell.day, cell.count) : "No commits"}
                    title={cell ? cellLabel(cell.day, cell.count) : ""}
                    style:background={cell ? levelBackground(cell.level) : "transparent"}
                  ></td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
        <div class="legend mt-1 flex items-center gap-[3px] text-[10px] text-muted-foreground" aria-hidden="true">
          <span>Less</span>
          {#each [0, 1, 2, 3, 4] as level (level)}
            <span class="cell legend-cell inline-block h-[11px] w-[11px] flex-none rounded-[2px]" style:background={levelBackground(level)}></span>
          {/each}
          <span>More</span>
        </div>
      </div>

      <!-- 4. contributors -->
      <section class="contributors" aria-label="Contributors">
        <h3 class="section-title mb-[2px] mt-1 text-[11.5px] font-medium">Contributors</h3>
        {#if contributors.length === 0}
          <p class="note m-0 flex-none text-muted-foreground">No commits in the last {entry.maxDays} days.</p>
        {:else}
          <ul class="contributor-list m-0 flex list-none flex-col gap-[3px] p-0">
            {#each contributors as c (c.name + c.email)}
              <li class="contributor grid grid-cols-[minmax(6rem,10rem)_1fr_2.5rem_auto] items-center gap-2">
                <span class="cname overflow-hidden text-ellipsis whitespace-nowrap" title={c.email}>{c.name}</span>
                <span class="cbar-wrap h-2 overflow-hidden rounded bg-muted" aria-hidden="true">
                  <span
                    class="cbar block h-full rounded bg-primary"
                    style:width={`${maxCount === 0 ? 0 : Math.round((c.count / maxCount) * 100)}%`}
                  ></span>
                </span>
                <span class="ccount text-right tabular-nums">{c.count}</span>
                <span class="crange whitespace-nowrap text-[10.5px] text-muted-foreground">{c.first_day} → {c.last_day}</span>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}

    {#if colorsOpen}
      <!-- Branch color rules editor (popover): pattern → color, first match wins. -->
      <div
        class="colors-pop invisible fixed left-0 top-0 z-40 flex max-h-[calc(100vh-1rem)] w-[min(22rem,calc(100vw-1rem))] flex-col gap-1.5 overflow-y-auto rounded-lg border border-border bg-popover px-2.5 py-2 shadow-[0_0.35rem_1.25rem_rgb(0_0_0/0.25)]"
        role="dialog"
        aria-label="Branch color rules"
        use:floating={{ anchor: colorsButton ?? undefined, alignEnd: true }}
      >
        <div class="colors-head flex items-center justify-between font-medium">
          <span>Branch color rules</span>
          <button
            class="btn cursor-pointer whitespace-nowrap rounded-sm border border-border bg-muted px-2 py-[2px] text-[11px] text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
            type="button"
            aria-label="Close branch color rules"
            onclick={() => (colorsOpen = false)}
          >
            ×
          </button>
        </div>
        <p class="colors-hint m-0 text-[10.5px] text-muted-foreground">
          Patterns match branch names on graph rows: <code class="font-mono">feature/*</code>
          (prefix), <code class="font-mono">*-hotfix</code> (suffix), <code class="font-mono">main</code>
          (exact), <code class="font-mono">*</code> (all). First match wins; matching rows are
          recolored in the commit graph.
        </p>
        {#if rules.length === 0}
          <p class="note m-0 flex-none text-muted-foreground">No rules yet.</p>
        {:else}
          <ul class="rule-list m-0 flex list-none flex-col gap-1 p-0">
            {#each rules as rule, index (index)}
              <li class="rule flex items-center gap-1.5">
                <span class="swatch h-3.5 w-3.5 flex-none rounded-[3px] border border-border" style:background={rule.color}></span>
                <input
                  class="rule-pattern min-w-0 flex-1 rounded-sm border border-border bg-background px-[5px] py-[2px] font-mono text-[11px] text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
                  aria-label={`Pattern ${index + 1}`}
                  value={rule.pattern}
                  onchange={(e) =>
                    branchColorStore.updateRule(root, index, {
                      pattern: (e.currentTarget as HTMLInputElement).value,
                    })}
                />
                <input
                  class="rule-color h-6 w-8 flex-none cursor-pointer rounded-sm border border-border bg-background p-0"
                  type="color"
                  aria-label={`Color ${index + 1}`}
                  value={/^#[0-9a-fA-F]{6}$/.test(rule.color) ? rule.color : "#6750a4"}
                  onchange={(e) =>
                    branchColorStore.updateRule(root, index, {
                      color: (e.currentTarget as HTMLInputElement).value,
                    })}
                />
                <Button
                  class="h-auto whitespace-nowrap rounded-sm px-2 py-[2px] text-[11px] font-normal"
                  variant="secondary"
                  type="button"
                  aria-label={`Remove rule ${rule.pattern}`}
                  onclick={() => removeRule(rule)}
                >
                  ×
                </Button>
              </li>
            {/each}
          </ul>
        {/if}
        <form
          class="rule-add flex items-center gap-1.5 border-t border-border pt-1"
          onsubmit={(e) => {
            e.preventDefault();
            addRule();
          }}
        >
          <input
            class="rule-pattern min-w-0 flex-1 rounded-sm border border-border bg-background px-[5px] py-[2px] font-mono text-[11px] text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
            placeholder="Pattern (feature/*)"
            aria-label="New rule pattern"
            value={newPattern}
            oninput={(e) => (newPattern = (e.currentTarget as HTMLInputElement).value)}
          />
          <input
            class="rule-color h-6 w-8 flex-none cursor-pointer rounded-sm border border-border bg-background p-0"
            type="color"
            aria-label="New rule color"
            value="#6750a4"
            oninput={(e) => (newColor = (e.currentTarget as HTMLInputElement).value)}
          />
          <Button class="h-auto whitespace-nowrap rounded-sm px-2 py-[2px] text-[11px] font-normal" variant="secondary" type="submit">Add</Button>
        </form>
      </div>
    {/if}

    <datalist id="stats-contributors-datalist">
      {#each contributors as c (c.email)}
        <option value={c.name}>{c.email}</option>
      {/each}
    </datalist>
  {/if}
</section>
