<script lang="ts">
  /**
   * StackedTabBar — the §10 Stacked (<800px) bottom tab bar, the
   * below-the-floor replacement for the vertical ActivityBar.
   *
   * The workspace shows ONE full-screen region at a time below 800px
   * (plan §10, "best-effort fallback"); this bar is its navigator: five
   * primary views — Files, Editor, Status, History, GitHub (the §10
   * matrix inventory) — plus a "More" overflow carrying every remaining
   * rail destination, so the rail's full panel inventory stays reachable
   * (§3 principle 4: collapse, don't hide).
   *
   * The component follows ActivityBar's contract (a `toolbar` of pressed
   * toggle buttons + the shared context-menu store's overflow) rotated to
   * a horizontal bottom row: same active pill affordance, same badge
   * placement (count bubble INSIDE the button), same "More" dot when an
   * overflow panel is the active view. Selecting an item — primary or
   * More — activates that panel's full-screen route in RepoView
   * (`onSelect`), which places the panel in the layout when needed; the
   * host elements themselves live in the split tree (keep-alive), this
   * bar only names the active one.
   *
   * Pure presentation: no layout writes here.
   */
  import {
    PANEL_META,
    panelIconData,
    type PanelId,
  } from "$lib/layout/layoutModel";
  import { stackedViewLabel } from "$lib/layout/stackedPlan";
  import { showMenuAtElement } from "$lib/components/menu/contextMenuStore.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";

  let {
    views,
    morePanels = [],
    active = null,
    badges = {},
    onSelect,
  }: {
    /** The five primary bar views, in bar order. */
    views: PanelId[];
    /** The "More" overflow panels (every other rail destination). */
    morePanels?: PanelId[];
    /** The active stacked view (`null` = none — non-stacked frames). */
    active?: PanelId | null;
    /** Optional small count bubble per view (e.g. changed files on Status). */
    badges?: Partial<Record<PanelId, number>>;
    onSelect: (panel: PanelId) => void;
  } = $props();

  const iconMore = $derived(appIcon("more-horiz")!);

  /** Caps runaway counts (ActivityBar idiom). */
  function badgeText(count: number): string {
    return count > 99 ? "99+" : String(count);
  }

  const moreActive = $derived(active !== null && morePanels.includes(active));
  /** Overflow panels with counts — the More button shows a small dot. */
  const moreBadgeCount = $derived(
    morePanels.reduce((sum, panel) => sum + (badges[panel] ?? 0), 0),
  );

  /** Overflow menu: one labeled, icon-carrying entry per hidden view. */
  function openMoreMenu(event: MouseEvent): void {
    showMenuAtElement(
      event.currentTarget as HTMLElement,
      morePanels.map((panel) => {
        const count = badges[panel] ?? 0;
        return {
          id: `stacked-panel-${panel}`,
          label:
            count > 0
              ? `${stackedViewLabel(panel)} (${badgeText(count)})`
              : stackedViewLabel(panel),
          icon: panelIconData(panel),
          run: () => onSelect(panel),
        };
      }),
      { align: "end", above: true },
    );
  }
</script>

<!-- Bottom chrome row (the ActivityBar's stacked-mode replacement):
     horizontal toolbar, equal-width items, icon over a micro label —
     the mobile bottom-bar grammar for the same navigation job. -->
<div
  class="stacked-tab-bar flex flex-none items-stretch gap-1 border-t border-border bg-popover px-1.5 py-1"
  role="toolbar"
  aria-orientation="horizontal"
  aria-label="Workspace views"
>
  {#each views as panel (panel)}
    {@const count = badges[panel] ?? 0}
    <button
      class="stacked-tab group relative flex min-w-0 flex-1 cursor-pointer flex-col items-center justify-center gap-0.5 rounded-md px-1 py-1 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary aria-pressed:bg-primary/15 aria-pressed:text-primary-ink"
      type="button"
      data-stacked-tab={panel}
      aria-pressed={active === panel}
      aria-label={stackedViewLabel(panel)}
      onclick={() => onSelect(panel)}
    >
      <svg
        class="h-[1.15rem] w-[1.15rem] flex-none"
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
      <span class="text-[0.65rem] leading-none">{stackedViewLabel(panel)}</span>
      {#if count > 0}
        <!-- Badge anchored INSIDE the button (ActivityBar §5.4 rule): it
             may never bleed past the button edge onto the bar border. -->
        <span
          class="stacked-badge absolute right-1 top-0.5 grid h-4 min-w-[0.8rem] place-items-center rounded-full border-2 border-popover bg-primary px-[0.14rem] text-[0.625rem] font-bold leading-none tracking-[-0.02em] text-primary-foreground tabular-nums group-aria-pressed:border-accent"
          aria-hidden="true"
        >{badgeText(count)}</span>
      {/if}
    </button>
  {/each}

  {#if morePanels.length > 0}
    <button
      class="stacked-tab relative flex min-w-0 flex-1 cursor-pointer flex-col items-center justify-center gap-0.5 rounded-md px-1 py-1 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary aria-pressed:bg-primary/15 aria-pressed:text-primary-ink"
      type="button"
      data-stacked-tab="more"
      aria-label="More views"
      aria-pressed={moreActive}
      aria-haspopup="menu"
      onclick={openMoreMenu}
    >
      <Icon icon={iconMore} size={18} />
      <span class="text-[0.65rem] leading-none">More</span>
      {#if moreBadgeCount > 0}
        <span
          class="stacked-badge dot absolute right-1.5 top-1 grid h-[0.45rem] w-[0.45rem] min-w-[0.45rem] place-items-center rounded-full bg-primary p-0 leading-none text-primary-foreground tabular-nums"
          aria-hidden="true"
        ></span>
      {/if}
    </button>
  {/if}
</div>
