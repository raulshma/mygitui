<script lang="ts">
  /**
   * ActivityBar — the VSCode-style vertical rail at the far left.
   *
   * Only the core panels (`RAIL_PANELS`: status, branches, remotes, terminal)
   * get rail buttons; every other panel lives behind the "More panels"
   * overflow menu so the rail stays scannable without hovering each icon
   * (UI review P1 #1). Hover/focus shows a small label flyout beside the
   * rail (`activity-btn`'s `data-label` ::after, Workstream A §5.4 keeps
   * this) — faster than a native tooltip and not clipped, because the rail
   * no longer scrolls.
   *
   * Clicking any entry activates that panel in the primary sidebar, opening
   * the sidebar first when it is collapsed. The rail itself never collapses
   * — it is what stays visible when the sidebar is hidden, and the bottom
   * chevron toggles the sidebar without unmounting it (RepoView hides the
   * pane, state survives).
   *
   * History gets a separate bottom placement because the workspace keeps it
   * on the right edge; `onHistory` toggles that secondary sidebar instead of
   * switching the primary one.
   *
   * Workstream A (§5.4): the "More panels" "⋯" moved into the bottom group
   * (above History) so the top rail is only core + pinned panels, the count
   * badge sits INSIDE the rail width (anchored within the button, no bleed
   * over the rail edge), and the active item gets a filled-pill affordance
   * (tinted rounded fill + edge indicator) rather than color alone.
   *
 * §10 (responsive matrix): panels converted to overlay drawers in
 * Medium/Narrow keep their rail item as the drawer's toggle —
 * `drawerActive` marks the panels whose drawer is currently open so the
 * pill + `aria-pressed` reflect it alongside the docked `active` panel.
 *
 * §10 Stacked (<800): the rail hides — the bottom StackedTabBar replaces
 * it as the workspace navigator, carrying the same inventory (5 primary
 * views + the rest behind "More").
   *
   * Overflow panels can be pinned: with `onTogglePin` set, each "More
   * panels" entry grows a pin action and pinned panels get their own rail
   * button (right-click unpins). Pin state lives in the `railPins` store —
   * this component only renders what it is given.
   */
  import {
    PANEL_META,
    RAIL_PANELS,
    panelIconData,
    panelLabel,
    type PanelId,
  } from "$lib/layout/layoutModel";
  import {
    showMenuAt,
    showMenuAtElement,
  } from "$lib/components/menu/contextMenuStore.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Button } from "$lib/components/ui/button/index.js";

  let {
    panels,
    pinned = [],
    active = null,
    drawerActive = {},
    historyActive = false,
    historyCanHide = true,
    collapsed = false,
    canCollapse = true,
    badges = {},
    onSelect,
    onHistory,
    onToggleCollapse,
    onTogglePin = null,
  }: {
    /** Panels the workspace can show in the primary sidebar. */
    panels: PanelId[];
    /** Overflow panels pinned to the rail (in pin order). */
    pinned?: PanelId[];
    /** Currently active primary-sidebar panel (null = none). */
    active?: PanelId | null;
    /** Panels whose overlay drawer is open (§10): pill + aria-pressed. */
    drawerActive?: Partial<Record<PanelId, boolean>>;
    /** Whether the right-hand history sidebar is visible/active. */
    historyActive?: boolean;
    /** Allow hiding the visible history view (false when it is the last pane). */
    historyCanHide?: boolean;
    /** Primary sidebar hidden behind this rail. */
    collapsed?: boolean;
    /** Allow hiding the primary sidebar (false when it is the only pane). */
    canCollapse?: boolean;
    /** Optional small count bubble per panel (e.g. changed files on Status). */
    badges?: Partial<Record<PanelId, number>>;
    onSelect: (panel: PanelId) => void;
    onHistory: () => void;
    onToggleCollapse: () => void;
    /** Pin/unpin hook; the pin affordances render only when set. */
    onTogglePin?: ((panel: PanelId) => void) | null;
  } = $props();

  const iconMore = $derived(appIcon("more-horiz")!);
  const iconPin = $derived(appIcon("push-pin")!);

  /** Rail chrome (flyout + active indicator) lives in `@utility activity-btn`.
   *  Active = filled pill (tinted rounded fill, not just a color swap). */
  const railBtn =
    "activity-btn relative flex-none rounded-lg text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:bg-accent focus-visible:text-foreground focus-visible:outline-2 focus-visible:outline-primary focus-visible:-outline-offset-2 focus-visible:ring-0 [&.active]:bg-primary/15 [&.active]:text-primary-ink [&.active]:shadow-[inset_0_0_0_1px_color-mix(in_srgb,var(--primary)_25%,transparent)]";

  /** Default rail buttons (registry order first: the core panels). */
  const defaultRail = $derived(
    RAIL_PANELS.filter((panel) => panels.includes(panel)),
  );
  /** Pinned buttons after the defaults, in pin order. */
  const pinnedRail = $derived(
    pinned.filter(
      (panel) => panels.includes(panel) && !RAIL_PANELS.includes(panel),
    ),
  );
  /** Every button rendered on the rail (defaults, then pins). */
  const railPanels = $derived([...defaultRail, ...pinnedRail]);
  /** Everything else, behind the overflow menu (order = `panels`). */
  const morePanels = $derived(
    panels.filter((panel) => !railPanels.includes(panel)),
  );
  const moreActive = $derived(active !== null && morePanels.includes(active));

  /** Overflow panels with counts — the More button shows a small dot. */
  const moreBadgeCount = $derived(
    morePanels.reduce((sum, panel) => sum + (badges[panel] ?? 0), 0),
  );

  /** Caps runaway counts; the full number stays in the flyout label. */
  function badgeText(count: number): string {
    return count > 99 ? "99+" : String(count);
  }

  /** A rail item is "on" when it is the docked active panel OR its drawer
   *  is open (§10 drawer toggles keep the pill affordance honest). */
  function isOn(panel: PanelId): boolean {
    return active === panel || (drawerActive[panel] ?? false);
  }

  /** Overflow menu: one labeled, icon-carrying entry per hidden panel. */
  function openMoreMenu(event: MouseEvent): void {
    showMenuAtElement(
      event.currentTarget as HTMLElement,
      morePanels.map((panel) => {
        const count = badges[panel] ?? 0;
        return {
          id: `panel-${panel}`,
          label:
            count > 0
              ? `${panelLabel(panel)} (${badgeText(count)})`
              : panelLabel(panel),
          icon: panelIconData(panel),
          action: onTogglePin
            ? {
                label: "Pin to rail",
                icon: iconPin,
                run: () => onTogglePin(panel),
              }
            : undefined,
          run: () => onSelect(panel),
        };
      }),
      { align: "end", above: true },
    );
  }

  /** Right-click on a pinned button unpins it (defaults stay fixed). */
  function onRailContext(event: MouseEvent, panel: PanelId): void {
    if (!onTogglePin || !pinned.includes(panel)) return;
    showMenuAt(event, [
      {
        id: `unpin-${panel}`,
        label: "Unpin from rail",
        icon: iconPin,
        run: () => onTogglePin(panel),
      },
    ]);
  }
</script>

<div
  class="activity-bar flex w-10 flex-none flex-col items-center gap-0.5 border-r border-border bg-popover py-1 wm-stacked:hidden"
  role="toolbar"
  aria-orientation="vertical"
  aria-label="Activity bar"
>
  {#each railPanels as panel (panel)}
    {@const count = badges[panel] ?? 0}
    <Button
      class="{railBtn} {isOn(panel) ? 'active' : ''}"
      variant="ghost"
      size="icon-lg"
      type="button"
      data-activity-panel={panel}
      data-label={count > 0
        ? `${panelLabel(panel)} — ${count} ${count === 1 ? "item" : "items"}`
        : panelLabel(panel)}
      aria-label={panelLabel(panel)}
      aria-pressed={isOn(panel)}
      onclick={() => onSelect(panel)}
      oncontextmenu={(event) => onRailContext(event, panel)}
    >
      <svg
        class="h-5 w-5"
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
      {#if count > 0}
        <!-- Badge anchored INSIDE the button (0.25rem inset): it may never
             bleed past the button edge onto the rail border (§5.4). -->
        <span
          class="activity-badge absolute right-0.5 top-ui-1 grid h-4 min-w-[0.8rem] place-items-center rounded-full border-2 border-popover bg-primary px-[0.14rem] text-ui-micro font-bold leading-none tracking-[-0.02em] text-primary-foreground tabular-nums [.active_&]:border-accent"
          aria-hidden="true"
          >{badgeText(count)}</span
        >
      {/if}
    </Button>
  {/each}

  <div
    class="activity-bottom mt-auto flex w-full flex-col items-center gap-0.5 border-t border-border pt-1"
  >
    {#if morePanels.length > 0}
      <!-- Workstream A §5.4: the pinned/unpinned overflow "⋯" lives in the
           bottom group, so the top rail is only core + pinned panels. -->
      <Button
        class="{railBtn} {moreActive ? 'active' : ''}"
        variant="ghost"
        size="icon-lg"
        type="button"
        data-label="More panels"
        aria-label="More panels"
        aria-pressed={moreActive}
        aria-haspopup="menu"
        onclick={openMoreMenu}
      >
        <Icon icon={iconMore} size={18} />
        {#if moreBadgeCount > 0}
          <span
            class="activity-badge dot absolute right-0.5 top-ui-1 grid h-[0.45rem] w-[0.45rem] min-w-[0.45rem] place-items-center rounded-full border-0 bg-primary p-0 leading-none tracking-[-0.02em] text-primary-foreground tabular-nums"
            aria-hidden="true"
          ></span>
        {/if}
      </Button>
    {/if}
    <Button
      class="{railBtn} {historyActive ? 'active' : ''}"
      variant="ghost"
      size="icon-lg"
      type="button"
      data-activity-panel="history"
      data-label="History"
      aria-label="History"
      aria-pressed={historyActive}
      disabled={historyActive && !historyCanHide}
      onclick={onHistory}
    >
      <svg
        class="h-5 w-5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d={PANEL_META.history.icon} />
      </svg>
    </Button>
    <Button
      class="{railBtn} collapse-btn"
      variant="ghost"
      size="icon-lg"
      type="button"
      data-label={collapsed ? "Show sidebar" : "Hide sidebar"}
      aria-label={collapsed ? "Show sidebar" : "Hide sidebar"}
      aria-pressed={collapsed}
      disabled={!canCollapse && !collapsed}
      onclick={onToggleCollapse}
    >
      <svg
        class="h-5 w-5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d={collapsed ? "M9 18l6-6-6-6" : "M15 18l-6-6 6-6"} />
      </svg>
    </Button>
  </div>
</div>
