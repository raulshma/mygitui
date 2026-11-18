<script lang="ts">
  /**
   * Keyboard-shortcuts cheatsheet overlay — opened by pressing `?` (the
   * keybind engine's `help.shortcuts`, default combo `shift+?` — pressing ?
   * produces `event.key === "?"` with Shift held) or from the palette
   * command of the same id.
   *
   * Read-only by design — rebinding lives in Settings' Keyboard shortcuts
   * section. Both surfaces derive their list from the SAME helper
   * (`shortcutSections.ts`: registry sections × effective bindings), so the
   * cheatsheet can never drift from the editor. Unbound commands are listed
   * too (they are rebindable in Settings), shown with an em dash.
   *
   * Overlay shape mirrors the command palette: a scrim that closes on
   * click, a dialog panel with a filter input (autofocused), grouped
   * entries with `Kbd` hints, Escape to dismiss (deferred to overlays above
   * it via the shared overlay stack), and focus returned to the trigger
   * element on close.
   */
  import { overlayStack } from "$lib/state/overlays.svelte";
  import { t } from "$lib/i18n";
  import { currentBindings } from "$lib/palette/keybinds";
  import {
    matchesShortcutQuery,
    shortcutSections,
    type ShortcutSection,
  } from "$lib/palette/shortcutSections";
  import { closeShortcuts, isShortcutsOpen } from "./shortcuts.svelte";
  import { trapTab } from "$lib/components/safety/trapTab";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Kbd } from "$lib/components/ui/kbd/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";

  const open = $derived(isShortcutsOpen());

  let query = $state("");
  let inputEl: HTMLInputElement | null = $state(null);
  let panelEl: HTMLElement | null = $state(null);
  /** Element focused before the overlay opened; focus returns there on close. */
  let triggerEl: HTMLElement | null = null;

  const searchIcon = $derived(appIcon("search"));

  /**
   * The grouped list, snapshotted once per open (bindings may change
   * mid-session via the Settings editor; the overlay re-mounts per open,
   * so a snapshot at open time is always current). Filtered per keystroke.
   */
  let sections = $state<ShortcutSection[]>([]);
  // Opening snapshots the bindings and resets the session.
  $effect(() => {
    if (!open) return;
    triggerEl =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    sections = shortcutSections(currentBindings());
    query = "";
    inputEl?.focus();
  });

  const filtered = $derived(
    sections
      .map((section) => ({
        ...section,
        entries: section.entries.filter((entry) => matchesShortcutQuery(entry, query)),
      }))
      .filter((section) => section.entries.length > 0),
  );
  const matchCount = $derived(
    filtered.reduce((sum, section) => sum + section.entries.length, 0),
  );

  function close(): void {
    closeShortcuts();
    // Return focus to the trigger (keybind source element, button, …).
    triggerEl?.focus();
    triggerEl = null;
  }

  /** Escape on the (never-focused) scrim; defers to overlays above. */
  function onScrimKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || !overlayStack.isTop("shortcuts")) return;
    event.preventDefault();
    close();
  }

  /** Typed keys stay in the filter; Tab is trapped; Escape keeps bubbling. */
  function onPanelKeydown(event: KeyboardEvent): void {
    trapTab(panelEl, event);
    if (event.key !== "Escape") event.stopPropagation();
  }
</script>

<svelte:window onkeydown={onScrimKeydown} />

{#if open}
  <div
    class="scrim fixed inset-0 z-70 flex justify-center items-start pt-[10vh] bg-black/50"
    role="presentation"
    onclick={close}
  >
    <div
      class="panel flex flex-col overflow-hidden w-[min(40rem,calc(100vw-2rem))] max-h-[75vh] bg-popover text-popover-foreground rounded-xl shadow-lg"
      role="dialog"
      aria-modal="true"
      aria-label={t("shortcuts.title")}
      tabindex="-1"
      bind:this={panelEl}
      onclick={(event) => event.stopPropagation()}
      onkeydown={onPanelKeydown}
    >
      <header class="flex flex-none items-center gap-3 border-b border-border px-4 py-2.5">
        <h2 class="m-0 shrink-0 text-sm font-semibold text-foreground">{t("shortcuts.title")}</h2>
        <span class="min-w-0 flex-1 text-ui-meta text-muted-foreground">
          Rebind in Settings → Keyboard shortcuts
        </span>
        <div class="relative flex w-44 flex-none items-center">
          {#if searchIcon}
            <span class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-muted-foreground">
              <Icon icon={searchIcon} size={13} />
            </span>
          {/if}
          <Input
            class="h-7 w-full rounded-full border-0 bg-muted pl-7 pr-2 text-ui-meta shadow-none ring-0 focus-visible:border-0 focus-visible:ring-2 focus-visible:ring-ring/50"
            type="text"
            placeholder="Filter…"
            aria-label="Filter shortcuts"
            bind:value={query}
            bind:ref={inputEl}
            onkeydown={(event) => {
              // Escape clears the filter first; the scrim handles closing.
              if (event.key === "Escape" && query !== "") {
                event.preventDefault();
                event.stopPropagation();
                query = "";
              }
            }}
          />
        </div>
      </header>
      <div class="list min-h-0 flex-1 overflow-y-auto px-2 pb-3">
        {#if matchCount === 0}
          <p class="m-0 px-3 py-4 text-ui-base text-muted-foreground">
            No shortcuts match “{query}”.
          </p>
        {:else}
          {#each filtered as section (section.name)}
            <section class="flex flex-col" aria-label={`${section.name} commands`}>
              <h3 class="sticky top-0 z-1 m-0 bg-popover px-3 pt-2.5 pb-1 text-ui-micro font-bold uppercase tracking-[0.08em] text-primary select-none">
                {section.name}
              </h3>
              <ul class="m-0 list-none p-0">
                {#each section.entries as entry (entry.command.id)}
                  <li class="flex min-w-0 items-center gap-2 rounded-lg px-3 py-1.5 text-sm">
                    <span class="min-w-0 flex-1 truncate text-foreground" title={entry.command.title}>
                      {entry.command.title}
                    </span>
                    {#if entry.combo !== null}
                      <Kbd class="shrink-0">{entry.display}</Kbd>
                    {:else}
                      <span class="shrink-0 text-ui-meta text-muted-foreground" aria-label="Unbound">—</span>
                    {/if}
                  </li>
                {/each}
              </ul>
            </section>
          {/each}
        {/if}
      </div>
      <footer class="flex flex-none items-center justify-between gap-3 border-t border-border px-4 pt-2 pb-3 text-ui-micro text-muted-foreground">
        <span>{matchCount} {matchCount === 1 ? "command" : "commands"}</span>
        <span aria-hidden="true">esc close</span>
      </footer>
    </div>
  </div>
{/if}
