<script lang="ts">
  /**
   * Repository tab strip (M1). Bound to the global tab store — no props
   * except `openFolder`, the "+" button callback (App wires it to the
   * placeholder folder-picker toast until the dialog plugin lands in M1.1).
   *
 * Accessibility: the strip is a `tablist` with a roving tabindex — the
 * active tab is tab-reachable (tabindex 0), the rest are programmatic
 * (tabindex -1). Arrow/Home/End keys move focus; Enter/Space activate the
 * focused tab (native button behavior routes through `setActive`).
 * Middle-click (auxclick button 1) closes a tab, as does its × button.
 *
 * Focus order is sane: each × close button follows its tab in DOM order, so
 * Tab runs tab → its close → next reachable control. Overflowed (ellipsis-
 * truncated) names announce in full: each tab's accessible name is
 * "name — root" (label-in-name compliant) and the title tooltip shows the
 * root on hover.
 */
  import { closeTab, setActive, tabStore } from "$lib/stores/tabs.svelte";

  let { openFolder }: { openFolder?: () => void } = $props();

  let strip: HTMLDivElement | undefined = $state();

  function tabButtons(): HTMLButtonElement[] {
    if (!strip) return [];
    return Array.from(
      strip.querySelectorAll<HTMLButtonElement>("button[role='tab']"),
    );
  }

  function onKeydown(event: KeyboardEvent): void {
    const buttons = tabButtons();
    if (buttons.length === 0) return;
    const current = buttons.findIndex((button) => button === document.activeElement);

    let next: number;
    if (event.key === "ArrowRight") {
      next = current === -1 ? 0 : (current + 1) % buttons.length;
    } else if (event.key === "ArrowLeft") {
      next = current === -1 ? buttons.length - 1 : (current - 1 + buttons.length) % buttons.length;
    } else if (event.key === "Home") {
      next = 0;
    } else if (event.key === "End") {
      next = buttons.length - 1;
    } else {
      return; // Enter/Space flow through the native button click.
    }

    event.preventDefault();
    buttons[next]?.focus();
  }

  function onAuxClick(event: MouseEvent, tabId: string): void {
    if (event.button === 1) {
      // Middle-click closes; prevent the browser's autoscroll affordance.
      event.preventDefault();
      void closeTab(tabId);
    }
  }
</script>

<div
  class="tabstrip"
  role="tablist"
  aria-label="Open repositories"
  tabindex="-1"
  bind:this={strip}
  onkeydown={onKeydown}
>
  {#each tabStore.tabs as tab (tab.id)}
    <div class="tab" class:active={tab.id === tabStore.activeId}>
      <button
        class="tab-main"
        type="button"
        role="tab"
        aria-selected={tab.id === tabStore.activeId}
        tabindex={tab.id === tabStore.activeId ? 0 : -1}
        title={tab.root}
        aria-label={`${tab.name} — ${tab.root}`}
        onclick={() => setActive(tab.id)}
        onauxclick={(event) => onAuxClick(event, tab.id)}
        onmousedown={(event) => {
          if (event.button === 1) event.preventDefault();
        }}
      >
        <span class="name">{tab.name}</span>
      </button>
      <button
        class="tab-close"
        type="button"
        aria-label="Close {tab.name}"
        title="Close {tab.name}"
        onclick={() => void closeTab(tab.id)}
      >
        ×
      </button>
    </div>
  {/each}

  <button
    class="tab-new"
    type="button"
    aria-label="Open a repository"
    title="Open a repository (folder picker lands in M1.1)"
    onclick={() => openFolder?.()}
  >
    +
  </button>
</div>

<style>
  .tabstrip {
    display: flex;
    align-items: stretch;
    gap: 0.125rem;
    padding: 0.25rem 0.5rem 0;
    background: var(--m3-surface-container, var(--m3-surface));
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    flex: none;
    min-width: 0;
  }

  .tab {
    display: flex;
    align-items: stretch;
    min-width: 0;
    max-width: 16rem;
    border-bottom: 2px solid transparent;
    border-radius: 0.5rem 0.5rem 0 0;
  }

  .tab.active {
    border-bottom-color: var(--m3-primary);
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .tab-main {
    min-width: 0;
    border: none;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.875rem;
    /* Constant across active/inactive: a selected-state weight bump widens
       the label, resizing the tab and shifting every sibling tab. */
    font-weight: 500;
    padding: 0.5rem 0.25rem 0.5rem 0.875rem;
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tab.active .tab-main {
    color: var(--m3-primary);
  }

  .tab-main:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
    border-radius: 0.5rem 0.5rem 0 0;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tab-close {
    flex: none;
    border: none;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    padding: 0 0.5rem;
    margin-left: 0.125rem;
    font-size: 1rem;
    line-height: 1;
    cursor: pointer;
    border-radius: 0 0.5rem 0 0;
  }

  .tab-close:hover,
  .tab-close:focus-visible {
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-highest, var(--m3-surface));
  }

  .tab-new {
    flex: none;
    border: none;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 1rem;
    line-height: 1;
    padding: 0.5rem 0.75rem;
    cursor: pointer;
    border-radius: 0.5rem;
    margin: 0.125rem 0;
  }

  .tab-new:hover,
  .tab-new:focus-visible {
    color: var(--m3-primary);
    background: var(--m3-surface-container-high, var(--m3-surface));
  }
</style>
