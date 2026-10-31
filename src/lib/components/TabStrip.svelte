<script lang="ts">
  /**
   * Repository tab strip (M1). Bound to the global tab store — no props
   * except `openFolder` (the "+" button callback) and an optional `trailing`
   * snippet for window-level utilities (AI health, theme toggle).
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
  import type { Snippet } from "svelte";
  import {
    closeTab,
    setActive,
    showRecentRepos,
    tabStore,
  } from "$lib/stores/tabs.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Button } from "$lib/components/ui/button/index.js";

  let {
    openFolder,
    trailing = undefined,
  }: {
    openFolder?: () => void;
    /** Window-level controls rendered at the right edge of the strip. */
    trailing?: Snippet;
  } = $props();

  const iconHome = $derived(appIcon("home-rounded")!);

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
      // Middle click closes; prevent the browser's autoscroll affordance.
      event.preventDefault();
      void closeTab(tabId);
    }
  }
</script>

<div class="tabbar flex min-w-0 flex-none items-stretch border-b border-border bg-card">
  <Button
    class="tab-home ml-2 size-[1.9rem] flex-none self-center rounded-lg aria-[pressed=true]:bg-secondary aria-[pressed=true]:text-secondary-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring focus-visible:ring-0"
    variant="ghost"
    size="icon-sm"
    type="button"
    aria-label="Recent repositories"
    aria-pressed={tabStore.recentsOpen}
    title="Recent repositories"
    onclick={showRecentRepos}
  >
    <Icon icon={iconHome} size={17} />
  </Button>
  <div
    class="tabstrip flex min-w-0 flex-1 items-stretch gap-0.5 overflow-x-auto px-2 pt-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
    role="tablist"
    aria-label="Open repositories"
    tabindex="-1"
    bind:this={strip}
    onkeydown={onKeydown}
  >
    {#each tabStore.tabs as tab (tab.id)}
      {@const isActive = tab.id === tabStore.activeId}
      <div
        class="tab flex min-w-0 max-w-64 items-stretch rounded-t-lg border-b-2 border-transparent transition-colors"
        class:active={isActive}
        class:bg-accent={isActive}
        class:border-b-primary={isActive}
      >
        <!-- font-weight stays constant across active/inactive: a
             selected-state weight bump widens the label, resizing the tab
             and shifting every sibling tab. -->
        <button
          class="tab-main min-w-0 cursor-pointer select-none overflow-hidden text-ellipsis whitespace-nowrap rounded-t-lg border-0 bg-transparent py-2 pl-3.5 pr-1 text-sm font-medium text-muted-foreground outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
          class:text-primary={isActive}
          type="button"
          role="tab"
          aria-selected={isActive}
          tabindex={isActive ? 0 : -1}
          title={tab.root}
          aria-label={`${tab.name} — ${tab.root}`}
          onclick={() => setActive(tab.id)}
          onauxclick={(event) => onAuxClick(event, tab.id)}
          onmousedown={(event) => {
            if (event.button === 1) event.preventDefault();
          }}
        >
          <span class="name block overflow-hidden text-ellipsis whitespace-nowrap">{tab.name}</span>
        </button>
        <Button
          class="tab-close ml-0.5 h-auto flex-none rounded-tr-lg px-2 text-base leading-none focus-visible:bg-accent focus-visible:text-foreground"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label="Close {tab.name}"
          title="Close {tab.name}"
          onclick={() => void closeTab(tab.id)}
        >
          ×
        </Button>
      </div>
    {/each}

    <Button
      class="tab-new my-0.5 h-auto flex-none px-3 text-base leading-none hover:text-primary focus-visible:bg-accent focus-visible:text-primary"
      variant="ghost"
      size="sm"
      type="button"
      aria-label="Open a repository"
      title="Open a repository"
      onclick={() => openFolder?.()}
    >
      +
    </Button>
  </div>

  {#if trailing}
    <div class="tabbar-trailing m-1 flex flex-none items-center gap-1 border-l border-border px-2">{@render trailing()}</div>
  {/if}
</div>
