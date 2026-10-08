<script lang="ts">
  /**
   * Command palette overlay (M4 F2) — Ctrl/Cmd+Shift+P.
   *
   * Mounted once from App.svelte; the global toggle keybinding is owned by
   * the keybind engine (`$lib/palette/keybinds.startKeybinds`, wired in
   * App.svelte), which routes "app.palette" to `togglePalette()`. This
   * component owns the overlay itself: fuzzy filtering (scorer + sections
   * live in `$lib/palette/palette.svelte`) over the command registry
   * filtered by the active-tab context, section-grouped results with a
   * "Recent" block (last 5 runs, localStorage), keyboard navigation
   * (↑/↓/Home/End/Enter/Esc) and per-command shortcut hints from the
   * effective keybind map. Running a command closes the palette; failures
   * toast (see `runCommand`).
   *
 * Accessibility mirrors the quick switcher: a combobox input driving a
 * grouped `listbox` via `aria-activedescendant` (flat option indexes), and
 * focus returns to the pre-open trigger element when the palette closes.
 */
  import { tabStore } from "$lib/stores/tabs.svelte";
  import { formatCombo } from "$lib/palette/keybinds";
  import {
    COMMANDS,
    activeCtx,
    type Command,
  } from "$lib/palette/commands";
import {
  closePalette,
  currentRecents,
  flattenSections,
  isPaletteOpen,
  paletteSections,
  runCommand,
  shortcutHint,
  fuzzyScore,
  type PaletteSection,
  type RankedCommand,
} from "$lib/palette/palette.svelte";

  const open = $derived(isPaletteOpen());
  const ctx = $derived(activeCtx());

  let query = $state("");
  let activeIndex = $state(0);
  let inputEl: HTMLInputElement | undefined = $state();
  /** Element focused before the palette opened; focus returns there on close. */
  let triggerEl: HTMLElement | null = null;

  /** Filtered + grouped commands (Recent block on top when unfiltered). */
  const sections = $derived.by(() => {
    if (!open) return [] as PaletteSection[];
    return paletteSections(COMMANDS, ctx, query, currentRecents());
  });
  /** Flat arrow-key navigation order (section by section). */
  const flat = $derived(flattenSections(sections));

  // Opening resets the session, remembers the trigger and focuses the input.
  $effect(() => {
    if (open) {
      triggerEl =
        document.activeElement instanceof HTMLElement ? document.activeElement : null;
      query = "";
      activeIndex = 0;
      inputEl?.focus();
    }
  });

  // Keep the highlighted option inside the (possibly filtered) list.
  $effect(() => {
    if (activeIndex >= flat.length) {
      activeIndex = Math.max(0, flat.length - 1);
    }
  });

  function close(): void {
    query = "";
    closePalette();
    // Return focus to the trigger (keybind source element, button, …) so
    // keyboard users are not dropped at <body>. Detached triggers no-op.
    triggerEl?.focus();
    triggerEl = null;
  }

  /** The command's effective shortcut hint ("Ctrl+Shift+P"), or null. */
  function hint(command: Command): string | null {
    return shortcutHint(command);
  }

  /** Matched title characters, for highlight rendering (may be null). */
  function titleHits(entry: RankedCommand, query: string): Set<number> {
    const hit = fuzzyScore(query, entry.command.title);
    return new Set(hit === null ? [] : hit.positions);
  }

  /** Section index + flat position of an entry (listbox option ids). */
  function optionId(index: number): string {
    return `cmd-opt-${index}`;
  }

  function run(entry: RankedCommand): void {
    void runCommand(entry.command, ctx);
    close();
  }

  function onInputKeydown(event: KeyboardEvent): void {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const delta = event.key === "ArrowDown" ? 1 : -1;
      activeIndex = Math.min(flat.length - 1, Math.max(0, activeIndex + delta));
    } else if (event.key === "Home") {
      event.preventDefault();
      activeIndex = 0;
    } else if (event.key === "End") {
      event.preventDefault();
      activeIndex = Math.max(0, flat.length - 1);
    } else if (event.key === "Enter") {
      const entry = flat[activeIndex];
      if (entry) {
        event.preventDefault();
        run(entry);
      }
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  }

  /** Escape on the (never-focused) scrim still closes — defense in depth. */
  function onScrimKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  }

  /** Typed keys stay in the combobox; Escape keeps bubbling to the scrim. */
  function onPanelKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape") event.stopPropagation();
  }

  /** Header label for a section boundary in the flat list. */
  function sectionFor(index: number): PaletteSection | null {
    let seen = 0;
    for (const section of sections) {
      if (index >= seen && index < seen + section.commands.length) {
        return section;
      }
      seen += section.commands.length;
    }
    return null;
  }

  function isFirstOfSection(index: number): boolean {
    return sectionFor(index - 1)?.name !== sectionFor(index)?.name;
  }
</script>

<svelte:window onkeydown={onScrimKeydown} />

{#if open}
  <div class="scrim" role="presentation" onclick={close}>
    <div
      class="panel"
      role="dialog"
      aria-modal="true"
      aria-label="Command palette"
      tabindex="-1"
      onclick={(event) => event.stopPropagation()}
      onkeydown={onPanelKeydown}
    >
      <input
        class="search"
        type="text"
        role="combobox"
        aria-expanded="true"
        aria-controls="palette-list"
        aria-activedescendant={flat.length > 0 ? optionId(activeIndex) : undefined}
        aria-label="Search commands"
        aria-autocomplete="list"
        placeholder="Type a command… ({tabStore.tabs.length > 0 ? tabStore.active?.name : 'no repository'})"
        bind:value={query}
        bind:this={inputEl}
        onkeydown={onInputKeydown}
      />
      <ul id="palette-list" class="list" role="listbox" aria-label="Commands">
        {#each flat as entry, i (entry.command.id)}
          {@const hits = titleHits(entry, query)}
          {#if isFirstOfSection(i)}
            <li class="section" role="presentation">{sectionFor(i)?.name}</li>
          {/if}
          <li
            id={optionId(i)}
            class="option"
            class:active={i === activeIndex}
            role="option"
            aria-selected={i === activeIndex}
            onclick={() => run(entry)}
            onkeydown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                run(entry);
              }
            }}
            onmouseenter={() => (activeIndex = i)}
          >
            <span class="label">
              {#each entry.command.title as ch, j (j)}<span class:hl={hits.has(j)}>{ch}</span>{/each}
            </span>
            {#if hint(entry.command) !== null}
              <kbd class="shortcut">{hint(entry.command)}</kbd>
            {/if}
          </li>
        {/each}
        {#if flat.length === 0}
          <li class="none" role="option" aria-disabled="true" aria-selected="false">
            No commands match “{query}”
          </li>
        {/if}
      </ul>
      <footer class="hintbar">↑↓ navigate · ↵ run · esc close</footer>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 70;
    background: color-mix(in srgb, var(--m3-scrim, #000) 32%, transparent);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 10vh;
  }

  .panel {
    width: min(36rem, calc(100vw - 2rem));
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
    padding: 0 0.5rem 0.5rem;
    max-height: 55vh;
    overflow-y: auto;
  }

  .section {
    padding: 0.625rem 0.75rem 0.25rem;
    font-size: 0.6875rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--m3-primary);
    user-select: none;
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
    flex: 1;
    min-width: 0;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .label .hl {
    color: var(--m3-primary);
    font-weight: 700;
  }

  .shortcut {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: 0.375rem;
    padding: 0.0625rem 0.375rem;
  }

  .none {
    padding: 1rem 0.75rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.8125rem;
  }

  .hintbar {
    padding: 0.5rem 1rem 0.75rem;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }
</style>
