<script lang="ts">
  /**
   * Repository tab strip (M1). Bound to the global tab store — no props
   * except `openFolder` (the "+" button callback) and an optional `trailing`
   * snippet for window-level utilities (status cluster, overflow menu).
   *
   * Workstream A (§5.2): the strip renders INSIDE the title bar (TitleBar's
   * `tabs` snippet) — one 40px top row; this component owns no row chrome
   * (no border/background) anymore, only the strip content.
   *
   * Since the §4.3 shared-primitives pass this is a thin surface over
   * {@link UnifiedTabStrip}: the strip engine owns the tablist semantics,
   * roving-tabindex keyboard navigation, scroll chrome (wheel translation,
   * edge fades, reveal-active) and the "⋯" overflow menu listing tabs
   * hidden by width. Everything repo-specific stays here — the cell markup
   * (remote cloud glyph, dirty dot, ahead/behind chip), middle-click close,
   * the shared context menu, and drag-to-reorder.
   *
   * Accessibility: roving tabindex — the active tab is tab-reachable
   * (tabindex 0), the rest are programmatic (tabindex -1). Arrow/Home/End
   * keys move focus; Enter/Space activate the focused tab. Overflowed
   * (ellipsis-truncated) names announce in full: each tab's accessible name
   * is "name — root" (label-in-name compliant) and the Hint tooltip shows
   * the root on hover.
   *
   * Remote tabs (`deviceId` set) lead with a small cloud glyph — dimmed while
   * the device is offline (`remoteDevices.statusOf`, unknown ids report
   * "disconnected") — so a repository living on another machine is
   * identifiable at a glance; the tooltip's `ssh://` root says the same.
   *
   * Status badges (`tabBadge.ts`, the status bar's derivation): a dirty
   * repository shows a small dot after its name (tooltip "N changed") and a
   * branch that is ahead of / behind its upstream shows a compact ↑n ↓n chip
   * (tooltip "N ahead / N behind"). Both stay out of the way when zero, and
   * counts cap at "99+" so a huge number cannot widen the tab.
   *
   * Drag-to-reorder (mouse only, HTML5 dnd — the RebasePlanner idiom, no
   * libraries): the tab label is `draggable`; dragging it over another tab
   * shows an insertion bar on the side of the hovered tab label's midpoint
   * the pointer is on, and dropping there splices the strip order through
   * `reorderTab` (persisted like open/close). The handlers only act on
   * drags this strip started (dataTransfer type marker), so external file
   * drops are untouched; the dragged tab dims and dropping onto itself,
   * the strip background, or outside the window is a no-op.
   *
   * Focus order is sane: each close button follows its tab in DOM order, so
   * Tab runs tab → its close → next reachable control.
   */
  import type { Snippet } from "svelte";
  import {
    closeTab,
    reorderTab,
    setActive,
    showRecentRepos,
    SSH_SCHEME,
    tabStore,
    type Tab,
  } from "$lib/stores/tabs.svelte";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { remoteDevices } from "$lib/stores/remoteDevices.svelte";
  import { favoriteRepos } from "$lib/stores/favoriteRepos.svelte";
  import { isTauri } from "$lib/entry/dragdrop";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { describeError } from "$lib/errors";
  import { copyText } from "$lib/clipboard";
  import { toast } from "$lib/toast";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import UnifiedTabStrip from "$lib/components/layout/UnifiedTabStrip.svelte";
  import { widthMode } from "$lib/layout/widthMode.svelte";
  import {
    aheadBehindLabel,
    badgeCount,
    changedLabel,
    tabAriaSuffix,
    tabBadge,
  } from "$lib/components/tabBadge";
  import { forgeChips, type TabAttention } from "$lib/forge/chips.svelte";
  import { formatRelativeTime } from "$lib/git/format";
  import iconClose from "@ktibow/iconset-material-symbols/close";
  import iconAdd from "@ktibow/iconset-material-symbols/add";

  let {
    openFolder,
    trailing = undefined,
  }: {
    openFolder?: () => void;
    /** Window-level controls rendered at the right edge of the strip. */
    trailing?: Snippet;
  } = $props();

  const iconHome = $derived(appIcon("home-rounded")!);
  const iconCloud = $derived(appIcon("cloud")!);
  const iconChevron = $derived(appIcon("chevron-down")!);

  // -- §10 Stacked (<800): the strip collapses to a repo dropdown ----------
  //
  // The merged title-bar row keeps its height; the tab CELLS give way to a
  // single dropdown trigger naming the active repository (the §10 matrix's
  // "tabs → dropdown"). The home button and the trailing status cluster
  // stay (collapse, don't hide), so every strip affordance below the floor
  // has an owner: switching lives in the dropdown, closing + recents are
  // menu rows, and the window drag region keeps the leftover row space.

  /** True below the responsive floor (drives the dropdown swap). */
  const stacked = $derived(widthMode.current === "stacked");
  /** The repository the trigger names (tabs exist whenever this renders). */
  const activeTab = $derived(
    tabStore.tabs.find((entry) => entry.id === tabStore.activeId) ?? null,
  );

  /** Strip items: the tab itself (cell markup reads every field) + the
   * ⋯ overflow menu label. */
  type RepoStripTab = Tab & { label: string };
  const stripTabs: RepoStripTab[] = $derived(
    tabStore.tabs.map((entry) => ({ ...entry, label: entry.name })),
  );

  // -- GitHub attention badges (E1, §9.2) --------------------------------------

  /**
   * Background repos with GitHub attention (failing/degraded/warning/
   * unpushed chips) show a small badge on their tab; the tooltip discloses
   * the state and its staleness ("as of Xm ago" — the last successful
   * snapshot fetch). The ACTIVE repo shows its state in the footer chips,
   * so its tab stays badge-free.
   */
  let forgeNow = $state(Date.now());
  const attentionById: Record<string, TabAttention | null> = $derived.by(() => {
    const map: Record<string, TabAttention | null> = {};
    for (const tab of tabStore.tabs) {
      if (tab.id === tabStore.activeId) continue;
      map[tab.id] = forgeChips.attentionFor(tab.id, forgeNow);
    }
    return map;
  });
  const hasAttentionBadges = $derived(
    Object.values(attentionById).some((entry) => entry?.attention === true),
  );
  /** 30s ticker — staleness labels only need minute resolution. */
  $effect(() => {
    if (!hasAttentionBadges) return;
    forgeNow = Date.now();
    const timer = setInterval(() => (forgeNow = Date.now()), 30_000);
    return () => clearInterval(timer);
  });

  /** Tooltip line of one attention badge: state + staleness. */
  function attentionTitle(tabId: string): string {
    const attention = attentionById[tabId];
    if (attention === null || !attention.attention) return "";
    const asOf =
      attention.fetchedAtMs === null
        ? ""
        : ` — as of ${formatRelativeTime(Math.floor(attention.fetchedAtMs / 1000), {
            now: Math.floor(forgeNow / 1000),
          })}`;
    return `GitHub: ${attention.text}${asOf}`;
  }

  // -- middle-click close ------------------------------------------------------

  function onAuxClick(event: MouseEvent, tabId: string): void {
    if (event.button === 1) {
      // Middle click closes; prevent the browser's autoscroll affordance.
      event.preventDefault();
      void closeTab(tabId);
    }
  }

  // -- drag-to-reorder (HTML5 dnd, RebasePlanner idiom — no libraries) -------

  /** dataTransfer type marker: strip drag handlers ignore every other drag. */
  const TAB_DRAG_TYPE = "application/x-mygitui-tab";

  /** Id of the tab being dragged, `null` when no strip drag is in flight. */
  let dragTabId = $state<string | null>(null);
  /** Id of the tab under the pointer (drives the insertion indicator). */
  let dropTargetId = $state<string | null>(null);
  /** Which side of the hovered tab's midpoint the drop would land on. */
  let dropSide = $state<"before" | "after">("before");

  function onDragStart(event: DragEvent, tabId: string): void {
    dragTabId = tabId;
    // Marker type first (what our handlers sniff), text/plain as the
    // human-readable fallback — the RebasePlanner payload shape.
    event.dataTransfer?.setData(TAB_DRAG_TYPE, tabId);
    event.dataTransfer?.setData("text/plain", tabId);
    if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
  }

  /** True only for drags this strip started (external file drops excluded). */
  function isTabDrag(event: DragEvent): boolean {
    return (
      dragTabId !== null &&
      (event.dataTransfer?.types.includes(TAB_DRAG_TYPE) ?? false)
    );
  }

  function onTabDragOver(event: DragEvent, tab: Tab): void {
    if (!isTabDrag(event)) return;
    if (tab.id === dragTabId) {
      // Hovering the dragged tab: no drop target (dropping onto itself is
      // a no-op), so any stale indicator goes away.
      dropTargetId = null;
      return;
    }
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
    // The insertion side is the hovered tab LABEL's midpoint, not the tab
    // cell's: the cell extends past the label to the close button, so a
    // cell-midpoint test put the label's own center on the "before" side —
    // the natural drop (onto the next tab's label) computed as a silent
    // no-op (insert before the tab it is already before).
    const label = (event.currentTarget as HTMLElement | null)
      ?.querySelector<HTMLElement>("button[role='tab']");
    const bounds = label?.getBoundingClientRect();
    dropSide =
      bounds && event.clientX >= bounds.left + bounds.width / 2 ? "after" : "before";
    dropTargetId = tab.id;
  }

  function onTabDrop(event: DragEvent, index: number): void {
    if (!isTabDrag(event)) return;
    event.preventDefault();
    // In-flight state is primary; the dataTransfer payload is the fallback
    // (the RebasePlanner shape) should the state ever be lost mid-drag.
    const raw = event.dataTransfer?.getData(TAB_DRAG_TYPE) ?? "";
    const id = raw !== "" ? raw : dragTabId;
    if (id !== null) {
      reorderTab(id, dropSide === "after" ? index + 1 : index);
    }
    clearDrag();
  }

  /** Fires on the source element whether or not the drop was accepted. */
  function onDragEnd(): void {
    clearDrag();
  }

  function clearDrag(): void {
    dragTabId = null;
    dropTargetId = null;
    dropSide = "before";
  }

  /**
   * Right-click menu (global context-menu store, BranchSwitcher-style):
   * close variants + path utilities. Close loops snapshot the strip first —
   * closeTab splices as each close settles, so live iteration would skip.
   * Remote tabs (`ssh://`) have no local path: Copy still works (the URI),
   * Reveal is disabled.
   */
  function tabMenu(event: MouseEvent, tab: Tab): void {
    const others = tabStore.tabs.filter((entry) => entry.id !== tab.id);
    const entries: MenuEntry[] = [
      { id: "close", label: "Close", run: () => void closeTab(tab.id) },
      {
        id: "close-others",
        label: "Close Others",
        disabled: others.length === 0,
        run: () => {
          for (const entry of others) void closeTab(entry.id);
        },
      },
      {
        id: "close-all",
        label: "Close All",
        run: () => {
          for (const entry of [...tabStore.tabs]) void closeTab(entry.id);
        },
      },
      { separator: true },
      {
        id: "favorite",
        label: favoriteRepos.has(tab.root) ? "Remove Favorite" : "Add to Favorites",
        run: () => {
          const now = favoriteRepos.toggle(tab.root);
          toast(
            now ? `Added ${tab.name} to favorites` : `Removed ${tab.name} from favorites`,
            { kind: "info" },
          );
        },
      },
      {
        id: "copy-path",
        label: "Copy Repo Path",
        run: () => void copyText(tab.root),
      },
      {
        id: "reveal",
        label: "Reveal Repository",
        disabled: tab.root.startsWith(SSH_SCHEME),
        run: () => void revealRepo(tab.root),
      },
    ];
    showMenuAt(event, entries);
  }

  /** Opens the repo root in the platform file manager (opener plugin). */
  async function revealRepo(root: string): Promise<void> {
    if (!isTauri()) return; // browser / e2e: no file manager to reveal in
    try {
      await revealItemInDir(root);
    } catch (err) {
      toast(`Reveal failed: ${describeError(err)}`, { kind: "error" });
    }
  }
</script>

<div class="tabbar flex min-w-0 flex-1 items-stretch">
  <Hint label="Recent repositories" side="bottom">
    {#snippet child({ props })}
      <Button
        class="tab-home ml-1 size-7 flex-none self-center rounded-lg aria-[pressed=true]:bg-secondary aria-[pressed=true]:text-secondary-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring focus-visible:ring-0"
        variant="ghost"
        size="icon-sm"
        type="button"
        {...props}
        aria-pressed={tabStore.recentsOpen}
        onclick={showRecentRepos}
      >
        <Icon icon={iconHome} size={16} />
      </Button>
    {/snippet}
  </Hint>
  {#if stacked}
    <!-- §10 Stacked: the repo tabs collapse to one dropdown trigger naming
         the active repository; the menu lists every open repository (the
         UnifiedTabStrip overflow's job at wider tiers) plus the strip's
         management rows (recents overlay, close) so nothing is lost. -->
    <DropdownMenu.Root>
      <DropdownMenu.Trigger
        class="repo-menu ml-1 inline-flex h-7 min-w-0 max-w-[16rem] cursor-pointer items-center gap-1 rounded-lg px-2 text-sm font-medium text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring aria-expanded:bg-secondary aria-expanded:text-secondary-foreground"
        aria-label={activeTab !== null
          ? `Open repositories — ${activeTab.name}`
          : "Open repositories"}
      >
        {#if activeTab !== null}
          {#if activeTab.deviceId !== null}
            <span
              class="remote-mark inline-flex flex-none items-center justify-center text-muted-foreground {remoteDevices.statusOf(activeTab.deviceId) === 'connected'
                ? ''
                : 'opacity-50'}"
              aria-hidden="true"
            >
              <Icon icon={iconCloud} size={13} />
            </span>
          {/if}
          <span class="name block min-w-0 truncate">{activeTab.name}</span>
          {#if tabBadge(activeTab.status).changed > 0}
            <span
              class="dirty-mark inline-block size-1.5 flex-none rounded-full bg-primary"
              role="img"
              aria-label={changedLabel(tabBadge(activeTab.status).changed)}
            ></span>
          {/if}
        {:else}
          <span class="name block min-w-0 truncate">Repositories</span>
        {/if}
        <Icon icon={iconChevron} size={14} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Content align="start" class="min-w-44 max-w-[calc(100vw-2rem)]">
        {#each tabStore.tabs as tab (tab.id)}
          <DropdownMenu.Item
            class="repo-menu-item cursor-pointer {tab.id === tabStore.activeId
              ? 'bg-accent text-accent-foreground'
              : ''}"
            onclick={() => setActive(tab.id)}
          >
            <span class="flex min-w-0 items-center gap-1.5">
              {#if tab.deviceId !== null}
                <span
                  class="remote-mark inline-flex flex-none items-center justify-center text-muted-foreground {remoteDevices.statusOf(tab.deviceId) === 'connected'
                    ? ''
                    : 'opacity-50'}"
                  aria-hidden="true"
                >
                  <Icon icon={iconCloud} size={13} />
                </span>
              {/if}
              <span class="min-w-0 truncate">{tab.name}</span>
              {#if tabBadge(tab.status).changed > 0}
                <span
                  class="dirty-mark inline-block size-1.5 flex-none rounded-full bg-primary"
                  role="img"
                  aria-label={changedLabel(tabBadge(tab.status).changed)}
                ></span>
              {/if}
            </span>
          </DropdownMenu.Item>
        {/each}
        <DropdownMenu.Separator />
        <DropdownMenu.Item class="cursor-pointer" onclick={showRecentRepos}>
          Recent repositories…
        </DropdownMenu.Item>
        {#if activeTab !== null}
          <DropdownMenu.Item
            class="cursor-pointer text-destructive data-[highlighted]:text-destructive"
            onclick={() => void closeTab(activeTab.id)}
          >
            Close {activeTab.name}
          </DropdownMenu.Item>
        {/if}
      </DropdownMenu.Content>
    </DropdownMenu.Root>
  {:else}
    <UnifiedTabStrip
      tabs={stripTabs}
      activeId={tabStore.activeId}
      label="Open repositories"
      class="flex-1"
      stripClass="tabstrip gap-0.5 px-2 pt-0.5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      onActivate={(item) => setActive(item.id)}
      onStripContextMenu={(event) => event.preventDefault()}
    >
      {#snippet tab(item, cell)}
      {@const isActive = cell.active}
      {@const dropping = dropTargetId === item.id && dragTabId !== null}
      {@const deviceConnected =
        item.deviceId !== null && remoteDevices.statusOf(item.deviceId) === "connected"}
      {@const badge = tabBadge(item.status)}
      <!-- Dragover/drop live on the tab cell (label + close area both accept
           a drop); dragstart only from the label — the close button stays a
           plain click target. -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="tab relative flex min-w-0 max-w-64 items-stretch rounded-t-lg border-b-2 border-transparent transition-colors"
        class:active={isActive}
        class:bg-accent={isActive}
        class:border-b-primary={isActive}
        class:opacity-45={dragTabId === item.id}
        ondragover={(event) => onTabDragOver(event, item)}
        ondrop={(event) => onTabDrop(event, cell.index)}
      >
        {#if dropping}
          <!-- Insertion bar (no layout shift): lands on the hovered tab's
               edge the pointer is past. -->
          <span
            class="pointer-events-none absolute inset-y-0.5 w-0.5 rounded-full bg-primary {dropSide === "before" ? "-left-px" : "-right-px"}"
            aria-hidden="true"
          ></span>
        {/if}
        <!-- font-weight stays constant across active/inactive: a
             selected-state weight bump widens the label, resizing the tab
             and shifting every sibling tab. -->
        <Hint label={item.root} side="bottom">
          {#snippet child({ props })}
            <button
              class="tab-main flex min-w-0 cursor-pointer select-none items-center gap-1 overflow-hidden text-ellipsis whitespace-nowrap rounded-t-lg border-0 bg-transparent py-1 pl-3.5 pr-1 text-sm font-medium text-muted-foreground outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
              class:text-accent-foreground={isActive}
              type="button"
              {...props}
              {...cell.triggerProps}
              aria-label={`${item.name} — ${item.root}${tabAriaSuffix(badge)}`}
              draggable="true"
              onclick={() => setActive(item.id)}
              onauxclick={(event) => onAuxClick(event, item.id)}
              oncontextmenu={(event) => tabMenu(event, item)}
              ondragstart={(event) => onDragStart(event, item.id)}
              ondragend={onDragEnd}
              onmousedown={(event) => {
                // Middle only: suppress autoscroll. The primary button keeps
                // its default so the native drag (draggable above) can start.
                if (event.button === 1) event.preventDefault();
              }}
            >
              {#if item.deviceId !== null}
                <!-- Remote repository: the cloud marks tabs living on a
                     device; dimmed while that device is not connected
                     (unknown ids report "disconnected"). -->
                <span
                  class="remote-mark inline-flex flex-none items-center justify-center text-muted-foreground {deviceConnected ? '' : 'opacity-50'}"
                  aria-hidden="true"
                >
                  <Icon icon={iconCloud} size={13} />
                </span>
              {/if}
              <span class="name block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{item.name}</span>
              {#if badge.changed > 0}
                <!-- Dirty mark: dot only (a count would resize the strip);
                     the number lives in the tooltip / accessible name. -->
                <span
                  class="dirty-mark ml-0.5 inline-block size-1.5 flex-none rounded-full bg-primary"
                  role="img"
                  aria-label={changedLabel(badge.changed)}
                  title={changedLabel(badge.changed)}
                ></span>
              {/if}
              {#if badge.ahead > 0 || badge.behind > 0}
                <!-- Ahead/behind chip: halves render only when non-zero. -->
                <span
                  class="ab-chip ml-0.5 inline-flex flex-none items-center gap-0.5 text-ui-micro leading-none text-muted-foreground tabular-nums"
                  role="img"
                  aria-label={aheadBehindLabel(badge.ahead, badge.behind)}
                  title={aheadBehindLabel(badge.ahead, badge.behind)}
                >
                  {#if badge.ahead > 0}<span class="ab-ahead">↑{badgeCount(badge.ahead)}</span>{/if}
                  {#if badge.behind > 0}<span class="ab-behind">↓{badgeCount(badge.behind)}</span>{/if}
                </span>
              {/if}
              {#if attentionById[item.id]?.attention === true}
                <!-- GitHub attention (E1): failing/degraded/warning/unpushed
                     chips on a background repo — a dot + count; the state
                     and its staleness ride in the tooltip. -->
                <span
                  class="forge-attention ml-0.5 inline-flex flex-none items-center rounded-full bg-destructive/15 px-1 text-[10px] leading-[1.5] font-medium text-destructive tabular-nums"
                  role="img"
                  aria-label={attentionTitle(item.id)}
                  title={attentionTitle(item.id)}
                >
                  ●{badgeCount(attentionById[item.id]!.count)}
                </span>
              {/if}
            </button>
          {/snippet}
        </Hint>
        <Hint label={`Close ${item.name}`} side="bottom">
          {#snippet child({ props })}
            <Button
              class="tab-close ml-0.5 h-auto flex-none rounded-tr-lg px-1.5 text-muted-foreground/70 hover:text-foreground focus-visible:bg-accent focus-visible:text-foreground"
              variant="ghost"
              size="icon-xs"
              type="button"
              {...props}
              onclick={() => void closeTab(item.id)}
            >
              <Icon icon={iconClose} size={13} />
            </Button>
          {/snippet}
        </Hint>
      </div>
    {/snippet}
  </UnifiedTabStrip>
  {/if}

  {#if trailing}
    <div class="tabbar-trailing m-0.5 flex flex-none items-center gap-1 border-l border-border px-2">{@render trailing()}</div>
  {/if}
</div>

