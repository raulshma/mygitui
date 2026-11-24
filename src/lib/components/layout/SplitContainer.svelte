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
 *
 * Overlay drawers (plan §10, Medium/Narrow): ids in `drawerPanes` render
 * their pane as a non-modal overlay drawer — the SAME keep-alive host
 * element, repositioned with `position: absolute` against the positioned
 * workspace frame (`.main-area`), so a docked↔drawer mode flip never
 * remounts the panel. An absolute pane is out of its split's flex flow:
 * the sibling grows (`1 1 0%`) and the divider hides. Ids in `drainedIds`
 * shrink to zero flex basis WITHOUT `display: none` — used for tab groups
 * whose panels all drawerized (or that are collapsed while hosting a
 * drawer): `display: none` would kill the absolutely-positioned drawer
 * hosts inside, so the pane drains to 0 width instead and its drawer
 * hosts escape through `position: absolute`.
 *
 * §10 Stacked (<800): bindings with `stacked: true` present their host as
 * a full-screen route instead of an edge drawer (see
 * `drawerPlan.overlayPaneClass`) — the same keep-alive element, filled
 * with `inset-0` over the `.layout-area`; every pane drains the same way.
 */
import type { Snippet } from "svelte";
import {
  RATIO_MAX,
  RATIO_MIN,
  RATIO_STEP,
  clampRatio,
  type LayoutNode,
  type MoveTarget,
  type PanelGroupAction,
  type PanelId,
  type SplitNode,
} from "$lib/layout/layoutModel";
import {
  overlayHostAttrs,
  overlayPaneClass,
  type DrawerBinding,
} from "$lib/layout/drawerPlan";
import TabGroup from "./TabGroup.svelte";

  // Self-import for recursion (Svelte 5's replacement for <svelte:self>).
  import SplitContainer from "./SplitContainer.svelte";

  let {
    node,
    renderPanel,
    hiddenPanels = [],
    collapsedIds = [],
    drainedIds = [],
    drawerPanes = [],
    drawerPanels = [],
    hideTabsFor = null,
    canMoveLeftFor,
    canMoveRightFor,
    onRatio,
    onActivateTab,
    onAddPanel,
    onPanelAction,
    onMovePanel,
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
     * its sibling then grows to fill the freed space. Nodes whose subtree
     * hosts a drawer must NOT be listed here (see `drainedIds`).
     */
    collapsedIds?: string[];
    /**
     * Node ids drained to zero flex basis WITHOUT display:none — tab
     * groups that are collapsed (or fully drawerized) while hosting a
     * drawer: their absolutely-positioned drawer hosts must stay rendered.
     */
    drainedIds?: string[];
    /**
     * Panes (by node id) rendered as overlay drawers — always leaf holders
     * whose whole pane becomes the drawer (tab-group members drawerize
     * per-panel through `drawerPanels` instead).
     */
    drawerPanes?: DrawerBinding[];
    /**
     * Panels (by PanelId) rendered as overlay drawers from within their
     * tab group's keep-alive hosts; passed down to every TabGroup, which
     * picks the entries matching its own tabs.
     */
    drawerPanels?: DrawerBinding[];
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
    onMovePanel?: (panel: PanelId, target: MoveTarget) => void;
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
  {@const drawerA = drawerPanes.find((d) => d.key === node.a.id) ?? null}
  {@const drawerB = drawerPanes.find((d) => d.key === node.b.id) ?? null}
  {@const drainedA = drainedIds.includes(node.a.id)}
  {@const drainedB = drainedIds.includes(node.b.id)}
  <!-- The rail consumes width from the tree, not from the left panes: on
       the right spine add the rail's share (ratio × rail width) back to `a`
       so toggling history only resizes the rightmost pane. -->
  {@const aBasis = rightRail
    ? `calc(${node.ratio * 100}% + ${node.ratio} * var(--history-rail-width, 0px))`
    : `${node.ratio * 100}%`}
  <div class="flex h-full w-full min-h-0 min-w-0">
    <div
      class={drawerA
        ? overlayPaneClass(drawerA)
        : "flex min-h-0 min-w-0 flex-col overflow-hidden [&[hidden]]:hidden [&>*]:flex-1 [&>*]:min-h-0"}
      hidden={drawerA ? undefined : hideA}
      style:flex={drawerA || drainedA
        ? "0 0 0%"
        : hideB || drawerB !== null || drainedB
          ? "1 1 0%"
          : `0 0 ${aBasis}`}
      {...(drawerA ? overlayHostAttrs(drawerA) : {})}
    >
      <SplitContainer
        node={node.a}
        {renderPanel}
        {hiddenPanels}
        {collapsedIds}
        {drainedIds}
        {drawerPanes}
        {drawerPanels}
        {hideTabsFor}
        {canMoveLeftFor}
        {canMoveRightFor}
        {onRatio}
        {onActivateTab}
        {onAddPanel}
        {onPanelAction}
        {onMovePanel}
        rightRail={false}
      />
    </div>
    <!-- A focusable separator is the ARIA-recommended pattern for split
         dividers (keyboard resize); the a11y lint can't see the role. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="relative z-1 mx-[-2px] w-[5px] shrink-0 cursor-col-resize touch-none focus-visible:outline-none {dragSplitId ===
      node.id
        ? "bg-[linear-gradient(to_right,transparent_1px,var(--primary)_1px,var(--primary)_4px,transparent_4px)]"
        : "bg-[linear-gradient(to_right,transparent_2px,var(--border)_2px,var(--border)_3px,transparent_3px)] hover:bg-[linear-gradient(to_right,transparent_1px,var(--primary)_1px,var(--primary)_4px,transparent_4px)] focus-visible:bg-[linear-gradient(to_right,transparent_1px,var(--primary)_1px,var(--primary)_4px,transparent_4px)]"}"
      hidden={hideA ||
        hideB ||
        drawerA !== null ||
        drawerB !== null ||
        drainedA ||
        drainedB}
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
    <div
      class={drawerB
        ? overlayPaneClass(drawerB)
        : "flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden [&[hidden]]:hidden [&>*]:flex-1 [&>*]:min-h-0"}
      hidden={drawerB ? undefined : hideB}
      style:flex={drawerB || drainedB ? "0 0 0%" : undefined}
      {...(drawerB ? overlayHostAttrs(drawerB) : {})}
    >
      <SplitContainer
        node={node.b}
        {renderPanel}
        {hiddenPanels}
        {collapsedIds}
        {drainedIds}
        {drawerPanes}
        {drawerPanels}
        {hideTabsFor}
        {canMoveLeftFor}
        {canMoveRightFor}
        {onRatio}
        {onActivateTab}
        {onAddPanel}
        {onPanelAction}
        {onMovePanel}
        {rightRail}
      />
    </div>
  </div>
{:else if node.kind === "tabs"}
  <TabGroup
    {node}
    {renderPanel}
    {hiddenPanels}
    {drawerPanels}
    hideTabs={node.id === hideTabsFor}
    canMoveLeft={groupCanMoveLeft}
    canMoveRight={groupCanMoveRight}
    onActivate={onActivateTab}
    onAdd={onAddPanel}
    onAction={onPanelAction}
    {onMovePanel}
  />
{:else}
  <div class="flex h-full w-full min-h-0 min-w-0 flex-col [&>*]:flex-1 [&>*]:min-h-0">
    {@render renderPanel(node.panel)()}
  </div>
{/if}
