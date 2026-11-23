<script lang="ts">
  /**
   * The Overview — the standalone full-window multi-repo dashboard that
   * replaced both Agent mode and the old recents home: the zero-tabs landing
   * IS the Overview, and the header toggle (or the old recents button /
   * command) shows it over open tabs; Esc or the toggle returns to the
   * workspace.
   *
   * Layout: summary strip (totals + filter presets + actions) on top, one
   * slim card-row per library repo, and the chat rail on the right (auto-
   * hidden below 1280px inner width, collapsible). Rows for open tabs read
   * live tab data; everything else scans via `repo_summary` ONLY while the
   * Overview is visible (become-visible / window focus / manual Refresh).
   * Expanding a non-open row warms a real session for the Overview's
   * lifetime; leaving the Overview closes warmed sessions again.
   *
   * `children` renders after the repo list (App.svelte's remote-devices
   * section keeps living there, like the old home's).
   */
  import { onMount } from "svelte";
  import type { Snippet } from "svelte";
  import { Kbd, KbdGroup } from "$lib/components/ui/kbd/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import EmptyState from "$lib/components/layout/EmptyState.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { primaryModifierLabel } from "$lib/palette/keybinds";
  import { pickFolder } from "$lib/ipc/client";
  import { overlayStack } from "$lib/state/overlays.svelte";
  import { hideRecentRepos, openTab, tabStore } from "$lib/stores/tabs.svelte";
  import { forgeStore } from "$lib/stores/forge.svelte";
  import { toast } from "$lib/toast";
  import ThemeToggle from "$lib/components/ThemeToggle.svelte";
  import UpdateChip from "$lib/components/UpdateChip.svelte";
  import AiHealthChip from "$lib/components/ai/AiHealthChip.svelte";
  import { statusBanner } from "$lib/components/forge/forgeModel";
  import { overviewData } from "./overviewData.svelte";
  import { overviewStore } from "./overviewStore.svelte";
  import { defaultExpanded, refreshedAgo, repoName, type OverviewRowData } from "./overviewModel";
  import OverviewStrip from "./OverviewStrip.svelte";
  import OverviewSortHeader from "./OverviewSortHeader.svelte";
  import OverviewRow from "./OverviewRow.svelte";
  import OverviewChatRail from "./OverviewChatRail.svelte";
  import OverviewToggle from "./OverviewToggle.svelte";

  let {
    onOpenSettings,
    onClone,
    children,
  }: {
    onOpenSettings?: () => void;
    /** Opens the clone dialog (App-owned); omitted hides the action. */
    onClone?: () => void;
    children?: Snippet;
  } = $props();

  const iconBrand = $derived(appIcon("account-tree")!);
  const iconSettings = $derived(appIcon("tune")!);
  const iconClone = $derived(appIcon("download")!);
  const iconBack = $derived(appIcon("arrow-back-rounded")!);

  // The 1280px chat-rail breakpoint (Tailwind's xl, same as the old Agent
  // view's right column).
  let innerWidth = $state(1280);
  const railWidthOk = $derived(innerWidth >= 1280);
  const showRail = $derived(railWidthOk && overviewStore.chatRailOpen);

  let now = $state(Date.now());
  // Lightweight stamp ticker — only runs while this view is mounted.
  $effect(() => {
    const timer = setInterval(() => (now = Date.now()), 5000);
    return () => clearInterval(timer);
  });

  const tabsOpen = $derived(tabStore.tabs.length > 0);
  const rows = $derived(overviewData.rows);
  const visible = $derived(
    overviewData.visibleRows(overviewStore.filter, overviewStore.sort, overviewStore.sortDir),
  );
  const totals = $derived(overviewData.totals());
  const banner = $derived(statusBanner(forgeStore.statusFor()));
  const refreshedLabel = $derived(
    refreshedAgo(overviewData.lastRefreshAt, now),
  );

  // Expansion: user overrides win; otherwise conflicts / sync-debt rows
  // render expanded by default.
  let overrides = $state<Record<string, boolean>>({});

  function isExpanded(row: OverviewRowData): boolean {
    return overrides[row.root] ?? defaultExpanded(row);
  }

  /** The chat target: the (first) expanded row in display order — sorted
   *  + filtered, so the rail never targets a row the user can't see. */
  const chatTarget = $derived(visible.find((row) => isExpanded(row)) ?? null);

  function onToggleRow(root: string): void {
    const row = overviewData.rowFor(root);
    if (!row) return;
    const next = !isExpanded(row);
    overrides = { ...overrides, [root]: next };
    if (next) {
      if (!row.open && !overviewData.warmer.isWarmed(root)) {
        // Expanding a non-open repo warms a real session (watcher + gh lane)
        // for the Overview's lifetime.
        void overviewData.warmRow(root);
      } else if (row.repoId) {
        void overviewData.refreshForgeRow(root, row.repoId);
      }
    }
  }

  function exitOverview(): void {
    overviewStore.disable();
    hideRecentRepos();
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape") return;
    // The workspace must exist to return to it (the zero-tabs landing has
    // no tabs, so Esc there is inert — like the old home).
    if (!tabsOpen) return;
    // An open overlay (palette, menus, dialogs) owns Escape first.
    if (overlayStack.top !== null) return;
    event.preventDefault();
    exitOverview();
  }

  async function openRepo(root: string): Promise<void> {
    exitOverview();
    try {
      await openTab(root);
    } catch (err) {
      toast(`Failed to open ${repoName(root)}`, { kind: "error" });
      console.warn("[overview] open failed:", err);
    }
  }

  function removeRow(root: string): void {
    overviewData.removeFromLibrary(root);
    toast(`Removed ${repoName(root)} from the library`);
  }

  async function addFolder(): Promise<void> {
    const path = await pickFolder();
    if (!path) return;
    const ok = await overviewData.addFolder(path);
    if (ok) {
      toast(`Added ${repoName(path)} to the library`, { kind: "success" });
    } else {
      toast(`${repoName(path)} is not a git repository — not added`, {
        kind: "error",
      });
    }
  }

  onMount(() => {
    const cleanups: Array<() => void> = [];

    // Become-visible refresh + focus refresh (the scan tiers), and the
    // warmed-session teardown on the way out.
    void overviewData.activate();
    cleanups.push(() => {
      void overviewData.deactivate();
    });

    const onFocus = (): void => {
      void overviewData.refresh();
    };
    const onVisibility = (): void => {
      if (document.visibilityState === "visible") void overviewData.refresh();
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);
    cleanups.push(() => {
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
    });

    // The strip's gh banner: one probe per Overview session (the same
    // forgeStore cache the workspace chips use).
    if (forgeStore.statusFor() === null) void forgeStore.refreshStatus();

    return () => {
      for (const unlisten of cleanups) unlisten();
    };
  });
</script>

<svelte:window bind:innerWidth onkeydown={onKeydown} />

<div class="overview flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background text-foreground">
  {#if !tabsOpen}
    <!-- Zero-tabs landing header: the old home's utilities (the tabs-open
         variant keeps them in the title bar's trailing cluster). -->
    <header
      class="overview-brand flex flex-none items-center gap-2.5 border-b border-border bg-card px-4 py-2"
    >
      <span class="brand-mark grid size-7 flex-none place-items-center rounded-md bg-primary/10 text-primary-ink" aria-hidden="true">
        <Icon icon={iconBrand} size={16} />
      </span>
      <span class="min-w-0 flex-1 truncate text-sm font-semibold tracking-[-0.01em]">mygitui</span>
      <span class="flex flex-none items-center gap-1.5">
        <UpdateChip compact />
        <AiHealthChip compact />
        <OverviewToggle />
        {#if onOpenSettings}
          <Hint label="Settings (Ctrl+,)">
            {#snippet child({ props })}
              <Button
                {...props}
                variant="ghost"
                size="icon-sm"
                type="button"
                aria-label="Open settings"
                onclick={onOpenSettings}
              >
                <Icon icon={iconSettings} size={15} />
              </Button>
            {/snippet}
          </Hint>
        {/if}
        <ThemeToggle />
      </span>
    </header>
  {/if}

  <OverviewStrip
    {totals}
    filter={overviewStore.filter}
    {banner}
    refreshing={overviewData.scanning}
    {refreshedLabel}
    showChatToggle={railWidthOk}
    chatRailOpen={overviewStore.chatRailOpen}
    onFilter={(filter) => overviewStore.setFilter(filter)}
    onRefresh={() => void overviewData.refresh()}
    onAddFolder={() => void addFolder()}
    onToggleChatRail={() => overviewStore.setChatRailOpen(!overviewStore.chatRailOpen)}
  />

  <div class="flex min-h-0 min-w-0 flex-1">
    <div class="overview-scroll min-h-0 min-w-0 flex-1 overflow-y-auto">
      {#if tabsOpen}
        <div class="flex flex-none items-center px-4 pt-2">
          <button
            class="back-btn inline-flex cursor-pointer items-center gap-1.5 rounded-md border border-border/60 bg-muted/30 px-2.5 py-1 text-ui-meta font-medium text-muted-foreground hover:bg-[color:var(--app-hover)] hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary transition-colors"
            type="button"
            onclick={exitOverview}
          >
            <Icon icon={iconBack} size={13} />
            Back to {tabStore.active?.name ?? "workspace"}
          </button>
        </div>
      {/if}

      {#if visible.length === 0 && rows.length === 0}
        <div class="flex h-full items-center justify-center p-6">
          <EmptyState
            icon={appIcon("folder-code")!}
            title="No repositories in the library"
            hint="Add a folder on disk, clone a remote, or drag a folder onto this window — open tabs and favorites join the library automatically."
          >
            <Button variant="secondary" size="sm" type="button" onclick={() => void addFolder()}>
              Add folder…
            </Button>
            {#if onClone}
              <Button variant="ghost" size="sm" type="button" onclick={onClone}>
                Clone repository…
              </Button>
            {/if}
          </EmptyState>
        </div>
      {:else if visible.length === 0}
        <div class="flex h-full items-center justify-center p-6">
          <EmptyState
            icon={appIcon("filter-list-off")!}
            title="No repositories match this filter"
            hint="Switch the filter preset off to see the whole library again."
          />
        </div>
      {:else}
        <OverviewSortHeader
          sort={overviewStore.sort}
          sortDir={overviewStore.sortDir}
          onSort={(id) => overviewStore.setSort(id)}
        />
        <ul
          class="overview-list m-0 flex list-none flex-col gap-2 p-0 px-4 py-3"
          aria-label="Repositories"
        >
          {#each visible as row (row.root)}
            <li class="min-w-0">
              <OverviewRow
                {row}
                expanded={isExpanded(row)}
                {now}
                onToggle={onToggleRow}
                onOpen={(root) => void openRepo(root)}
                onRemove={removeRow}
              />
            </li>
          {/each}
        </ul>
      {/if}

      {#if children}
        <div class="overview-children border-t border-border px-5 py-4">
          {@render children()}
        </div>
      {/if}
    </div>

    {#if showRail}
      <OverviewChatRail
        repo={chatTarget}
        onCollapse={() => overviewStore.setChatRailOpen(false)}
      />
    {/if}
  </div>

  {#if !tabsOpen}
    <footer
      class="overview-foot flex flex-none items-center gap-5 border-t border-border bg-muted/80 px-5 py-[0.65rem] text-ui-micro text-muted-foreground"
    >
      <span class="hint inline-flex items-center gap-1.5">
        <KbdGroup><Kbd>{primaryModifierLabel()}</Kbd><Kbd>K</Kbd></KbdGroup>
        <span class="ml-0.5">command palette</span>
      </span>
      <span class="hint inline-flex items-center gap-1.5">
        <KbdGroup><Kbd>{primaryModifierLabel()}</Kbd><Kbd>Shift</Kbd><Kbd>K</Kbd></KbdGroup>
        <span class="ml-0.5">quick switcher</span>
      </span>
      <span class="foot-note ml-auto opacity-85">Tip: drop a folder here to add it to the library</span>
    </footer>
  {/if}
</div>
