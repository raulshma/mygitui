<script lang="ts">
  /**
   * ActivityBar — the VSCode-style vertical rail at the far left.
   *
   * One icon per workspace panel (status, branches, remotes, …): clicking
   * activates that panel in the primary sidebar, opening the sidebar first
   * when it is collapsed. The rail itself never collapses — it is what stays
   * visible when the sidebar is hidden, and the bottom chevron toggles the
   * sidebar without unmounting it (RepoView hides the pane, state survives).
   *
   * History gets a separate bottom placement because the workspace keeps it
   * on the right edge; `onHistory` toggles that secondary sidebar instead of
   * switching the primary one.
   */
  import { PANEL_META, panelLabel, type PanelId } from "$lib/layout/layoutModel";

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
    /** Panels shown as primary-sidebar icons, top to bottom. */
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

  /** Caps runaway counts; the full number stays in the title. */
  function badgeText(count: number): string {
    return count > 99 ? "99+" : String(count);
  }
</script>

<div class="activity-bar" role="toolbar" aria-orientation="vertical" aria-label="Activity bar">
  {#each panels as panel (panel)}
    {@const count = badges[panel] ?? 0}
    <button
      class="activity-btn"
      class:active={active === panel}
      type="button"
      data-activity-panel={panel}
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

  <div class="activity-bottom">
    <button
      class="activity-btn"
      class:active={historyActive}
      type="button"
      data-activity-panel="history"
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
    overflow-y: auto;
    scrollbar-width: none;
  }

  .activity-bar::-webkit-scrollbar {
    display: none;
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
