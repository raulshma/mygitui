<script lang="ts">
  /**
   * Quick switcher overlay (cmd/ctrl+Shift+K as of §11 — it ceded Ctrl+K to
   * the command palette and its toggle moved onto the keybind engine as the
   * `app.quick-switcher` command, so the combo shows up in the `?`
   * cheatsheet and is rebindable in Settings like every other shortcut).
   *
   * Mounted once from App.svelte; a `svelte:window` keydown listener owns
   * Escape (the engine handles the toggle). Fuzzy search (scorer + item
   * building live in `$lib/stores/switcher.svelte`) over open tabs and
   * recent repositories; Enter/click activates — `setActive` for tabs,
   * `openTab` for recents (failures toast, the overlay still closes).
   *
   * Accessibility: combobox input driving a `listbox` via
   * `aria-activedescendant`; ↑/↓/Home/End navigate, Enter opens, Esc closes.
   */
  import { recentRepos, tabStore } from "$lib/stores/tabs.svelte";
  import { favoriteRepos } from "$lib/stores/favoriteRepos.svelte";
  import {
    activateSwitcherItem,
    buildSwitcherItems,
    closeSwitcher,
    isSwitcherOpen,
    rankSwitcherItems,
    toggleItemFavorite,
    type RankedItem,
  } from "$lib/stores/switcher.svelte";
  import { toast } from "$lib/toast";
  import { trapTab } from "$lib/components/safety/trapTab";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { describeError } from "$lib/errors";

  const open = $derived(isSwitcherOpen());

  /** Star glyphs resolved reactively (plain divs can't host `{@const}`). */
  const starIcon = $derived(appIcon("star"));
  const starOutlineIcon = $derived(appIcon("star-outline"));

  let query = $state("");
  let activeIndex = $state(0);
  let inputEl: HTMLInputElement | null = $state(null);
  let panelEl: HTMLElement | null = $state(null);
  /** Element focused before the switcher opened (restored on close). */
  let restoreTo: HTMLElement | null = null;

  // Snapshot recents/tabs while computing matches; tabs and the favorite
  // set are rune state so this stays fresh, and recents are re-read on
  // every open (see effect).
  const items = $derived(
    buildSwitcherItems(recentRepos.list(), tabStore.tabs, favoriteRepos.favorites),
  );
  const ranked = $derived(rankSwitcherItems(items, query));

  /**
   * Flat render list: the ranked rows are grouped into a Favorites block
   * (first) and an Open tabs & recents block (second), with separator
   * headings between them (`role="separator"` listbox children — keyboard
   * navigation stays on `ranked`'s option indexes and skips these).
   * `favoriteRepos.favorites` is only read to re-render the stars when the
   * set flips.
   */
  type RenderRow = { type: "option"; item: RankedItem; index: number } | { type: "separator"; label: string };
  const rows = $derived.by<RenderRow[]>(() => {
    void favoriteRepos.favorites;
    const favs: RenderRow[] = [];
    const rest: RenderRow[] = [];
    ranked.forEach((item, index) => {
      (item.kind === "favorite" ? favs : rest).push({ type: "option", item, index });
    });
    const out: RenderRow[] = [];
    if (favs.length > 0) {
      out.push({ type: "separator", label: "Favorites" });
      out.push(...favs);
      if (rest.length > 0) out.push({ type: "separator", label: "Repositories" });
    }
    out.push(...rest);
    return out;
  });

  // Opening resets the session, remembers the invoker and focuses the
  // input; closing restores focus there and clears it.
  $effect(() => {
    if (open) {
      restoreTo =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
      query = "";
      activeIndex = 0;
      inputEl?.focus();
    } else if (restoreTo !== null) {
      restoreTo.focus();
      restoreTo = null;
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
   * combobox); Tab is trapped inside the dialog; Escape keeps bubbling so
   * the scrim/window close handlers run.
   */
  function onPanelKeydown(event: KeyboardEvent): void {
    trapTab(panelEl, event);
    if (event.key !== "Escape") event.stopPropagation();
  }

  function onGlobalKeydown(event: KeyboardEvent): void {
    // The toggle combo lives in the keybind engine now (`app.quick-switcher`,
    // default ctrl/cmd+Shift+K) — this listener only closes on Escape.
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
      const message = describeError(err);
      toast(`Failed to open ${item.sub}: ${message}`, { kind: "error" });
    }
  }

  /** Matched label characters (positions index `label + " " + sub`). */
  function labelHits(item: RankedItem): Set<number> {
    const len = item.label.length;
    return new Set(item.positions.filter((p) => p < len));
  }

  /** Flips the row's repo in the favorite set without activating the row. */
  function star(event: MouseEvent, item: RankedItem): void {
    event.stopPropagation();
    event.preventDefault();
    const now = toggleItemFavorite(item);
    toast(now ? `Added ${item.label} to favorites` : `Removed ${item.label} from favorites`, {
      kind: "info",
    });
  }
</script>

<svelte:window onkeydown={onGlobalKeydown} />

{#if open}
  <div
    class="scrim fixed inset-0 z-60 flex justify-center items-start pt-[10vh] bg-black/50"
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
      bind:this={panelEl}
      onclick={(event) => event.stopPropagation()}
      onkeydown={onPanelKeydown}
    >
      <Input
        class="search m-3 h-auto rounded-full border-0 bg-muted px-4 py-2.5 text-sm shadow-none ring-0 focus-visible:border-0 focus-visible:ring-2 focus-visible:ring-ring"
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
        class="list list-none m-0 px-2 pt-1 pb-2 max-h-[55vh] overflow-y-auto"
        role="listbox"
        aria-label="Repositories"
      >
        {#each rows as row (row.type === "option" ? row.item.id : `sep-${row.label}`)}
          {#if row.type === "separator"}
            <li
              class="sep px-3 pt-2 pb-1 text-ui-micro font-semibold uppercase tracking-wide text-muted-foreground"
              role="separator"
              aria-orientation="horizontal"
            >{row.label}</li>
          {:else}
            {@const item = row.item}
            {@const i = row.index}
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
                    class:text-primary-ink={hits.has(j)}
                    class:font-bold={hits.has(j)}
                  >{ch}</span>{/each}
              </span>
              <span
                class="sub flex-1 min-w-0 text-muted-foreground font-mono text-ui-micro whitespace-nowrap overflow-hidden text-ellipsis"
              >{item.sub}</span>
              {#if item.kind === "tab"}
                <Badge
                  class="badge shrink-0 uppercase tracking-[0.05em] text-primary-ink"
                  variant="outline"
                  aria-label="Open tab"
                >open</Badge>
              {/if}
              <Hint label={favoriteRepos.has(item.sub) ? "Remove from favorites" : "Add to favorites"}>
                {#snippet child({ props })}
                  <button
                    {...props}
                    type="button"
                    class="shrink-0 flex cursor-pointer items-center justify-center rounded-full size-6 text-muted-foreground hover:bg-accent hover:text-accent-foreground aria-pressed:text-primary-ink"
                    aria-pressed={favoriteRepos.has(item.sub)}
                    aria-label={favoriteRepos.has(item.sub) ? `Remove ${item.label} from favorites` : `Add ${item.label} to favorites`}
                    onclick={(e) => star(e, item)}
                  >
                    {#if favoriteRepos.has(item.sub) && starIcon}
                      <Icon icon={starIcon} size={15} />
                    {:else if !favoriteRepos.has(item.sub) && starOutlineIcon}
                      <Icon icon={starOutlineIcon} size={15} />
                    {:else}
                      ★
                    {/if}
                  </button>
                {/snippet}
              </Hint>
            </li>
          {/if}
        {/each}
        {#if ranked.length === 0}
          <li
            class="none px-3 py-4 text-muted-foreground text-ui-base"
            role="option"
            aria-disabled="true"
            aria-selected="false"
          >
            No matches for “{query}”
          </li>
        {/if}
      </ul>
      <footer class="hint px-4 pt-2 pb-3 border-t border-border text-muted-foreground text-ui-micro">↑↓ navigate · ↵ open · esc close · cmd/ctrl+Shift+K toggle</footer>
    </div>
  </div>
{/if}
