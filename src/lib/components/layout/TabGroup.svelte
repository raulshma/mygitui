<script lang="ts">
  /**
   * TabGroup — renders a `{ kind: "tabs" }` layout leaf: the tab strip
   * (a {@link UnifiedTabStrip} surface — same engine as the repo/file tab
   * strips since the §4.3 primitives pass), the panel bodies, a
   * "+ Add panel" menu listing hidden panels, and a right-click context
   * menu per tab ("Move to right group" / "New group to the right" /
   * "Hide").
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
   * Overlay drawers (plan §10, Medium/Narrow): panels in `drawerPanels`
   * keep their keep-alive host but present it as a non-modal overlay
   * drawer (`position: absolute` against the positioned workspace frame —
   * the SAME element as the docked host, so a docked↔drawer mode flip
   * never remounts the panel). A drawerized panel shows no tab button
   * (the ActivityBar item / edge rail toggle it instead), and it mounts
   * eagerly when its drawer is open so the content is there on first
   * open. Visibility is the drawer's open state, not the group's active
   * tab.
   *
   * Pure presentation: every action is delegated to callbacks owned by
   * RepoView (which applies them through the layout store). Component
   * rendering is not unit testable, so behavior tests live in the layout
   * model/store suites.
   *
   * Strip behavior preserved from the pre-unification implementation:
   * arrows activate as they move focus (`activateOnArrow`), middle-click
   * hides the panel, and the background right-click is suppressed — the
   * per-tab context menu replaces it.
   */
  import type { Snippet } from "svelte";
  import {
    PANEL_META,
    panelLabel,
    type MoveTarget,
    type PanelGroupAction,
    type PanelId,
    type TabsNode,
  } from "$lib/layout/layoutModel";
  import {
    PANEL_DRAG_MIME,
    getDragPanelInfo,
    setInFlightDrag,
  } from "$lib/layout/panelDnd";
import {
  overlayHostAttrs,
  overlayPaneClass,
  type DrawerBinding,
} from "$lib/layout/drawerPlan";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import UnifiedTabStrip from "./UnifiedTabStrip.svelte";

  let {
    node,
    renderPanel,
    hiddenPanels,
    drawerPanels = [],
    hideTabs = false,
    canMoveLeft,
    canMoveRight,
    onActivate,
    onAdd,
    onAction,
    onMovePanel,
  }: {
    node: TabsNode;
    /** Snippet factory: PanelId → the panel's rendered content. */
    renderPanel: (id: PanelId) => Snippet;
    /** Panels currently not in the tree (candidates for "+ Add panel"). */
    hiddenPanels: PanelId[];
    /**
     * Panels of ANY group rendered as overlay drawers; entries whose
     * `key` is one of this group's tabs apply here.
     */
    drawerPanels?: DrawerBinding[];
    /** Replace the tab strip with a compact title row (activity-bar group). */
    hideTabs?: boolean;
    /** Whether the holder group has a group to its left. */
    canMoveLeft: boolean;
    /** Whether the holder group has a group to its right. */
    canMoveRight: boolean;
    onActivate: (tabsId: string, index: number) => void;
    onAdd: (tabsId: string, panel: PanelId) => void;
    onAction: (action: PanelGroupAction, tabsId: string, panel: PanelId) => void;
    onMovePanel?: (panel: PanelId, target: MoveTarget) => void;
  } = $props();

  /** Active index clamped against the current tabs (defensive vs overlays). */
  const activeIndex = $derived(
    node.tabs.length === 0
      ? 0
      : Math.min(Math.max(node.active, 0), node.tabs.length - 1),
  );
  const activePanel = $derived(node.tabs[activeIndex]);

  /** Drawer bindings for THIS group's tabs (other groups' entries ignored). */
  const groupDrawerPanels = $derived.by(() => {
    const map = new Map<PanelId, DrawerBinding>();
    for (const binding of drawerPanels) {
      if (node.tabs.includes(binding.key as PanelId)) {
        map.set(binding.key as PanelId, binding);
      }
    }
    return map;
  });

  /** Strip items: PanelId + the label the ⋯ overflow menu shows. Drawerized
   *  panels show no tab button — the ActivityBar item / edge rail toggles
   *  their drawer instead (plan §10). */
  const stripTabs: Array<{ id: PanelId; label: string }> = $derived(
    node.tabs
      .filter((panel) => !groupDrawerPanels.has(panel))
      .map((panel) => ({ id: panel, label: panelLabel(panel) })),
  );

  // -- keep-alive mounting -----------------------------------------------------

  /** Panels already activated in this group, in first-activation order. */
  let mounted: PanelId[] = $state([]);

  // Runs pre-render so a newly activated panel mounts in the same flush —
  // the body never renders a frame without it. An OPEN drawer mounts its
  // panel the same way (first drawer open must not wait for a visit).
  $effect.pre(() => {
    const additions = node.tabs.filter(
      (panel) =>
        (panel === activePanel ||
          (groupDrawerPanels.get(panel)?.open ?? false)) &&
        !mounted.includes(panel),
    );
    if (additions.length > 0) {
      mounted = [...mounted, ...additions];
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

  /** Middle click hides the panel, mirroring the repo/file tab strips. */
  function onAuxClick(event: MouseEvent, panel: PanelId): void {
    if (event.button !== 1) return;
    // Prevent the webview's autoscroll affordance.
    event.preventDefault();
    onAction("hide", node.id, panel);
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

  // -- drag and drop ---------------------------------------------------------
  let dragPanel = $state<PanelId | null>(null);
  let dropTargetPanel = $state<PanelId | null>(null);
  let dropPosition = $state<"before" | "after" | "left-split" | "right-split" | "tab-end" | null>(null);

  function onTabDragStart(event: DragEvent, panel: PanelId): void {
    dragPanel = panel;
    setInFlightDrag({ panel, sourceGroupId: node.id });
    event.dataTransfer?.setData(PANEL_DRAG_MIME, JSON.stringify({ panel, sourceGroupId: node.id }));
    event.dataTransfer?.setData("text/plain", panel);
    if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
  }

  function onTabDragEnd(): void {
    dragPanel = null;
    dropTargetPanel = null;
    dropPosition = null;
    setInFlightDrag(null);
  }

  function onTabDragOver(event: DragEvent, targetPanel: PanelId, index: number): void {
    const info = getDragPanelInfo(event);
    if (!info) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "move";

    if (info.sourceGroupId === node.id && info.panel === targetPanel) {
      dropTargetPanel = null;
      dropPosition = null;
      return;
    }

    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const isAfter = event.clientX >= rect.left + rect.width / 2;
    dropTargetPanel = targetPanel;
    dropPosition = isAfter ? "after" : "before";
  }

  function onTabDrop(event: DragEvent, targetPanel: PanelId, index: number): void {
    const info = getDragPanelInfo(event);
    if (!info) return;
    event.preventDefault();
    event.stopPropagation();

    const { panel, sourceGroupId } = info;
    const isAfter = dropPosition === "after";
    let targetIndex = isAfter ? index + 1 : index;

    if (sourceGroupId === node.id) {
      const fromIndex = node.tabs.indexOf(panel);
      if (fromIndex !== -1 && fromIndex < targetIndex) {
        targetIndex -= 1;
      }
      if (targetIndex !== fromIndex) {
        if (onMovePanel) {
          onMovePanel(panel, { kind: "tab", nodeId: node.id, index: targetIndex });
        }
      }
    } else {
      if (onMovePanel) {
        onMovePanel(panel, { kind: "tab", nodeId: node.id, index: targetIndex });
      }
    }

    onTabDragEnd();
  }

  function onStripDragOver(event: DragEvent): void {
    const info = getDragPanelInfo(event);
    if (!info) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
    dropTargetPanel = null;
    dropPosition = "tab-end";
  }

  function onStripDrop(event: DragEvent): void {
    const info = getDragPanelInfo(event);
    if (!info) return;
    event.preventDefault();
    const { panel, sourceGroupId } = info;
    if (sourceGroupId === node.id) {
      const fromIndex = node.tabs.indexOf(panel);
      if (fromIndex !== node.tabs.length - 1 && onMovePanel) {
        onMovePanel(panel, { kind: "tab", nodeId: node.id, index: node.tabs.length - 1 });
      }
    } else {
      if (onMovePanel) {
        onMovePanel(panel, { kind: "tab", nodeId: node.id });
      }
    }
    onTabDragEnd();
  }

  function onBodyDragOver(event: DragEvent): void {
    const info = getDragPanelInfo(event);
    if (!info) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "move";

    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const relX = event.clientX - rect.left;
    if (relX < rect.width * 0.25) {
      dropPosition = "left-split";
    } else if (relX > rect.width * 0.75) {
      dropPosition = "right-split";
    } else {
      dropPosition = null;
    }
  }

  function onBodyDragLeave(event: DragEvent): void {
    const related = event.relatedTarget as Node | null;
    if (event.currentTarget && (event.currentTarget as HTMLElement).contains(related)) {
      return;
    }
    if (dropPosition === "left-split" || dropPosition === "right-split") {
      dropPosition = null;
    }
  }

  function onBodyDrop(event: DragEvent): void {
    const info = getDragPanelInfo(event);
    if (!info) return;
    event.preventDefault();

    const { panel } = info;
    if (dropPosition === "left-split") {
      if (onMovePanel) {
        onMovePanel(panel, { kind: "before", nodeId: node.id, asTabs: true });
      } else {
        onAction("new-group-left", node.id, panel);
      }
    } else if (dropPosition === "right-split") {
      if (onMovePanel) {
        onMovePanel(panel, { kind: "after", nodeId: node.id, asTabs: true });
      } else {
        onAction("new-group-right", node.id, panel);
      }
    } else if (info.sourceGroupId !== node.id) {
      if (onMovePanel) {
        onMovePanel(panel, { kind: "tab", nodeId: node.id });
      }
    }
    onTabDragEnd();
  }
</script>

<div class="tabgroup flex h-full w-full min-h-0 flex-col" bind:this={rootEl}>
  {#if hideTabs}
    {#if activePanel}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="panel-head flex flex-none items-center gap-ui-35 border-b border-border bg-popover px-2.5 py-1 cursor-grab"
        draggable="true"
        ondragstart={(e) => onTabDragStart(e, activePanel)}
        ondragend={onTabDragEnd}
        onauxclick={(e) => onAuxClick(e, activePanel)}
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
          class="panel-title min-w-0 flex-1 truncate text-ui-micro font-semibold uppercase tracking-[0.04em] text-muted-foreground"
          >{panelLabel(activePanel)}</span
        >
        <Hint label="Panel actions">
          {#snippet child({ props })}
            <Button
              class="panel-menu size-5 flex-none rounded-sm p-0 text-sm leading-none focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 focus-visible:ring-0"
              variant="ghost"
              size="icon-xs"
              type="button"
              {...props}
              aria-haspopup="menu"
              aria-expanded={menu?.kind === "context"}
              onclick={(e) => openContext(e, activePanel)}
            >
              ⋯
            </Button>
          {/snippet}
        </Hint>
      </div>
    {/if}
  {:else}
    <UnifiedTabStrip
      tabs={stripTabs}
      activeId={activePanel ?? null}
      label="Panels"
      stripClass="panel-tabs flex-none flex-nowrap gap-0.5 border-b border-border bg-popover px-2 pt-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      activateOnArrow
      edgeFades={false}
      onActivate={(item) => onActivate(node.id, node.tabs.indexOf(item.id))}
      onStripContextMenu={(e) => e.preventDefault()}
    >
      {#snippet tab(item, cell)}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="relative inline-flex items-stretch"
          ondragover={(e) => onTabDragOver(e, item.id, cell.index)}
          ondrop={(e) => onTabDrop(e, item.id, cell.index)}
        >
          {#if dropTargetPanel === item.id && dropPosition === "before"}
            <span
              class="pointer-events-none absolute -left-0.5 inset-y-0.5 w-1 rounded-full bg-primary z-20"
              aria-hidden="true"
            ></span>
          {/if}
          <Hint label={`${panelLabel(item.id)} — middle-click to close`}>
            {#snippet child({ props })}
              <button
                data-tab-index={cell.index}
                class="panel-tab inline-flex flex-none cursor-pointer items-center gap-1 whitespace-nowrap rounded-t-sm border border-b-0 bg-transparent px-2 py-1 text-ui-micro font-medium text-muted-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 aria-selected:-mb-px aria-selected:rounded-b-none aria-selected:bg-card aria-selected:pb-[calc(0.25rem+1px)] aria-selected:text-primary-ink {dragPanel === item.id ? 'opacity-40' : ''}"
                type="button"
                draggable="true"
                {...props}
                {...cell.triggerProps}
                id={`tab-${node.id}-${item.id}`}
                aria-controls={`tabpanel-${node.id}`}
                aria-label={panelLabel(item.id)}
                onclick={() => onActivate(node.id, cell.index)}
                onauxclick={(e) => onAuxClick(e, item.id)}
                oncontextmenu={(e) => openContext(e, item.id)}
                ondragstart={(e) => onTabDragStart(e, item.id)}
                ondragend={onTabDragEnd}
                onmousedown={(e) => {
                  if (e.button === 1) e.preventDefault();
                }}
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
                  <path d={PANEL_META[item.id]?.icon ?? ""} />
                </svg>
                {panelLabel(item.id)}
              </button>
            {/snippet}
          </Hint>
          {#if dropTargetPanel === item.id && dropPosition === "after"}
            <span
              class="pointer-events-none absolute -right-0.5 inset-y-0.5 w-1 rounded-full bg-primary z-20"
              aria-hidden="true"
            ></span>
          {/if}
        </div>
      {/snippet}
      {#snippet inStrip()}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="tab-strip-drop flex flex-1 items-center h-full min-w-8"
          ondragover={onStripDragOver}
          ondrop={onStripDrop}
        >
          {#if hiddenPanels.length > 0}
            <Hint label="Add panel">
              {#snippet child({ props })}
                <Button
                  class="panel-add h-auto flex-none rounded-b-none rounded-t-sm border-b border-b-transparent px-2 py-1 text-ui-base leading-none hover:text-primary-ink focus-visible:bg-accent focus-visible:text-primary-ink"
                  variant="ghost"
                  size="sm"
                  type="button"
                  {...props}
                  aria-haspopup="menu"
                  aria-expanded={menu?.kind === "add"}
                  onclick={openAdd}
                >
                  +
                </Button>
              {/snippet}
            </Hint>
          {/if}
        </div>
      {/snippet}
    </UnifiedTabStrip>
  {/if}

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="panel-body relative flex min-h-0 flex-1 flex-col overflow-hidden"
    id={`tabpanel-${node.id}`}
    role="tabpanel"
    tabindex="0"
    aria-label={hideTabs && activePanel ? panelLabel(activePanel) : undefined}
    aria-labelledby={!hideTabs && activePanel ? `tab-${node.id}-${activePanel}` : undefined}
    ondragover={onBodyDragOver}
    ondragleave={onBodyDragLeave}
    ondrop={onBodyDrop}
  >
    {#if dropPosition === "left-split"}
      <div
        class="pointer-events-none absolute inset-y-0 left-0 w-1/4 z-30 bg-primary/15 border-r-2 border-primary flex items-center justify-center text-xs font-semibold text-primary select-none backdrop-blur-[1px]"
        aria-hidden="true"
      >
        <span class="rounded bg-popover/90 px-2 py-1 shadow-sm border border-border">Drop to split left</span>
      </div>
    {:else if dropPosition === "right-split"}
      <div
        class="pointer-events-none absolute inset-y-0 right-0 w-1/4 z-30 bg-primary/15 border-l-2 border-primary flex items-center justify-center text-xs font-semibold text-primary select-none backdrop-blur-[1px]"
        aria-hidden="true"
      >
        <span class="rounded bg-popover/90 px-2 py-1 shadow-sm border border-border">Drop to split right</span>
      </div>
    {/if}
    <!-- Keep-alive hosts: one per visited panel; inactive ones are hidden,
         not unmounted (see the script's keep-alive note). A drawerized
         panel's host presents as a non-modal overlay drawer instead — the
         SAME element, repositioned with CSS, so docked↔drawer mode flips
         never remount the panel. Stacked bindings present the host as a
         full-screen route (same element, `inset-0`). -->
    {#each shownPanels as panel (panel)}
      {@const drawer = groupDrawerPanels.get(panel) ?? null}
      <div
        class={drawer
          ? overlayPaneClass(drawer)
          : "panel-host flex min-h-0 flex-1 flex-col [&[hidden]]:hidden [&>aside]:min-h-0 [&>aside]:flex-1 [&>section]:min-h-0 [&>section]:flex-1"}
        hidden={drawer ? undefined : panel !== activePanel}
        {...(drawer ? overlayHostAttrs(drawer) : {})}
      >
        {@render renderPanel(panel)()}
      </div>
    {/each}
  </div>
</div>

{#if menu}
  <div
    class="menu fixed z-80 flex min-w-48 flex-col rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-overlay"
    role="menu"
    bind:this={menuEl}
    style:left={`${menu.x}px`}
    style:top={`${menu.y}px`}
  >
    {#if menu.kind === "context"}
      <button
        class="menu-item inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-ui-35 text-left text-xs text-inherit enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
        type="button"
        role="menuitem"
        disabled={!canMoveLeft}
        onclick={() => runAction("move-left-group")}
      >
        Move to left group
      </button>
      <button
        class="menu-item inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-ui-35 text-left text-xs text-inherit enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
        type="button"
        role="menuitem"
        disabled={!canMoveRight}
        onclick={() => runAction("move-right-group")}
      >
        Move to right group
      </button>
      <button
        class="menu-item inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-ui-35 text-left text-xs text-inherit enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
        type="button"
        role="menuitem"
        onclick={() => runAction("new-group-left")}
      >
        New group to the left
      </button>
      <button
        class="menu-item inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-ui-35 text-left text-xs text-inherit enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
        type="button"
        role="menuitem"
        onclick={() => runAction("new-group-right")}
      >
        New group to the right
      </button>
      <button
        class="menu-item danger inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-ui-35 text-left text-xs text-inherit text-destructive enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
        type="button"
        role="menuitem"
        onclick={() => runAction("hide")}
      >
        Hide
      </button>
    {:else}
      {#each hiddenPanels as panel (panel)}
        <button
          class="menu-item inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border-0 bg-transparent px-2 py-ui-35 text-left text-xs text-inherit enabled:hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
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
