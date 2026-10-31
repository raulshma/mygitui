<script lang="ts">
  /**
   * TabGroup — renders a `{ kind: "tabs" }` layout leaf: the tab strip
   * (same visual + keyboard pattern as the pre-M4 RepoView tablist),
   * the panel bodies, a "+ Add panel" menu listing hidden panels,
   * and a right-click context menu per tab ("Move to right group" /
   * "New group to the right" / "Hide").
   *
   * When `hideTabs` is set (the activity-bar-controlled primary sidebar),
   * the horizontal strip is replaced by a compact view title + actions
   * button; switching happens through `ActivityBar.svelte` instead.
   *
   * Keep-alive mounting: every panel, once activated, stays mounted —
   * inactive bodies are hidden (`display: none` hosts), not unmounted —
   * so view state (history selection + open detail, scroll positions,
   * loaded streams) survives tab switches. Panels mount lazily on their
   * first activation, which keeps never-visited panels (and their dynamic
   * import trees) out of the session. Panels removed from the group
   * ("Hide" / move) unmount; re-adding starts fresh.
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
  import { Button } from "$lib/components/ui/button/index.js";

  let {
    node,
    renderPanel,
    hiddenPanels,
    hideTabs = false,
    canMoveLeft,
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
    /** Replace the tab strip with a compact title row (activity-bar group). */
    hideTabs?: boolean;
    /** Whether the holder group has a group to its left. */
    canMoveLeft: boolean;
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

  // -- keep-alive mounting -----------------------------------------------------

  /** Panels already activated in this group, in first-activation order. */
  let mounted: PanelId[] = $state([]);

  // Runs pre-render so a newly activated panel mounts in the same flush —
  // the body never renders a frame without it.
  $effect.pre(() => {
    if (activePanel && !mounted.includes(activePanel)) {
      mounted = [...mounted, activePanel];
    }
  });

  /** Panels to keep mounted: visited ∩ current tabs (active always wins). */
  const shownPanels = $derived.by(() => {
    const list = mounted.filter((panel) => node.tabs.includes(panel));
    if (activePanel && !list.includes(activePanel)) list.push(activePanel);
    return list;
  });

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

<div class="tabgroup flex h-full w-full min-h-0 flex-col" bind:this={rootEl}>
  {#if hideTabs}
    {#if activePanel}
      <div
        class="panel-head flex flex-none items-center gap-[0.35rem] border-b border-border bg-popover pt-[0.3rem] pb-[0.3rem] pr-[0.4rem] pl-[0.6rem]"
      >
        <svg
          class="tab-icon h-3 w-3 flex-none text-muted-foreground"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d={PANEL_META[activePanel]?.icon ?? ""} />
        </svg>
        <span
          class="panel-title min-w-0 flex-1 truncate text-[0.6875rem] font-semibold uppercase tracking-[0.04em] text-muted-foreground"
          >{panelLabel(activePanel)}</span
        >
        <Button
          class="panel-menu size-[1.35rem] flex-none rounded-sm p-0 text-sm leading-none focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 focus-visible:ring-0"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label="Panel actions"
          aria-haspopup="menu"
          aria-expanded={menu?.kind === "context"}
          title="Panel actions"
          onclick={(e) => openContext(e, activePanel)}
        >
          ⋯
        </Button>
      </div>
    {/if}
  {:else}
    <div
      class="panel-tabs flex flex-none flex-nowrap gap-0.5 overflow-x-auto border-b border-border bg-popover px-2 pt-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      role="tablist"
      aria-label="Panels"
      tabindex="-1"
      onkeydown={onTablistKeydown}
      oncontextmenu={(e) => e.preventDefault()}
    >
      {#each node.tabs as panel, index (panel)}
        <button
          data-tab-index={index}
          class="panel-tab inline-flex flex-none cursor-pointer items-center gap-1 whitespace-nowrap rounded-t-sm border border-b-0 bg-transparent px-2 py-1 text-[0.6875rem] font-medium text-muted-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 aria-selected:-mb-px aria-selected:rounded-b-none aria-selected:bg-card aria-selected:pb-[calc(0.25rem+1px)] aria-selected:text-primary"
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
            class="tab-icon h-3 w-3 flex-none"
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
        <Button
          class="panel-add h-auto flex-none rounded-b-none rounded-t-sm border-b border-b-transparent px-2 py-1 text-[0.8125rem] leading-none hover:text-primary focus-visible:bg-accent focus-visible:text-primary"
          variant="ghost"
          size="sm"
          type="button"
          aria-label="Add panel"
          aria-haspopup="menu"
          aria-expanded={menu?.kind === "add"}
          title="Add panel"
          onclick={openAdd}
        >
          +
        </Button>
      {/if}
    </div>
  {/if}

  <div
    class="panel-body flex min-h-0 flex-1 flex-col overflow-hidden"
    id={`tabpanel-${node.id}`}
    role="tabpanel"
    aria-label={hideTabs && activePanel ? panelLabel(activePanel) : undefined}
    aria-labelledby={!hideTabs && activePanel ? `tab-${node.id}-${activePanel}` : undefined}
  >
    <!-- Keep-alive hosts: one per visited panel; inactive ones are hidden,
         not unmounted (see the script's keep-alive note). -->
    {#each shownPanels as panel (panel)}
      <div
        class="panel-host flex min-h-0 flex-1 flex-col [&[hidden]]:hidden [&>aside]:min-h-0 [&>aside]:flex-1 [&>section]:min-h-0 [&>section]:flex-1"
        hidden={panel !== activePanel}
      >
        {@render renderPanel(panel)()}
      </div>
    {/each}
  </div>
</div>

{#if menu}
  <div
    class="menu fixed z-80 flex min-w-48 flex-col rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-[0_0.5rem_1.5rem_rgb(0_0_0/0.3)]"
    role="menu"
    bind:this={menuEl}
    style:left={`${menu.x}px`}
    style:top={`${menu.y}px`}
  >
    {#if menu.kind === "context"}
      <button
        class="menu-item inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-[0.35rem] text-left text-xs text-inherit enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
        type="button"
        role="menuitem"
        disabled={!canMoveLeft}
        onclick={() => runAction("move-left-group")}
      >
        Move to left group
      </button>
      <button
        class="menu-item inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-[0.35rem] text-left text-xs text-inherit enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
        type="button"
        role="menuitem"
        disabled={!canMoveRight}
        onclick={() => runAction("move-right-group")}
      >
        Move to right group
      </button>
      <button
        class="menu-item inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-[0.35rem] text-left text-xs text-inherit enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
        type="button"
        role="menuitem"
        onclick={() => runAction("new-group-left")}
      >
        New group to the left
      </button>
      <button
        class="menu-item inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-[0.35rem] text-left text-xs text-inherit enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
        type="button"
        role="menuitem"
        onclick={() => runAction("new-group-right")}
      >
        New group to the right
      </button>
      <button
        class="menu-item danger inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-[0.35rem] text-left text-xs text-inherit text-destructive enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
        type="button"
        role="menuitem"
        onclick={() => runAction("hide")}
      >
        Hide
      </button>
    {:else}
      {#each hiddenPanels as panel (panel)}
        <button
          class="menu-item inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-[0.35rem] text-left text-xs text-inherit enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
          type="button"
          role="menuitem"
          onclick={() => runAdd(panel)}
        >
          <svg
            class="tab-icon h-3 w-3 flex-none"
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
