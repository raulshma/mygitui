<!--
  ContextMenu (M9 F1) — the single global right-click menu.

  Backed by `contextMenu.svelte.ts`; mounted once in App.svelte. Keyboard:
  ArrowUp/Down move, Home/End jump, Enter/Space run, Escape closes; focus
  returns to the opener via `document.activeElement` restore. Clicking
  elsewhere closes. Position clamps to the viewport.
-->
<script lang="ts">
  import Icon from "$lib/components/icons/Icon.svelte";
  import {
    contextMenu,
    isSeparator,
    type MenuEntry,
    type MenuItem,
  } from "./contextMenuStore.svelte";

  let listEl: HTMLUListElement | undefined = $state();
  let active = $state(-1);
  let restoreFocusTo: Element | null = null;

  const selectable = (): number[] =>
    contextMenu.entries.flatMap((entry, index): number[] =>
      isSeparator(entry) || entry.disabled ? [] : [index],
    );

  function close(): void {
    contextMenu.hide();
    active = -1;
    if (restoreFocusTo instanceof HTMLElement) restoreFocusTo.focus();
    restoreFocusTo = null;
  }

  function run(item: MenuItem): void {
    close();
    item.run();
  }

  function onWindowPointerDown(event: PointerEvent): void {
    if (!contextMenu.open) return;
    if (event.target instanceof Node && listEl?.contains(event.target)) return;
    close();
  }

  function onKeydown(event: KeyboardEvent): void {
    if (!contextMenu.open) return;
    const enabled = selectable();
    if (event.key === "Escape") {
      event.preventDefault();
      close();
      return;
    }
    if (enabled.length === 0) return;
    const position = enabled.indexOf(active);
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        active = enabled[(position + 1 + enabled.length) % enabled.length] ?? enabled[0]!;
        focusActive();
        break;
      case "ArrowUp":
        event.preventDefault();
        active =
          enabled[(position - 1 + enabled.length) % enabled.length] ??
          enabled[enabled.length - 1]!;
        focusActive();
        break;
      case "Home":
        event.preventDefault();
        active = enabled[0]!;
        focusActive();
        break;
      case "End":
        event.preventDefault();
        active = enabled[enabled.length - 1]!;
        focusActive();
        break;
      case "Enter":
      case " ":
        event.preventDefault();
        if (position >= 0) {
          const item = contextMenu.entries[active];
          if (item && !isSeparator(item)) run(item);
        }
        break;
    }
  }

  /** Per-item activation (click or the item-local Enter/Space). */
  function activate(entry: MenuEntry, event: Event): void {
    event.preventDefault();
    if (!isSeparator(entry) && !entry.disabled) run(entry);
  }

  function focusActive(): void {
    const node = listEl?.querySelector<HTMLLIElement>(
      `[data-index="${active}"]`,
    );
    node?.focus();
  }

  $effect(() => {
    if (contextMenu.open) {
      restoreFocusTo = document.activeElement;
      const first = selectable()[0];
      active = first ?? -1;
      queueMicrotask(() => listEl?.focus());
    }
  });

  $effect(() => {
    // Clamp the position after content renders (menu may exceed the viewport
    // near edges/bottom).
    if (!contextMenu.open || !listEl) return;
    const rect = listEl.getBoundingClientRect();
    if (rect.right > window.innerWidth) {
      contextMenu.move(window.innerWidth - rect.width - 8, contextMenu.y);
    }
    if (rect.bottom > window.innerHeight) {
      contextMenu.move(contextMenu.x, window.innerHeight - rect.height - 8);
    }
  });
</script>

<svelte:window
  onpointerdown={onWindowPointerDown}
  onkeydown={onKeydown}
  onblur={close}
/>

{#if contextMenu.open}
  <ul
    bind:this={listEl}
    class="menu"
    role="menu"
    tabindex="-1"
    aria-label="Context menu"
    style:left="{contextMenu.x}px"
    style:top="{contextMenu.y}px"
  >
    {#each contextMenu.entries as entry, index}
      {#if isSeparator(entry)}
        <li role="separator" class="sep"></li>
      {:else}
        <li
          role="menuitem"
          data-index={index}
          class:entry
          class:danger={entry.danger}
          class:disabled={entry.disabled}
          tabindex="-1"
          aria-disabled={entry.disabled}
          onclick={(event) => activate(entry, event)}
          onkeydown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              activate(entry, event);
            }
          }}
          onpointerenter={() => {
            if (!entry.disabled) active = index;
          }}
        >
          {#if entry.icon}
            <span class="icon" aria-hidden="true">
              <Icon icon={entry.icon} />
            </span>
          {/if}
          {entry.label}
        </li>
      {/if}
    {/each}
  </ul>
{/if}

<style>
  .menu {
    position: fixed;
    z-index: 1000;
    min-width: 180px;
    max-width: 320px;
    margin: 0;
    padding: 0.25rem;
    list-style: none;
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    border: 1px solid var(--m3-outline-variant, transparent);
    border-radius: var(--m3-shape-medium, 12px);
    box-shadow: var(--m3-elevation-2, 0 4px 12px rgb(0 0 0 / 0.3));
  }

  .entry {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4rem 0.75rem;
    border-radius: var(--m3-shape-small, 8px);
    font: var(--m3-body-medium, 0.875rem/1.4 sans-serif);
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .icon {
    flex: none;
    display: inline-flex;
  }

  .entry:focus-visible,
  .entry:focus {
    outline: none;
    background: var(--m3-secondary-container, var(--m3-surface-container-highest, inherit));
  }

  .entry.danger {
    color: var(--m3-error);
  }

  .entry.disabled {
    opacity: 0.45;
    cursor: default;
  }

  .sep {
    height: 1px;
    margin: 0.25rem 0.5rem;
    background: var(--m3-outline-variant, currentColor);
    opacity: 0.4;
  }
</style>
