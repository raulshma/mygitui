<script lang="ts">
  /**
   * Working-copy status panel (real, B4 lane).
   *
   * Sections (Conflicted / Staged / Unstaged / Untracked, collapsible, with
   * counts) come from the pure model in `statusModel.ts`; one entry can
   * appear in Staged AND Unstaged. Rows carry a change-kind letter
   * (A/M/D/R/C/U/!), a path with the dir dimmed and the basename bold, and
   * `old → new` for renames.
   *
   * Interaction: click opens the diff (`onOpenDiff`) and selects the path;
   * the checkbox toggles `selected` membership (visual only in M1 — staging
   * actions land in M2, hence the disabled Stage/Unstage all buttons).
   * Full keyboard: roving-tabindex rows, Arrows/Home/End to move (Shift
   * extends a range, Ctrl moves without selecting), Space toggles selection,
   * Enter opens the diff.
   *
   * Accessibility: `role="tree"` + grouped `treeitem`s, aria-labels
   * everywhere, `aria-expanded` collapse toggles, `aria-selected` rows.
   */
  import type { RepoStatus, StatusEntry } from "$lib/ipc/types";
  import {
    buildStatusIndex,
    filterSections,
    kindClass,
    kindLetter,
    type SectionId,
    type StatusIndex,
    type StatusRow,
  } from "./statusModel";

  let {
    status,
    onOpenDiff,
    selected = $bindable([]),
  }: {
    status: RepoStatus | null;
    onOpenDiff: (entry: StatusEntry) => void;
    selected: string[];
  } = $props();

  let filterText = $state("");
  let collapsed = $state<Record<SectionId, boolean>>({
    conflicted: false,
    staged: false,
    unstaged: false,
    untracked: false,
  });

  /** Roving focus: key of the focused row (null → first visible row). */
  let focusKey: string | null = $state(null);
  /** Index (in visibleRows) of the selection anchor for Shift ranges. */
  let anchorIndex = 0;
  /** Row elements by key (roving-tabindex focus targets; non-reactive). */
  const rowEls: Record<string, HTMLElement | undefined> = {};

  /** Search index — rebuilt once per status change, not per keystroke. */
  const index: StatusIndex | null = $derived(
    status ? buildStatusIndex(status.entries) : null,
  );
  const sections = $derived(index ? filterSections(index, filterText) : []);
  /** Rows actually on screen (collapsed sections excluded) for key nav. */
  const visibleRows = $derived(
    sections.filter((s) => !collapsed[s.id]).flatMap((s) => s.rows),
  );
  /** The single tab-reachable row (falls back to the first visible row when
   *  the focused one is filtered out or collapsed away). */
  const tabIndexKey = $derived(
    focusKey !== null && visibleRows.some((r) => r.key === focusKey)
      ? focusKey
      : (visibleRows[0]?.key ?? null),
  );

  function isSelected(path: string): boolean {
    return selected.includes(path);
  }

  function toggleSelected(path: string): void {
    selected = isSelected(path)
      ? selected.filter((p) => p !== path)
      : [...selected, path];
  }

  function setRangeSelection(from: number, to: number): void {
    const [lo, hi] = from <= to ? [from, to] : [to, from];
    const paths: string[] = [];
    for (let i = lo; i <= hi; i++) {
      const path = visibleRows[i]?.entry.path;
      if (path !== undefined && !paths.includes(path)) paths.push(path);
    }
    selected = paths;
  }

  function focusRow(key: string): void {
    focusKey = key;
    rowEls[key]?.focus();
  }

  /** Plain click: select + open diff. Ctrl: toggle. Shift: range. */
  function onRowClick(event: MouseEvent, row: StatusRow, rowIndex: number): void {
    anchorIndex = rowIndex;
    focusKey = row.key;
    if (event.shiftKey) {
      setRangeSelection(anchorIndex, rowIndex);
    } else if (event.ctrlKey || event.metaKey) {
      toggleSelected(row.entry.path);
    } else {
      selected = [row.entry.path];
      onOpenDiff(row.entry);
    }
  }

  function onTreeKeydown(event: KeyboardEvent): void {
    const rows = visibleRows;
    if (rows.length === 0) return;
    const current =
      focusKey !== null ? rows.findIndex((r) => r.key === focusKey) : 0;
    const base = current === -1 ? 0 : current;

    // Space/Enter on the checkbox itself: native behavior already toggles.
    const onCheckbox = (event.target as HTMLElement).tagName === "INPUT";

    let next: number | null = null;
    if (event.key === "ArrowDown") next = (base + 1) % rows.length;
    else if (event.key === "ArrowUp") next = (base - 1 + rows.length) % rows.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = rows.length - 1;

    if (next !== null) {
      event.preventDefault();
      const row = rows[next]!;
      if (event.shiftKey) setRangeSelection(anchorIndex, next);
      else if (!event.ctrlKey && !event.metaKey) selected = [row.entry.path];
      focusRow(row.key);
      return;
    }

    if (onCheckbox && (event.key === " " || event.key === "Enter")) return;

    if (event.key === " ") {
      event.preventDefault();
      const row = rows[base];
      if (row) {
        anchorIndex = base;
        toggleSelected(row.entry.path);
      }
    } else if (event.key === "Enter") {
      event.preventDefault();
      const row = rows[base];
      if (row) {
        anchorIndex = base;
        selected = [row.entry.path];
        onOpenDiff(row.entry);
      }
    }
  }

  /** ArrowDown in the filter box drops into the first row. */
  function onFilterKeydown(event: KeyboardEvent): void {
    if (event.key === "ArrowDown" && visibleRows.length > 0) {
      event.preventDefault();
      focusRow(visibleRows[0]!.key);
    }
  }

  function toggleSection(id: SectionId): void {
    collapsed[id] = !collapsed[id];
  }
</script>

<aside aria-label="Working copy">
  {#if status}
    <div class="toolbar">
      <input
        class="filter"
        type="text"
        placeholder="Filter paths…"
        aria-label="Filter changed files by path"
        bind:value={filterText}
        onkeydown={onFilterKeydown}
      />
      <button
        class="tb"
        type="button"
        disabled
        title="Staging actions land in M2"
        aria-label="Stage all (available in M2)"
      >
        Stage all
      </button>
      <button
        class="tb"
        type="button"
        disabled
        title="Staging actions land in M2"
        aria-label="Unstage all (available in M2)"
      >
        Unstage all
      </button>
    </div>

    {#if status.merging || status.rebasing || status.sequencer || status.detached || status.ahead > 0 || status.behind > 0}
      <div class="banners">
        {#if status.merging}<span class="chip chip-state">Merge in progress</span>{/if}
        {#if status.rebasing}<span class="chip chip-state">Rebase in progress</span>{/if}
        {#if status.sequencer}<span class="chip chip-state">Cherry-pick / revert in progress</span>{/if}
        {#if status.detached}<span class="chip chip-state">Detached HEAD</span>{/if}
        {#if status.ahead > 0 || status.behind > 0}
          <span class="chip" title="Commits ahead / behind the upstream">
            ↑{status.ahead} ↓{status.behind}
          </span>
        {/if}
      </div>
    {/if}

    {#if index !== null && index.total === 0}
      <p class="empty">Working copy clean</p>
    {:else if sections.length === 0}
      <p class="empty">No files match “{filterText}”</p>
    {:else}
      <div class="tree" role="tree" aria-label="Changed files" tabindex="-1">
        {#each sections as section (section.id)}
          <div class="section" role="group" aria-labelledby={`section-${section.id}`}>
            <button
              class="section-header"
              type="button"
              id={`section-${section.id}`}
              aria-expanded={!collapsed[section.id]}
              aria-label={`${section.label} (${section.rows.length}), ${collapsed[section.id] ? "expand" : "collapse"}`}
              onclick={() => toggleSection(section.id)}
            >
              <span class="chevron" class:closed={collapsed[section.id]}>▸</span>
              <span class="section-name">{section.label}</span>
              <span class="count">{section.rows.length}</span>
            </button>
            {#if !collapsed[section.id]}
              {#each section.rows as row (row.key)}
                <div
                  class="row"
                  class:selected={isSelected(row.entry.path)}
                  role="treeitem"
                  aria-selected={isSelected(row.entry.path)}
                  aria-label={`${row.entry.path}${row.entry.old_path ? ` (renamed from ${row.entry.old_path})` : ""}`}
                  tabindex={row.key === tabIndexKey ? 0 : -1}
                  data-row-key={row.key}
                  bind:this={rowEls[row.key]}
                  onkeydown={onTreeKeydown}
                  onclick={(event) =>
                    onRowClick(
                      event,
                      row,
                      visibleRows.findIndex((r) => r.key === row.key),
                    )}
                >
                  <input
                    class="check"
                    type="checkbox"
                    checked={isSelected(row.entry.path)}
                    aria-label={`Select ${row.entry.path}`}
                    tabindex={-1}
                    onclick={(event) => {
                      event.stopPropagation();
                      toggleSelected(row.entry.path);
                    }}
                  />
                  <span class={`kind ${kindClass(row.kind)}`} aria-hidden="true">
                    {kindLetter(row.kind)}
                  </span>
                  <span class="path">
                    {#if row.entry.old_path && (row.kind === "renamed" || row.kind === "copied")}
                      <span class="old">{row.entry.old_path}</span>
                      <span class="arrow">→</span>
                    {/if}
                    {#if row.dir}<span class="dir">{row.dir}</span>{/if}
                    <span class="base">{row.base}</span>
                  </span>
                </div>
              {/each}
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  {:else}
    <p class="empty">Loading status…</p>
  {/if}
</aside>

<style>
  aside {
    display: flex;
    flex-direction: column;
    min-height: 0;
    font-size: 0.8125rem;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.375rem 0.5rem;
    flex: none;
  }

  .filter {
    flex: 1;
    min-width: 0;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    padding: 0.25rem 0.5rem;
  }

  .filter:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
    border-color: transparent;
  }

  .tb {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.75rem;
    padding: 0.25rem 0.5rem;
    cursor: not-allowed;
  }

  .banners {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    padding: 0 0.5rem 0.375rem;
    flex: none;
  }

  .chip {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.0625rem 0.5rem;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    white-space: nowrap;
  }

  .chip-state {
    background: var(--m3-tertiary-container, var(--m3-surface));
    color: var(--m3-on-tertiary-container, var(--m3-on-surface));
    border-color: transparent;
    font-weight: 500;
  }

  .tree {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding-bottom: 0.75rem;
  }

  .section-header {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 0.25rem;
    width: 100%;
    border: none;
    background: var(--m3-surface-container, var(--m3-surface));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.6875rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0.375rem 0.5rem;
    cursor: pointer;
    text-align: left;
  }

  .section-header:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .chevron {
    display: inline-block;
    transition: transform 0.1s ease;
  }

  .chevron.closed {
    transform: rotate(-90deg);
  }

  .count {
    margin-left: auto;
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0 0.4rem;
    font-variant-numeric: tabular-nums;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.125rem 0.5rem 0.125rem 0.25rem;
    cursor: pointer;
    min-width: 0;
  }

  .row:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .row:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .row.selected {
    background: var(--m3-secondary-container, var(--m3-surface-container-high));
  }

  .check {
    flex: none;
    margin: 0;
    accent-color: var(--m3-primary);
  }

  .kind {
    flex: none;
    width: 1.125rem;
    height: 1.125rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--m3-shape-extra-small, 4px);
    font-size: 0.6875rem;
    font-weight: 700;
    font-family: ui-monospace, Consolas, monospace;
  }

  .k-added {
    color: var(--m3-primary);
    background: var(--m3-primary-container, transparent);
  }

  .k-deleted {
    color: var(--m3-error);
    background: var(--m3-error-container, transparent);
  }

  .k-renamed {
    color: var(--m3-tertiary);
    background: var(--m3-tertiary-container, transparent);
  }

  .k-modified {
    color: var(--m3-secondary, var(--m3-on-surface-variant));
  }

  .k-untracked {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .k-conflicted {
    color: var(--m3-on-error-container, var(--m3-error));
    background: var(--m3-error-container, transparent);
  }

  .path {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
  }

  .dir {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 0.85;
  }

  .base {
    color: var(--m3-on-surface);
    font-weight: 600;
  }

  .old {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    opacity: 0.7;
  }

  .arrow {
    color: var(--m3-tertiary);
    padding: 0 0.125rem;
  }

  .empty {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    padding: 1rem 0.75rem;
    margin: 0;
    font-size: 0.8125rem;
  }
</style>
