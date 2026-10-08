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
    canMoveRightFor,
    onRatio,
    onActivateTab,
    onAddPanel,
    onPanelAction,
  }: {
    /** The (sub)tree this instance renders. */
    node: LayoutNode;
    /** Snippet factory: PanelId → the panel's rendered content. */
    renderPanel: (id: PanelId) => Snippet;
    /** Panels not in the tree (for TabGroup's "+ Add panel" menu). */
    hiddenPanels?: PanelId[];
    /** Whether the tab group at `tabsId` has a group to its right. */
    canMoveRightFor?: (tabsId: string) => boolean;
    /** Splitter moved (already clamped by callers via the store). */
    onRatio: (splitId: string, ratio: number) => void;
    onActivateTab: (tabsId: string, index: number) => void;
    onAddPanel: (tabsId: string, panel: PanelId) => void;
    onPanelAction: (action: PanelGroupAction, tabsId: string, panel: PanelId) => void;
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

  const groupCanMoveRight = $derived(
    node.kind === "tabs" ? (canMoveRightFor?.(node.id) ?? false) : false,
  );
</script>

{#if node.kind === "split"}
  <div class="split">
    <div class="pane" style:flex={`0 0 ${node.ratio * 100}%`}>
      <SplitContainer
        node={node.a}
        {renderPanel}
        {hiddenPanels}
        {canMoveRightFor}
        {onRatio}
        {onActivateTab}
        {onAddPanel}
        {onPanelAction}
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
    <div class="pane grow">
      <SplitContainer
        node={node.b}
        {renderPanel}
        {hiddenPanels}
        {canMoveRightFor}
        {onRatio}
        {onActivateTab}
        {onAddPanel}
        {onPanelAction}
      />
    </div>
  </div>
{:else if node.kind === "tabs"}
  <TabGroup
    {node}
    {renderPanel}
    {hiddenPanels}
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
