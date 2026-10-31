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
import { commandIcon } from "$lib/palette/commandIcons";
import Icon from "$lib/components/icons/Icon.svelte";
import { Input } from "$lib/components/ui/input/index.js";
import { Kbd } from "$lib/components/ui/kbd/index.js";

  const open = $derived(isPaletteOpen());
  const ctx = $derived(activeCtx());

  let query = $state("");
  let activeIndex = $state(0);
  let inputEl: HTMLInputElement | null = $state(null);
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
  <div
    class="scrim fixed inset-0 z-[70] flex justify-center items-start pt-[10vh] bg-black/50"
    role="presentation"
    onclick={close}
  >
    <div
      class="panel flex flex-col overflow-hidden w-[min(36rem,calc(100vw-2rem))] bg-popover text-popover-foreground rounded-xl shadow-lg"
      role="dialog"
      aria-modal="true"
      aria-label="Command palette"
      tabindex="-1"
      onclick={(event) => event.stopPropagation()}
      onkeydown={onPanelKeydown}
    >
      <Input
        class="search m-3 h-auto rounded-full border-0 bg-muted px-4 py-2.5 text-base shadow-none ring-0 focus-visible:border-0 focus-visible:ring-2 focus-visible:ring-ring md:text-base"
        type="text"
        role="combobox"
        aria-expanded="true"
        aria-controls="palette-list"
        aria-activedescendant={flat.length > 0 ? optionId(activeIndex) : undefined}
        aria-label="Search commands"
        aria-autocomplete="list"
        placeholder="Type a command… ({tabStore.tabs.length > 0 ? tabStore.active?.name : 'no repository'})"
        bind:value={query}
        bind:ref={inputEl}
        onkeydown={onInputKeydown}
      />
      <ul
        id="palette-list"
        class="list list-none m-0 px-2 pb-2 max-h-[55vh] overflow-y-auto"
        role="listbox"
        aria-label="Commands"
      >
        {#each flat as entry, i (entry.command.id)}
          {@const hits = titleHits(entry, query)}
          {@const cmdIcon = commandIcon(entry.command)}
          {#if isFirstOfSection(i)}
            <li
              class="section px-3 pt-2.5 pb-1 text-[0.6875rem] font-bold uppercase tracking-[0.08em] text-primary select-none"
              role="presentation"
            >{sectionFor(i)?.name}</li>
          {/if}
          <li
            id={optionId(i)}
            class="option flex items-center gap-2 px-3 py-2 rounded-full cursor-pointer min-w-0"
            class:active={i === activeIndex}
            class:bg-accent={i === activeIndex}
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
            {#if cmdIcon}
              <span
                class="cmd-icon shrink-0 inline-flex items-center justify-center size-4 text-muted-foreground"
                class:text-accent-foreground={i === activeIndex}
                aria-hidden="true"
              >
                <Icon icon={cmdIcon} size={15} />
              </span>
            {/if}
            <span class="label flex-1 min-w-0 font-medium whitespace-nowrap overflow-hidden text-ellipsis">
              {#each entry.command.title as ch, j (j)}<span
                  class:hl={hits.has(j)}
                  class:text-primary={hits.has(j)}
                  class:font-bold={hits.has(j)}
                >{ch}</span>{/each}
            </span>
            {#if hint(entry.command) !== null}
              <Kbd
                class="shortcut h-auto shrink-0 rounded-md border border-border px-1.5 py-0.5 font-mono text-[0.6875rem]"
              >{hint(entry.command)}</Kbd>
            {/if}
          </li>
        {/each}
        {#if flat.length === 0}
          <li
            class="none px-3 py-4 text-muted-foreground text-[0.8125rem]"
            role="option"
            aria-disabled="true"
            aria-selected="false"
          >
            No commands match “{query}”
          </li>
        {/if}
      </ul>
      <footer class="hintbar px-4 pt-2 pb-3 border-t border-border text-muted-foreground text-[0.6875rem]">↑↓ navigate · ↵ run · esc close</footer>
    </div>
  </div>
{/if}
