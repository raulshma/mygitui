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
   *      labels along the top, intensity via --m3-primary color-mix
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
        return "color-mix(in srgb, var(--m3-primary) 25%, transparent)";
      case 2:
        return "color-mix(in srgb, var(--m3-primary) 50%, transparent)";
      case 3:
        return "color-mix(in srgb, var(--m3-primary) 75%, transparent)";
      case 4:
        return "var(--m3-primary)";
      default:
        return "var(--m3-surface-container-high, var(--m3-surface))";
    }
  }

  function cellLabel(day: string | null, count: number): string {
    if (!day) return "No commits";
    return count === 1
      ? `1 commit on ${day}`
      : `${count} commits on ${day}`;
  }
</script>

<section class="stats" aria-label="Contribution statistics">
  {#if !supported}
    <p class="note" role="note">
      Contribution stats need the mygitui desktop app (IPC unavailable here).
    </p>
  {:else}
    <header class="stats-head">
      <span class="title">Contribution stats</span>
      <input
        class="author"
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
      <button
        class="btn"
        type="button"
        title="Clear the author filter"
        onclick={clearAuthorFilter}
      >
        All authors
      </button>
      <span class="spacer"></span>
      <button
        class="btn"
        type="button"
        aria-expanded={colorsOpen}
        title="Branch color rules (applied to the commit graph)"
        onclick={() => (colorsOpen = !colorsOpen)}
      >
        Colors…
      </button>
      <button
        class="btn"
        type="button"
        title="Refresh stats"
        onclick={refresh}
      >
        ↻
      </button>
    </header>

    {#if error}
      <p class="error" role="alert">
        {error}
        <button class="btn" type="button" onclick={refresh}>Retry</button>
      </p>
    {/if}

    {#if loading && activity.length === 0 && contributors.length === 0}
      <p class="note" role="status">Loading stats…</p>
    {:else}
      <!-- 2. summary cards -->
      <div class="cards">
        <div class="card">
          <span class="card-value">{summary.commits}</span>
          <span class="card-label">commits</span>
        </div>
        <div class="card">
          <span class="card-value">{summary.activeDays}</span>
          <span class="card-label">active days</span>
        </div>
        <div class="card">
          <span class="card-value">{summary.avgPerDay}</span>
          <span class="card-label">avg / active day</span>
        </div>
        <div class="card">
          <span class="card-value">{streakSummary.current}</span>
          <span class="card-label">current streak</span>
        </div>
        <div class="card">
          <span class="card-value">{streakSummary.longest}</span>
          <span class="card-label">longest streak</span>
        </div>
      </div>

      <!-- 3. heatmap (table semantics; weeks are columns, rows Sun..Sat).
           tabindex makes the scrollable region keyboard-scrollable. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div class="heatmap-wrap" role="region" aria-label="Contribution heatmap" tabindex="0">
        <table class="heatmap">
          <thead>
            <tr>
              {#each grid.weeks as _, week (week)}
                <th scope="col">
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
                    class="cell"
                    aria-label={cell ? cellLabel(cell.day, cell.count) : "No commits"}
                    title={cell ? cellLabel(cell.day, cell.count) : ""}
                    style:background={cell ? levelBackground(cell.level) : "transparent"}
                  ></td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
        <div class="legend" aria-hidden="true">
          <span>Less</span>
          {#each [0, 1, 2, 3, 4] as level (level)}
            <span class="cell legend-cell" style:background={levelBackground(level)}></span>
          {/each}
          <span>More</span>
        </div>
      </div>

      <!-- 4. contributors -->
      <section class="contributors" aria-label="Contributors">
        <h3 class="section-title">Contributors</h3>
        {#if contributors.length === 0}
          <p class="note">No commits in the last {entry.maxDays} days.</p>
        {:else}
          <ul class="contributor-list">
            {#each contributors as c (c.name + c.email)}
              <li class="contributor">
                <span class="cname" title={c.email}>{c.name}</span>
                <span class="cbar-wrap" aria-hidden="true">
                  <span
                    class="cbar"
                    style:width={`${maxCount === 0 ? 0 : Math.round((c.count / maxCount) * 100)}%`}
                  ></span>
                </span>
                <span class="ccount">{c.count}</span>
                <span class="crange">{c.first_day} → {c.last_day}</span>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}

    {#if colorsOpen}
      <!-- Branch color rules editor (popover): pattern → color, first match wins. -->
      <div class="colors-pop" role="dialog" aria-label="Branch color rules">
        <div class="colors-head">
          <span>Branch color rules</span>
          <button
            class="btn"
            type="button"
            aria-label="Close branch color rules"
            onclick={() => (colorsOpen = false)}
          >
            ×
          </button>
        </div>
        <p class="colors-hint">
          Patterns match branch names on graph rows: <code>feature/*</code>
          (prefix), <code>*-hotfix</code> (suffix), <code>main</code>
          (exact), <code>*</code> (all). First match wins; matching rows are
          recolored in the commit graph.
        </p>
        {#if rules.length === 0}
          <p class="note">No rules yet.</p>
        {:else}
          <ul class="rule-list">
            {#each rules as rule, index (index)}
              <li class="rule">
                <span class="swatch" style:background={rule.color}></span>
                <input
                  class="rule-pattern"
                  aria-label={`Pattern ${index + 1}`}
                  value={rule.pattern}
                  onchange={(e) =>
                    branchColorStore.updateRule(root, index, {
                      pattern: (e.currentTarget as HTMLInputElement).value,
                    })}
                />
                <input
                  class="rule-color"
                  type="color"
                  aria-label={`Color ${index + 1}`}
                  value={/^#[0-9a-fA-F]{6}$/.test(rule.color) ? rule.color : "#6750a4"}
                  onchange={(e) =>
                    branchColorStore.updateRule(root, index, {
                      color: (e.currentTarget as HTMLInputElement).value,
                    })}
                />
                <button
                  class="btn"
                  type="button"
                  aria-label={`Remove rule ${rule.pattern}`}
                  onclick={() => removeRule(rule)}
                >
                  ×
                </button>
              </li>
            {/each}
          </ul>
        {/if}
        <form
          class="rule-add"
          onsubmit={(e) => {
            e.preventDefault();
            addRule();
          }}
        >
          <input
            class="rule-pattern"
            placeholder="Pattern (feature/*)"
            aria-label="New rule pattern"
            value={newPattern}
            oninput={(e) => (newPattern = (e.currentTarget as HTMLInputElement).value)}
          />
          <input
            class="rule-color"
            type="color"
            aria-label="New rule color"
            value="#6750a4"
            oninput={(e) => (newColor = (e.currentTarget as HTMLInputElement).value)}
          />
          <button class="btn" type="submit">Add</button>
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

<style>
  .stats {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.5rem 0.75rem;
    overflow-y: auto;
    overflow-x: hidden;
    font-size: 0.75rem;
    background: var(--m3-surface);
    color: var(--m3-on-surface);
  }

  .stats-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.375rem;
    flex: none;
    min-width: 0;
  }

  .title {
    font-weight: 500;
    margin-right: 0.25rem;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .spacer {
    flex: 1;
  }

  input.author {
    width: 11rem;
    min-width: 5rem;
    flex: 0 1 auto;
    padding: 0.15rem 0.45rem;
    font-size: 0.72rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface);
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
  }

  .btn {
    padding: 0.12rem 0.5rem;
    font-size: 0.7rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-high, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
    white-space: nowrap;
  }

  .btn:hover,
  .btn:focus-visible,
  input.author:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }

  .error {
    margin: 0;
    padding: 0.25rem 0.5rem;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    color: var(--m3-on-error-container, var(--m3-error));
    background: var(--m3-error-container, transparent);
    border-radius: var(--m3-shape-small, 8px);
    flex: none;
  }

  .note {
    margin: 0;
    color: var(--m3-on-surface-variant);
    flex: none;
  }

  .cards {
    display: flex;
    flex-wrap: wrap;
    gap: 0.375rem;
    flex: none;
  }

  .card {
    display: flex;
    flex-direction: column;
    gap: 0.05rem;
    padding: 0.35rem 0.65rem;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    min-width: 4.5rem;
  }

  .card-value {
    font-size: 0.95rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .card-label {
    font-size: 0.625rem;
    color: var(--m3-on-surface-variant);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .heatmap-wrap {
    flex: none;
    overflow-x: auto;
    padding-bottom: 0.15rem;
  }

  .heatmap {
    border-collapse: separate;
    border-spacing: 2px;
    font-size: 0.625rem;
    color: var(--m3-on-surface-variant);
  }

  .heatmap th {
    font-weight: 400;
    text-align: left;
    padding: 0 0 0.15rem;
    min-width: 11px;
    white-space: nowrap;
  }

  .heatmap td {
    width: 11px;
    height: 11px;
    min-width: 11px;
    border-radius: 2px;
  }

  .legend {
    display: flex;
    align-items: center;
    gap: 3px;
    margin-top: 0.25rem;
    color: var(--m3-on-surface-variant);
    font-size: 0.625rem;
  }

  .legend-cell {
    display: inline-block;
  }

  .cell {
    display: inline-block;
    border-radius: 2px;
  }

  .section-title {
    margin: 0.25rem 0 0.15rem;
    font-size: 0.72rem;
    font-weight: 500;
  }

  .contributor-list {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  .contributor {
    display: grid;
    grid-template-columns: minmax(6rem, 10rem) 1fr 2.5rem auto;
    align-items: center;
    gap: 0.5rem;
  }

  .cname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .cbar-wrap {
    height: 8px;
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-radius: 4px;
    overflow: hidden;
  }

  .cbar {
    display: block;
    height: 100%;
    background: var(--m3-primary);
    border-radius: 4px;
  }

  .ccount {
    font-variant-numeric: tabular-nums;
    text-align: right;
  }

  .crange {
    color: var(--m3-on-surface-variant);
    font-size: 0.65rem;
    white-space: nowrap;
  }

  .colors-pop {
    position: absolute;
    top: 2.4rem;
    right: 0.75rem;
    z-index: 5;
    width: min(22rem, calc(100% - 1.5rem));
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    padding: 0.5rem 0.65rem;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-medium, 12px);
    background: var(--m3-surface-container, var(--m3-surface));
    box-shadow: 0 0.35rem 1.25rem rgb(0 0 0 / 0.25);
  }

  .colors-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-weight: 500;
  }

  .colors-hint {
    margin: 0;
    color: var(--m3-on-surface-variant);
    font-size: 0.65rem;
  }

  .colors-hint code {
    font-family: ui-monospace, Consolas, monospace;
  }

  .rule-list {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .rule,
  .rule-add {
    display: flex;
    align-items: center;
    gap: 0.375rem;
  }

  .rule-add {
    padding-top: 0.25rem;
    border-top: 1px solid var(--m3-outline-variant);
  }

  .swatch {
    flex: none;
    width: 14px;
    height: 14px;
    border-radius: 3px;
    border: 1px solid var(--m3-outline-variant);
  }

  .rule-pattern {
    flex: 1;
    min-width: 0;
    padding: 0.12rem 0.35rem;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.68rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface);
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
  }

  .rule-color {
    flex: none;
    width: 2rem;
    height: 1.4rem;
    padding: 0;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface);
    cursor: pointer;
  }
</style>
