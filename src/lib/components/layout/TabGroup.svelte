<script lang="ts">
  /**
   * TabGroup — renders a `{ kind: "tabs" }` layout leaf: the tab strip
   * (same visual + keyboard pattern as the pre-M4 RepoView tablist),
   * the active panel's body, a "+ Add panel" menu listing hidden panels,
   * and a right-click context menu per tab ("Move to right group" /
   * "New group to the right" / "Hide").
   *
   * Pure presentation: every action is delegated to callbacks owned by
   * RepoView (which applies them through the layout store). Component
   * rendering is not unit testable, so behavior tests live in the layout
   * model/store suites.
   */
  import type { Snippet } from "svelte";
  import {
    PANEL_META,
    panelLabel,
    type PanelGroupAction,
    type PanelId,
    type TabsNode,
  } from "$lib/layout/layoutModel";

  let {
    node,
    renderPanel,
    hiddenPanels,
    canMoveRight,
    onActivate,
    onAdd,
    onAction,
  }: {
    node: TabsNode;
    /** Snippet factory: PanelId → the panel's rendered content. */
    renderPanel: (id: PanelId) => Snippet;
    /** Panels currently not in the tree (candidates for "+ Add panel"). */
    hiddenPanels: PanelId[];
    /** Whether the holder group has a group to its right. */
    canMoveRight: boolean;
    onActivate: (tabsId: string, index: number) => void;
    onAdd: (tabsId: string, panel: PanelId) => void;
    onAction: (action: PanelGroupAction, tabsId: string, panel: PanelId) => void;
  } = $props();

  /** Active index clamped against the current tabs (defensive vs overlays). */
  const activeIndex = $derived(
    node.tabs.length === 0
      ? 0
      : Math.min(Math.max(node.active, 0), node.tabs.length - 1),
  );
  const activePanel = $derived(node.tabs[activeIndex]);

  // -- popovers (context menu + add menu) ----------------------------------

  type Menu =
    | { kind: "context"; x: number; y: number; panel: PanelId }
    | { kind: "add"; x: number; y: number };

  let menu = $state<Menu | null>(null);
  let rootEl = $state<HTMLDivElement | undefined>(undefined);
  let menuEl = $state<HTMLDivElement | undefined>(undefined);

  const MENU_WIDTH = 200;

  $effect(() => {
    if (!menu) return;
    const close = (event: PointerEvent): void => {
      const path = event.composedPath();
      // Keep open when pressing inside the group OR the open menu — the
      // menu's own buttons still receive their click afterwards.
      if (rootEl && path.includes(rootEl)) return;
      if (menuEl && path.includes(menuEl)) return;
      menu = null;
    };
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") menu = null;
    };
    document.addEventListener("pointerdown", close, true);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", close, true);
      document.removeEventListener("keydown", onKey);
    };
  });

  function openContext(event: MouseEvent, panel: PanelId): void {
    if (event.button !== 0 && event.button !== 2) return;
    event.preventDefault();
    menu = {
      kind: "context",
      x: Math.min(event.clientX, window.innerWidth - MENU_WIDTH - 8),
      y: Math.min(event.clientY, window.innerHeight - 130),
      panel,
    };
  }

  function openAdd(event: MouseEvent): void {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = {
      kind: "add",
      x: Math.min(rect.left, window.innerWidth - MENU_WIDTH - 8),
      y: rect.bottom + 4,
    };
  }

  function runAction(action: PanelGroupAction): void {
    if (menu?.kind !== "context") return;
    const { panel } = menu;
    menu = null;
    onAction(action, node.id, panel);
  }

  function runAdd(panel: PanelId): void {
    menu = null;
    onAdd(node.id, panel);
  }

  // -- keyboard tablist ------------------------------------------------------

  function onTablistKeydown(event: KeyboardEvent): void {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    if (node.tabs.length === 0) return;
    event.preventDefault();
    const dir = event.key === "ArrowRight" ? 1 : -1;
    const next = (activeIndex + dir + node.tabs.length) % node.tabs.length;
    onActivate(node.id, next);
    // Move focus to the newly selected tab button.
    requestAnimationFrame(() => {
      rootEl
        ?.querySelector<HTMLButtonElement>(`button[data-tab-index="${next}"]`)
        ?.focus();
    });
  }
</script>

<div class="tabgroup" bind:this={rootEl}>
  <div
    class="panel-tabs"
    role="tablist"
    aria-label="Panels"
    tabindex="-1"
    onkeydown={onTablistKeydown}
    oncontextmenu={(e) => e.preventDefault()}
  >
    {#each node.tabs as panel, index (panel)}
      <button
        data-tab-index={index}
        class="panel-tab"
        type="button"
        role="tab"
        aria-selected={index === activeIndex}
        aria-controls={`tabpanel-${node.id}`}
        id={`tab-${node.id}-${panel}`}
        tabindex={index === activeIndex ? 0 : -1}
        onclick={() => onActivate(node.id, index)}
        oncontextmenu={(e) => openContext(e, panel)}
      >
        <svg
          class="tab-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d={PANEL_META[panel]?.icon ?? ""} />
        </svg>
        {panelLabel(panel)}
      </button>
    {/each}

    {#if hiddenPanels.length > 0}
      <button
        class="panel-add"
        type="button"
        aria-label="Add panel"
        aria-haspopup="menu"
        aria-expanded={menu?.kind === "add"}
        title="Add panel"
        onclick={openAdd}
      >
        +
      </button>
    {/if}
  </div>

  <div
    class="panel-body"
    id={`tabpanel-${node.id}`}
    role="tabpanel"
    aria-labelledby={activePanel ? `tab-${node.id}-${activePanel}` : undefined}
  >
    {#if activePanel}
      {@render renderPanel(activePanel)()}
    {/if}
  </div>
</div>

{#if menu}
  <div
    class="menu"
    role="menu"
    bind:this={menuEl}
    style:left={`${menu.x}px`}
    style:top={`${menu.y}px`}
  >
    {#if menu.kind === "context"}
      <button
        class="menu-item"
        type="button"
        role="menuitem"
        disabled={!canMoveRight}
        onclick={() => runAction("move-right-group")}
      >
        Move to right group
      </button>
      <button
        class="menu-item"
        type="button"
        role="menuitem"
        onclick={() => runAction("new-group-right")}
      >
        New group to the right
      </button>
      <button
        class="menu-item danger"
        type="button"
        role="menuitem"
        onclick={() => runAction("hide")}
      >
        Hide
      </button>
    {:else}
      {#each hiddenPanels as panel (panel)}
        <button
          class="menu-item"
          type="button"
          role="menuitem"
          onclick={() => runAdd(panel)}
        >
          <svg
            class="tab-icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d={PANEL_META[panel]?.icon ?? ""} />
          </svg>
          {panelLabel(panel)}
        </button>
      {/each}
    {/if}
  </div>
{/if}

<style>
  .tabgroup {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    min-height: 0;
  }

  .panel-tabs {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    gap: 0.125rem;
    padding: 0.25rem 0.5rem 0;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container, var(--m3-surface));
  }

  .panel-tab {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-bottom: none;
    border-radius: var(--m3-shape-small, 8px) var(--m3-shape-small, 8px) 0 0;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.6875rem;
    /* Constant across selected/unselected: a selected-state weight bump
       widens the label, resizing the tab and shifting every sibling tab. */
    font-weight: 500;
    padding: 0.25rem 0.5rem;
    cursor: pointer;
  }

  .tab-icon {
    width: 0.75rem;
    height: 0.75rem;
    flex: none;
  }

  .panel-tab:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .panel-tab:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .panel-tab[aria-selected="true"] {
    background: var(--m3-surface);
    color: var(--m3-primary);
    /* Visually merge with the panel body below. */
    padding-bottom: calc(0.25rem + 1px);
    margin-bottom: -1px;
  }

  .panel-add {
    flex: none;
    border: none;
    border-bottom: 1px solid transparent;
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.8125rem;
    line-height: 1;
    padding: 0.25rem 0.5rem;
    cursor: pointer;
    border-radius: var(--m3-shape-small, 8px) var(--m3-shape-small, 8px) 0 0;
  }

  .panel-add:hover,
  .panel-add:focus-visible {
    color: var(--m3-primary);
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .panel-body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .panel-body > :global(aside),
  .panel-body > :global(section) {
    flex: 1;
    min-height: 0;
  }

  .menu {
    position: fixed;
    z-index: 80;
    min-width: 12rem;
    display: flex;
    flex-direction: column;
    padding: 0.25rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-medium, 12px);
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    box-shadow: 0 0.5rem 1.5rem rgb(0 0 0 / 0.3);
  }

  .menu-item {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
    border: none;
    background: none;
    color: inherit;
    font: inherit;
    font-size: 0.75rem;
    text-align: left;
    padding: 0.35rem 0.5rem;
    border-radius: var(--m3-shape-small, 8px);
    cursor: pointer;
    white-space: nowrap;
  }

  .menu-item:hover:not(:disabled),
  .menu-item:focus-visible {
    background: var(--m3-surface-container-highest, var(--m3-surface));
    outline: none;
  }

  .menu-item:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .menu-item.danger {
    color: var(--m3-error, red);
  }
</style>
