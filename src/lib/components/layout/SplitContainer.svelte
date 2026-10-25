<script lang="ts">
  /**
   * SplitContainer — recursive renderer for the layout splitter tree.
   *
   * - `leaf`     → renders the panel via the `renderPanel` snippet factory
   * - `tabs`     → delegates to TabGroup.svelte (tab strip + menus)
   * - `split`    → two child SplitContainers beside a draggable divider
   *
   * Dividers: pointer-drag (ratio clamped to RATIO_MIN–RATIO_MAX by the
   * model), `role="separator"` with `aria-orientation="vertical"`, and
   * Left/Right arrow keys nudge ±RATIO_STEP (2%) when focused. Splits are
   * horizontal (a left, b right); the drag ratio maps the divider's x
   * position within the split's box.
   *
   * All state changes flow out through `onRatio` (RepoView applies them to
   * the layout store); this component holds only transient drag state.
   *
   * Collapsed sidebars: ids in `collapsedIds` are removed from the flex
   * layout (`display: none`) while their subtree stays mounted, so panel
   * state survives and the sibling pane automatically fills the freed
   * space. `hideTabsFor` replaces one group's tab strip with a compact title
   * row (the activity-bar-controlled primary sidebar).
   */
  import type { Snippet } from "svelte";
  import {
    RATIO_MAX,
    RATIO_MIN,
    RATIO_STEP,
    clampRatio,
    type LayoutNode,
    type PanelGroupAction,
    type PanelId,
    type SplitNode,
  } from "$lib/layout/layoutModel";
  import TabGroup from "./TabGroup.svelte";

  // Self-import for recursion (Svelte 5's replacement for <svelte:self>).
  import SplitContainer from "./SplitContainer.svelte";

  let {
    node,
    renderPanel,
    hiddenPanels = [],
    collapsedIds = [],
    hideTabsFor = null,
    canMoveLeftFor,
    canMoveRightFor,
    onRatio,
    onActivateTab,
    onAddPanel,
    onPanelAction,
    /** Whether a collapsed-history rail sits at the right edge of the tree.
     *  Split bases on the right spine are widened by the rail width: the
     *  rail is a sibling of the whole tree, so without this every
     *  percentage-sized pane would shrink slightly whenever history is
     *  collapsed. Compensating here keeps every left-hand pane at its size
     *  and lets the rail's width come out of the rightmost pane instead. */
    rightRail = false,
  }: {
    /** The (sub)tree this instance renders. */
    node: LayoutNode;
    /** Snippet factory: PanelId → the panel's rendered content. */
    renderPanel: (id: PanelId) => Snippet;
    /** Panels not in the tree (for TabGroup's "+ Add panel" menu). */
    hiddenPanels?: PanelId[];
    /**
     * Node ids whose pane is currently collapsed. The subtree stays mounted
     * (and keeps its state) but is removed from layout with `display: none`;
     * its sibling then grows to fill the freed space.
     */
    collapsedIds?: string[];
    /** Tab group whose tab strip is replaced by the activity bar. */
    hideTabsFor?: string | null;
    /** Whether the tab group at `tabsId` has a group to its left. */
    canMoveLeftFor?: (tabsId: string) => boolean;
    /** Whether the tab group at `tabsId` has a group to its right. */
    canMoveRightFor?: (tabsId: string) => boolean;
    /** Splitter moved (already clamped by callers via the store). */
    onRatio: (splitId: string, ratio: number) => void;
    onActivateTab: (tabsId: string, index: number) => void;
    onAddPanel: (tabsId: string, panel: PanelId) => void;
    onPanelAction: (action: PanelGroupAction, tabsId: string, panel: PanelId) => void;
    /** True while the history rail occupies the right edge (see destructure). */
    rightRail?: boolean;
  } = $props();

  // -- divider dragging ------------------------------------------------------

  let dragSplitId = $state<string | null>(null);
  let dragBox = $state<{ left: number; width: number } | null>(null);

  function dividerDown(event: PointerEvent, split: SplitNode): void {
    const host = event.currentTarget as HTMLElement;
    const box = host.parentElement?.getBoundingClientRect();
    if (!box || box.width === 0) return;
    event.preventDefault();
    dragSplitId = split.id;
    dragBox = { left: box.left, width: box.width };
    try {
      host.setPointerCapture(event.pointerId);
    } catch {
      // Pointer capture is best-effort; moves still work while hovered.
    }
  }

  function dividerMove(event: PointerEvent): void {
    if (!dragSplitId || !dragBox) return;
    onRatio(dragSplitId, clampRatio((event.clientX - dragBox.left) / dragBox.width));
  }

  function dividerUp(event: PointerEvent): void {
    if (!dragSplitId) return;
    try {
      (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
    } catch {
      // Already released (pointercancel path).
    }
    dragSplitId = null;
    dragBox = null;
  }

  function dividerKey(event: KeyboardEvent, split: SplitNode): void {
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      onRatio(split.id, clampRatio(split.ratio - RATIO_STEP));
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      onRatio(split.id, clampRatio(split.ratio + RATIO_STEP));
    } else if (event.key === "Home") {
      event.preventDefault();
      onRatio(split.id, RATIO_MIN);
    } else if (event.key === "End") {
      event.preventDefault();
      onRatio(split.id, RATIO_MAX);
    }
  }

  const groupCanMoveLeft = $derived(
    node.kind === "tabs" ? (canMoveLeftFor?.(node.id) ?? false) : false,
  );
  const groupCanMoveRight = $derived(
    node.kind === "tabs" ? (canMoveRightFor?.(node.id) ?? false) : false,
  );
</script>

{#if node.kind === "split"}
  {@const hideA = collapsedIds.includes(node.a.id)}
  {@const hideB = collapsedIds.includes(node.b.id)}
  <!-- The rail consumes width from the tree, not from the left panes: on
       the right spine add the rail's share (ratio × rail width) back to `a`
       so toggling history only resizes the rightmost pane. -->
  {@const aBasis = rightRail
    ? `calc(${node.ratio * 100}% + ${node.ratio} * var(--history-rail-width, 0px))`
    : `${node.ratio * 100}%`}
  <div class="split" class:hide-a={hideA} class:hide-b={hideB}>
    <div
      class="pane pane-a"
      style:flex={hideB ? "1 1 0%" : `0 0 ${aBasis}`}
    >
      <SplitContainer
        node={node.a}
        {renderPanel}
        {hiddenPanels}
        {collapsedIds}
        {hideTabsFor}
        {canMoveLeftFor}
        {canMoveRightFor}
        {onRatio}
        {onActivateTab}
        {onAddPanel}
        {onPanelAction}
        rightRail={false}
      />
    </div>
    <!-- A focusable separator is the ARIA-recommended pattern for split
         dividers (keyboard resize); the a11y lint can't see the role. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="divider"
      class:dragging={dragSplitId === node.id}
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize panels"
      aria-valuemin={Math.round(RATIO_MIN * 100)}
      aria-valuemax={Math.round(RATIO_MAX * 100)}
      aria-valuenow={Math.round(node.ratio * 100)}
      tabindex="0"
      onpointerdown={(e) => dividerDown(e, node)}
      onpointermove={dividerMove}
      onpointerup={dividerUp}
      onpointercancel={dividerUp}
      onkeydown={(e) => dividerKey(e, node)}
    ></div>
    <div class="pane pane-b grow">
      <SplitContainer
        node={node.b}
        {renderPanel}
        {hiddenPanels}
        {collapsedIds}
        {hideTabsFor}
        {canMoveLeftFor}
        {canMoveRightFor}
        {onRatio}
        {onActivateTab}
        {onAddPanel}
        {onPanelAction}
        {rightRail}
      />
    </div>
  </div>
{:else if node.kind === "tabs"}
  <TabGroup
    {node}
    {renderPanel}
    {hiddenPanels}
    hideTabs={node.id === hideTabsFor}
    canMoveLeft={groupCanMoveLeft}
    canMoveRight={groupCanMoveRight}
    onActivate={onActivateTab}
    onAdd={onAddPanel}
    onAction={onPanelAction}
  />
{:else}
  <div class="leaf">
    {@render renderPanel(node.panel)()}
  </div>
{/if}

<style>
  .split {
    display: flex;
    width: 100%;
    height: 100%;
    min-width: 0;
    min-height: 0;
  }

  .pane {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .pane.grow {
    flex: 1 1 0%;
  }

  /* Collapsed sidebars keep their DOM (state) but leave the flex layout; the
   * remaining pane automatically takes the freed width. */
  .split.hide-a > .pane-a,
  .split.hide-b > .pane-b,
  .split.hide-a > .divider,
  .split.hide-b > .divider {
    display: none;
  }

  .pane > :global(*) {
    flex: 1;
    min-height: 0;
  }

  .divider {
    flex: none;
    width: 5px;
    margin: 0 -2px;
    z-index: 1;
    cursor: col-resize;
    touch-action: none;
    background:
      linear-gradient(
        to right,
        transparent 2px,
        var(--m3-outline-variant, var(--m3-primary)) 2px,
        var(--m3-outline-variant, var(--m3-primary)) 3px,
        transparent 3px
      );
  }

  .divider:hover,
  .divider:focus-visible,
  .divider.dragging {
    background:
      linear-gradient(
        to right,
        transparent 1px,
        var(--m3-primary) 1px,
        var(--m3-primary) 4px,
        transparent 4px
      );
  }

  .divider:focus-visible {
    outline: none;
  }

  .leaf {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    min-width: 0;
    min-height: 0;
  }

  .leaf > :global(*) {
    flex: 1;
    min-height: 0;
  }
</style>
