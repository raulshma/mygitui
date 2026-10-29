<script lang="ts">
  /**
   * ActivityBar — the VSCode-style vertical rail at the far left.
   *
   * Only the core panels (`RAIL_PANELS`: status, branches, remotes, terminal)
   * get rail buttons; every other panel lives behind the "More panels"
   * overflow menu so the rail stays scannable without hovering each icon
   * (UI review P1 #1). Hover/focus shows a small label flyout beside the
   * rail — faster than a native tooltip and not clipped, because the rail
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
   */
  import {
    PANEL_META,
    RAIL_PANELS,
    panelIconData,
    panelLabel,
    type PanelId,
  } from "$lib/layout/layoutModel";
  import { showMenuAtElement } from "$lib/components/menu/contextMenuStore.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import iconMore from "@ktibow/iconset-material-symbols/more-horiz";

  let {
    panels,
    active = null,
    historyActive = false,
    historyCanHide = true,
    collapsed = false,
    canCollapse = true,
    badges = {},
    onSelect,
    onHistory,
    onToggleCollapse,
  }: {
    /** Panels the workspace can show in the primary sidebar. */
    panels: PanelId[];
    /** Currently active primary-sidebar panel (null = none). */
    active?: PanelId | null;
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
  } = $props();

  /** Rail buttons, registry order first: the core panels that are placed. */
  const railPanels = $derived(
    RAIL_PANELS.filter((panel) => panels.includes(panel)),
  );
  /** Everything else, behind the overflow menu (order = `panels`). */
  const morePanels = $derived(
    panels.filter((panel) => !RAIL_PANELS.includes(panel)),
  );
  const moreActive = $derived(active !== null && morePanels.includes(active));

  /** Overflow panels with counts — the More button shows a small dot. */
  const moreBadgeCount = $derived(
    morePanels.reduce((sum, panel) => sum + (badges[panel] ?? 0), 0),
  );

  /** Caps runaway counts; the full number stays in the title. */
  function badgeText(count: number): string {
    return count > 99 ? "99+" : String(count);
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
          run: () => onSelect(panel),
        };
      }),
      { align: "end", above: true },
    );
  }
</script>

<div class="activity-bar" role="toolbar" aria-orientation="vertical" aria-label="Activity bar">
  {#each railPanels as panel (panel)}
    {@const count = badges[panel] ?? 0}
    <button
      class="activity-btn"
      class:active={active === panel}
      type="button"
      data-activity-panel={panel}
      data-label={panelLabel(panel)}
      aria-label={panelLabel(panel)}
      aria-pressed={active === panel}
      title={count > 0
        ? `${panelLabel(panel)} — ${count} ${count === 1 ? "item" : "items"}`
        : panelLabel(panel)}
      onclick={() => onSelect(panel)}
    >
      <svg
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
        <span class="activity-badge" aria-hidden="true">{badgeText(count)}</span>
      {/if}
    </button>
  {/each}

  {#if morePanels.length > 0}
    <button
      class="activity-btn"
      class:active={moreActive}
      type="button"
      data-label="More panels"
      aria-label="More panels"
      aria-pressed={moreActive}
      aria-haspopup="menu"
      title="More panels"
      onclick={openMoreMenu}
    >
      <Icon icon={iconMore} size={18} />
      {#if moreBadgeCount > 0}
        <span class="activity-badge dot" aria-hidden="true"></span>
      {/if}
    </button>
  {/if}

  <div class="activity-bottom">
    <button
      class="activity-btn"
      class:active={historyActive}
      type="button"
      data-activity-panel="history"
      data-label="History"
      aria-label="History"
      aria-pressed={historyActive}
      title="History"
      disabled={historyActive && !historyCanHide}
      onclick={onHistory}
    >
      <svg
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
    </button>
    <button
      class="activity-btn collapse-btn"
      type="button"
      data-label={collapsed ? "Show sidebar" : "Hide sidebar"}
      aria-label={collapsed ? "Show sidebar" : "Hide sidebar"}
      aria-pressed={collapsed}
      title={collapsed ? "Show sidebar" : "Hide sidebar"}
      disabled={!canCollapse && !collapsed}
      onclick={onToggleCollapse}
    >
      <svg
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
    </button>
  </div>
</div>

<style>
  .activity-bar {
    flex: none;
    width: 2.5rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.125rem;
    padding: 0.25rem 0;
    background: var(--m3-surface-container, var(--m3-surface));
    border-right: 1px solid var(--m3-outline-variant);
  }

  .activity-btn {
    position: relative;
    flex: none;
    width: 2.25rem;
    height: 2.25rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    border: none;
    border-radius: var(--m3-shape-small, 8px);
    background: none;
    color: var(--m3-on-surface-variant);
    cursor: pointer;
  }

  .activity-btn svg {
    width: 1.15rem;
    height: 1.15rem;
  }

  .activity-btn:hover:not(:disabled),
  .activity-btn:focus-visible:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .activity-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .activity-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .activity-btn.active {
    color: var(--m3-primary);
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  /* Label flyout beside the rail (hover/focus) — quicker and more readable
     than a native tooltip; the rail no longer scrolls, so nothing clips it. */
  .activity-btn::after {
    content: attr(data-label);
    position: absolute;
    left: calc(100% + 0.4rem);
    top: 50%;
    transform: translateY(-50%) translateX(-0.25rem);
    padding: 0.25rem 0.55rem;
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-inverse-surface, rgb(0 0 0 / 0.85));
    color: var(--m3-inverse-on-surface, #fff);
    font-size: 0.75rem;
    line-height: 1.3;
    white-space: nowrap;
    opacity: 0;
    pointer-events: none;
    transition:
      opacity 0.12s ease 0.3s,
      transform 0.12s ease 0.3s;
    z-index: 70;
  }

  .activity-btn:hover::after,
  .activity-btn:focus-visible::after {
    opacity: 1;
    transform: translateY(-50%) translateX(0);
  }

  /* Changed-item count bubble (Status). Small, high-contrast, capped at 99+. */
  .activity-badge {
    position: absolute;
    top: 0.1rem;
    right: 0.05rem;
    display: grid;
    place-items: center;
    min-width: 0.8rem;
    height: 0.8rem;
    padding: 0 0.14rem;
    border: 2px solid var(--m3-surface-container, var(--m3-surface));
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font-size: 0.5rem;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    line-height: 1;
    letter-spacing: -0.02em;
  }

  /* Dot variant for the overflow button (aggregate, no number). */
  .activity-badge.dot {
    min-width: 0.45rem;
    width: 0.45rem;
    height: 0.45rem;
    padding: 0;
    border: none;
  }

  .activity-btn.active .activity-badge {
    border-color: var(--m3-surface-container-high, var(--m3-surface));
  }

  /* VSCode-style active marker on the rail edge. */
  .activity-btn.active::before {
    content: "";
    position: absolute;
    left: -0.125rem;
    top: 20%;
    bottom: 20%;
    width: 2px;
    border-radius: 9999px;
    background: var(--m3-primary);
  }

  .activity-bottom {
    margin-top: auto;
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.125rem;
    padding-top: 0.25rem;
    border-top: 1px solid var(--m3-outline-variant);
  }

  .collapse-btn {
    color: var(--m3-on-surface-variant);
  }

  .collapse-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
