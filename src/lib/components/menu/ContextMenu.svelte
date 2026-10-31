<!--
  ContextMenu (M9 F1) — the single global right-click menu.

  State lives in `contextMenuStore.svelte` (unchanged public API); this
  component renders it through bits-ui's DropdownMenu primitives in
  controlled mode (`open` follows the store). A virtual anchor at the
  stored pointer position replaces a real trigger: the floating engine
  positions the menu there and collision-flips/clamps it to the viewport
  (replacing the old manual clamp pass). Keyboard: ArrowUp/Down move,
  Home/End jump, Enter/Space run, Escape closes — all handled by the
  primitive; focus returns to the opener via the captured
  `document.activeElement`. Clicking elsewhere closes (the layer's
  outside-pointerdown behavior); a window blur closes too.
-->
<script lang="ts">
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import {
    contextMenu,
    isSeparator,
    type MenuItem,
  } from "./contextMenuStore.svelte";

  let restoreFocusTo: Element | null = null;

  /** Virtual anchor at the store's pointer position (measured lazily). */
  const anchor = {
    getBoundingClientRect: () => ({
      x: contextMenu.x,
      y: contextMenu.y,
      top: contextMenu.y,
      left: contextMenu.x,
      bottom: contextMenu.y,
      right: contextMenu.x,
      width: 0,
      height: 0,
      toJSON: () => ({}),
    }),
  };

  function close(): void {
    contextMenu.hide();
    if (restoreFocusTo instanceof HTMLElement) restoreFocusTo.focus();
    restoreFocusTo = null;
  }

  /** Item activation: close first (restore opener focus), then run. */
  function run(item: MenuItem): void {
    close();
    item.run();
  }

  /** Every bits-ui close path (outside press, Escape, selection) lands here. */
  function onOpenChange(next: boolean): void {
    if (!next) close();
  }

  /**
   * Focus stays with the opener; the primitive's own auto-focus (it would
   * target the non-existent trigger) is suppressed.
   */
  function onCloseAutoFocus(event: Event): void {
    event.preventDefault();
    if (restoreFocusTo instanceof HTMLElement) restoreFocusTo.focus();
    restoreFocusTo = null;
  }

  /** Remember the opener whenever the menu opens. */
  $effect(() => {
    if (contextMenu.open) {
      restoreFocusTo = document.activeElement;
    }
  });

  /** Entry-local trailing action (e.g. pin); does not trigger the entry. */
  function runAction(entry: MenuItem, event: Event): void {
    event.preventDefault();
    event.stopPropagation();
    if (entry.disabled || !entry.action) return;
    close();
    entry.action.run();
  }
</script>

<svelte:window onblur={() => contextMenu.open && close()} />

<DropdownMenu.Root open={contextMenu.open} {onOpenChange}>
  <DropdownMenu.Content
    customAnchor={anchor}
    class="menu z-[1000] w-auto min-w-[180px] max-w-[320px]"
    {onCloseAutoFocus}
  >
    {#each contextMenu.entries as entry, index (index)}
      {#if isSeparator(entry)}
        <DropdownMenu.Separator />
      {:else}
        <DropdownMenu.Item
          variant={entry.danger ? "destructive" : "default"}
          disabled={entry.disabled}
          onSelect={() => run(entry)}
        >
          {#if entry.icon}
            <span class="icon inline-flex shrink-0" aria-hidden="true">
              <Icon icon={entry.icon} />
            </span>
          {/if}
          <span class="label min-w-0 flex-1 overflow-hidden text-ellipsis">{entry.label}</span>
          {#if entry.action}
            <button
              type="button"
              class="entry-action -mr-[0.35rem] inline-flex h-6 w-6 shrink-0 cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent p-0 text-xs text-inherit [font:inherit] opacity-65 hover:bg-accent hover:opacity-100 focus-visible:bg-accent focus-visible:opacity-100 focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-ring group-data-[disabled]/dropdown-menu-item:pointer-events-none group-data-[disabled]/dropdown-menu-item:opacity-30"
              title={entry.action.label}
              aria-label={entry.action.label}
              onclick={(event) => runAction(entry, event)}
              onkeydown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  runAction(entry, event);
                }
              }}
            >
              {#if entry.action.icon}
                <Icon icon={entry.action.icon} />
              {:else}
                {entry.action.label}
              {/if}
            </button>
          {/if}
        </DropdownMenu.Item>
      {/if}
    {/each}
  </DropdownMenu.Content>
</DropdownMenu.Root>
