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
  import { Input } from "$lib/components/ui/input/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";

  const open = $derived(isSwitcherOpen());

  let query = $state("");
  let activeIndex = $state(0);
  let inputEl: HTMLInputElement | null = $state(null);

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
  <div
    class="scrim fixed inset-0 z-[60] flex justify-center items-start pt-[12vh] bg-black/50"
    role="presentation"
    onclick={close}
    onkeydown={onScrimKeydown}
  >
    <div
      class="panel flex flex-col overflow-hidden w-[min(34rem,calc(100vw-2rem))] bg-popover text-popover-foreground rounded-xl shadow-lg"
      role="dialog"
      aria-modal="true"
      aria-label="Quick switcher"
      tabindex="-1"
      onclick={(event) => event.stopPropagation()}
      onkeydown={onPanelKeydown}
    >
      <Input
        class="search m-3 h-auto rounded-full border-0 bg-muted px-4 py-2.5 text-base shadow-none ring-0 focus-visible:border-0 focus-visible:ring-2 focus-visible:ring-ring md:text-base"
        type="text"
        role="combobox"
        aria-expanded="true"
        aria-controls="switcher-list"
        aria-activedescendant={ranked.length > 0 ? `sw-opt-${activeIndex}` : undefined}
        aria-label="Search repositories"
        aria-autocomplete="list"
        placeholder="Search open tabs and recent repositories…"
        bind:value={query}
        bind:ref={inputEl}
        onkeydown={onInputKeydown}
      />
      <ul
        id="switcher-list"
        class="list list-none m-0 px-2 pt-1 pb-2 max-h-[50vh] overflow-y-auto"
        role="listbox"
        aria-label="Repositories"
      >
        {#each ranked as item, i (item.id)}
          {@const hits = labelHits(item)}
          <li
            id={`sw-opt-${i}`}
            class="option flex items-center gap-2 px-3 py-2 rounded-full cursor-pointer min-w-0"
            class:active={i === activeIndex}
            class:bg-accent={i === activeIndex}
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
            <span class="label min-w-0 font-medium whitespace-nowrap overflow-hidden text-ellipsis">
              {#each item.label as ch, j (j)}<span
                  class:hl={hits.has(j)}
                  class:text-primary={hits.has(j)}
                  class:font-bold={hits.has(j)}
                >{ch}</span>{/each}
            </span>
            <span
              class="sub flex-1 min-w-0 text-muted-foreground font-mono text-[0.6875rem] whitespace-nowrap overflow-hidden text-ellipsis"
            >{item.sub}</span>
            {#if item.kind === "tab"}
              <Badge
                class="badge h-auto shrink-0 rounded-full border-border px-2 py-0.5 text-[0.625rem] uppercase tracking-[0.05em] text-primary"
                variant="outline"
                aria-label="Open tab"
              >open</Badge>
            {/if}
          </li>
        {/each}
        {#if ranked.length === 0}
          <li
            class="none px-3 py-4 text-muted-foreground text-[0.8125rem]"
            role="option"
            aria-disabled="true"
            aria-selected="false"
          >
            No matches for “{query}”
          </li>
        {/if}
      </ul>
      <footer class="hint px-4 pt-2 pb-3 border-t border-border text-muted-foreground text-[0.6875rem]">↑↓ navigate · ↵ open · esc close · cmd/ctrl+K toggle</footer>
    </div>
  </div>
{/if}
