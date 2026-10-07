<script lang="ts">
  /**
   * Quick switcher overlay (cmd/ctrl+K, B4 lane).
   *
   * Mounted once from App.svelte; a `svelte:window` keydown listener owns
   * the global shortcut (toggle) and Escape. Fuzzy search (scorer + item
   * building live in `$lib/stores/switcher.svelte`) over open tabs and
   * recent repositories; Enter/click activates — `setActive` for tabs,
   * `openTab` for recents (failures toast, the overlay still closes).
   *
   * Accessibility: combobox input driving a `listbox` via
   * `aria-activedescendant`; ↑/↓/Home/End navigate, Enter opens, Esc closes.
   */
  import { recentRepos, tabStore } from "$lib/stores/tabs.svelte";
  import {
    activateSwitcherItem,
    buildSwitcherItems,
    closeSwitcher,
    isSwitcherOpen,
    rankSwitcherItems,
    toggleSwitcher,
    type RankedItem,
  } from "$lib/stores/switcher.svelte";
  import { toast } from "$lib/toast";

  const open = $derived(isSwitcherOpen());

  let query = $state("");
  let activeIndex = $state(0);
  let inputEl: HTMLInputElement | undefined = $state();

  // Snapshot recents/tabs while computing matches; tabs are rune state so
  // this stays fresh, and recents are re-read on every open (see effect).
  const items = $derived(buildSwitcherItems(recentRepos.list(), tabStore.tabs));
  const ranked = $derived(rankSwitcherItems(items, query));

  // Opening resets the session and focuses the input; closing clears it.
  $effect(() => {
    if (open) {
      query = "";
      activeIndex = 0;
      inputEl?.focus();
    }
  });

  // Keep the highlighted option inside the (possibly filtered) list.
  $effect(() => {
    if (activeIndex >= ranked.length) {
      activeIndex = Math.max(0, ranked.length - 1);
    }
  });

  function close(): void {
    query = "";
    closeSwitcher();
  }

  /** Escape on the (never-focused) scrim still closes — defense in depth. */
  function onScrimKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  }

  /**
   * Keys typed inside the panel stop at the panel (they belong to the
   * combobox); Escape keeps bubbling so the scrim/window close handlers run.
   */
  function onPanelKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape") event.stopPropagation();
  }

  function onGlobalKeydown(event: KeyboardEvent): void {
    const k = event.key.toLowerCase();
    if ((event.metaKey || event.ctrlKey) && k === "k") {
      event.preventDefault();
      query = "";
      toggleSwitcher();
      return;
    }
    if (open && event.key === "Escape") {
      event.preventDefault();
      close();
    }
  }

  function onInputKeydown(event: KeyboardEvent): void {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const delta = event.key === "ArrowDown" ? 1 : -1;
      activeIndex = Math.min(
        ranked.length - 1,
        Math.max(0, activeIndex + delta),
      );
    } else if (event.key === "Home") {
      event.preventDefault();
      activeIndex = 0;
    } else if (event.key === "End") {
      event.preventDefault();
      activeIndex = Math.max(0, ranked.length - 1);
    } else if (event.key === "Enter") {
      const item = ranked[activeIndex];
      if (item) {
        event.preventDefault();
        void activate(item);
      }
    }
  }

  async function activate(item: RankedItem): Promise<void> {
    try {
      await activateSwitcherItem(item);
      query = "";
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast(`Failed to open ${item.sub}: ${message}`, { kind: "error" });
    }
  }

  /** Matched label characters (positions index `label + " " + sub`). */
  function labelHits(item: RankedItem): Set<number> {
    const len = item.label.length;
    return new Set(item.positions.filter((p) => p < len));
  }
</script>

<svelte:window onkeydown={onGlobalKeydown} />

{#if open}
  <div class="scrim" role="presentation" onclick={close} onkeydown={onScrimKeydown}>
    <div
      class="panel"
      role="dialog"
      aria-modal="true"
      aria-label="Quick switcher"
      tabindex="-1"
      onclick={(event) => event.stopPropagation()}
      onkeydown={onPanelKeydown}
    >
      <input
        class="search"
        type="text"
        role="combobox"
        aria-expanded="true"
        aria-controls="switcher-list"
        aria-activedescendant={ranked.length > 0 ? `sw-opt-${activeIndex}` : undefined}
        aria-label="Search repositories"
        aria-autocomplete="list"
        placeholder="Search open tabs and recent repositories…"
        bind:value={query}
        bind:this={inputEl}
        onkeydown={onInputKeydown}
      />
      <ul id="switcher-list" class="list" role="listbox" aria-label="Repositories">
        {#each ranked as item, i (item.id)}
          {@const hits = labelHits(item)}
          <li
            id={`sw-opt-${i}`}
            class="option"
            class:active={i === activeIndex}
            role="option"
            aria-selected={i === activeIndex}
            onclick={() => void activate(item)}
            onkeydown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void activate(item);
              }
            }}
            onmouseenter={() => (activeIndex = i)}
          >
            <span class="label">
              {#each item.label as ch, j (j)}<span class:hl={hits.has(j)}>{ch}</span>{/each}
            </span>
            <span class="sub">{item.sub}</span>
            {#if item.kind === "tab"}
              <span class="badge" aria-label="Open tab">open</span>
            {/if}
          </li>
        {/each}
        {#if ranked.length === 0}
          <li class="none" role="option" aria-disabled="true" aria-selected="false">
            No matches for “{query}”
          </li>
        {/if}
      </ul>
      <footer class="hint">↑↓ navigate · ↵ open · esc close · cmd/ctrl+K toggle</footer>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: color-mix(in srgb, var(--m3-scrim, #000) 32%, transparent);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
  }

  .panel {
    width: min(34rem, calc(100vw - 2rem));
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-radius: var(--m3-shape-extra-large, 28px);
    box-shadow: var(--m3-elevation-3, 0 4px 8px rgba(0, 0, 0, 0.3));
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .search {
    margin: 0.75rem;
    padding: 0.625rem 1rem;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-surface-container-highest, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
  }

  .search:focus-visible {
    outline: 2px solid var(--m3-primary);
  }

  .search::placeholder {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 0.25rem 0.5rem 0.5rem;
    max-height: 50vh;
    overflow-y: auto;
  }

  .option {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0.75rem;
    border-radius: var(--m3-shape-full, 9999px);
    cursor: pointer;
    min-width: 0;
  }

  .option.active {
    background: var(--m3-secondary-container, var(--m3-surface-container-highest));
  }

  .label {
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .label .hl {
    color: var(--m3-primary);
    font-weight: 700;
  }

  .sub {
    flex: 1;
    min-width: 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .badge {
    flex: none;
    font-size: 0.625rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--m3-primary);
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.0625rem 0.5rem;
  }

  .none {
    padding: 1rem 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.8125rem;
  }

  .hint {
    padding: 0.5rem 1rem 0.75rem;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }
</style>
