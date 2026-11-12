<script lang="ts">
  /**
   * Repository tab strip (M1). Bound to the global tab store — no props
   * except `openFolder` (the "+" button callback) and an optional `trailing`
   * snippet for window-level utilities (AI health, theme toggle).
   *
   * Accessibility: the strip is a `tablist` with a roving tabindex — the
   * active tab is tab-reachable (tabindex 0), the rest are programmatic
   * (tabindex -1). Arrow/Home/End keys move focus; Enter/Space activate the
   * focused tab (native button behavior routes through `setActive`).
 * Middle-click (auxclick button 1) closes a tab, as does its close button;
 * right-click opens the shared context menu (close variants + path
 * utilities) through the global context-menu store.
 *
 * Remote tabs (`deviceId` set) lead with a small cloud glyph — dimmed while
 * the device is offline (`remoteDevices.statusOf`, unknown ids report
 * "disconnected") — so a repository living on another machine is
 * identifiable at a glance; the tooltip's `ssh://` root says the same.
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
   * Overflow: the strip scrolls horizontally (vertical wheel is translated),
   * fades whichever edge has more content, and scrolls the active tab back
   * into view whenever activation or membership changes.
   *
   * Focus order is sane: each close button follows its tab in DOM order, so
   * Tab runs tab → its close → next reachable control. Overflowed (ellipsis-
   * truncated) names announce in full: each tab's accessible name is
   * "name — root" (label-in-name compliant) and the Hint tooltip shows the
   * root on hover.
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
  import { isTauri } from "$lib/entry/dragdrop";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { describeError } from "$lib/errors";
  import { toast } from "$lib/toast";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
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

  let strip: HTMLDivElement | undefined = $state();
  let overflowLeft = $state(false);
  let overflowRight = $state(false);

  /** Edge fades only where there is actually more strip to scroll to. */
  const stripMask = $derived(
    overflowLeft && overflowRight
      ? "linear-gradient(to right, transparent, black 1.25rem, black calc(100% - 1.25rem), transparent)"
      : overflowLeft
        ? "linear-gradient(to right, transparent, black 1.25rem)"
        : overflowRight
          ? "linear-gradient(to left, transparent, black 1.25rem)"
          : "none",
  );

  function syncOverflow(): void {
    if (!strip) return;
    const max = strip.scrollWidth - strip.clientWidth;
    // Sub-pixel scrollLeft/rounding would otherwise flicker the far fade.
    overflowLeft = strip.scrollLeft > 1;
    overflowRight = max > 1 && strip.scrollLeft < max - 1;
  }

  /**
   * Keeps the active tab visible after a switch, a close, or a window
   * resize. Scrolls the strip only — `scrollIntoView` would also drag
   * every scrollable ancestor.
   */
  function revealActive(): void {
    if (!strip) return;
    const active = strip.querySelector<HTMLButtonElement>("button[role='tab'][aria-selected='true']");
    if (!active) return;
    const bounds = active.getBoundingClientRect();
    const view = strip.getBoundingClientRect();
    const pad = 8;
    const left = bounds.left - view.left + strip.scrollLeft;
    const right = left + bounds.width;
    let target = strip.scrollLeft;
    if (left < target + pad) target = left - pad;
    else if (right > target + strip.clientWidth - pad) target = right - strip.clientWidth + pad;
    const max = Math.max(0, strip.scrollWidth - strip.clientWidth);
    target = Math.min(Math.max(target, 0), max);
    if (Math.abs(target - strip.scrollLeft) < 1) return;
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    strip.scrollTo({ left: target, behavior: reduce ? "auto" : "smooth" });
  }

  $effect(() => {
    // Re-run on membership changes as well as activation: closing the last
    // tab shifts everything left of it.
    void tabStore.tabs.length;
    void tabStore.activeId;
    const node = strip;
    if (!node) return;
    revealActive();
    syncOverflow();
  });

  $effect(() => {
    const node = strip;
    if (!node) return;
    // Vertical wheel over a horizontal strip: translate it, but leave
    // trackpad horizontal gestures to the browser. Non-passive so the
    // page behind the strip does not scroll along with it.
    const onWheel = (event: WheelEvent): void => {
      if (node.scrollWidth <= node.clientWidth) return;
      if (event.deltaY === 0 || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
      event.preventDefault();
      node.scrollLeft += event.deltaY;
    };
    node.addEventListener("wheel", onWheel, { passive: false });
    node.addEventListener("scroll", syncOverflow, { passive: true });
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(syncOverflow);
    observer?.observe(node);
    return () => {
      node.removeEventListener("wheel", onWheel);
      node.removeEventListener("scroll", syncOverflow);
      observer?.disconnect();
    };
  });

  function tabButtons(): HTMLButtonElement[] {
    if (!strip) return [];
    return Array.from(
      strip.querySelectorAll<HTMLButtonElement>("button[role='tab']"),
    );
  }

  function onKeydown(event: KeyboardEvent): void {
    const buttons = tabButtons();
    if (buttons.length === 0) return;
    const current = buttons.findIndex((button) => button === document.activeElement);

    let next: number;
    if (event.key === "ArrowRight") {
      next = current === -1 ? 0 : (current + 1) % buttons.length;
    } else if (event.key === "ArrowLeft") {
      next = current === -1 ? buttons.length - 1 : (current - 1 + buttons.length) % buttons.length;
    } else if (event.key === "Home") {
      next = 0;
    } else if (event.key === "End") {
      next = buttons.length - 1;
    } else {
      return; // Enter/Space flow through the native button click.
    }

    event.preventDefault();
    buttons[next]?.focus();
  }

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
        id: "copy-path",
        label: "Copy Repo Path",
        // Inline clipboard like BranchSwitcher (no shared helper exists).
        run: () =>
          void navigator.clipboard.writeText(tab.root).then(
            () => toast(`Copied ${tab.root}`),
            () => toast("Could not access the clipboard", { kind: "error" }),
          ),
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

<div class="tabbar flex min-w-0 flex-none items-stretch border-b border-border bg-card">
  <Hint label="Recent repositories" side="bottom">
    {#snippet child({ props })}
      <Button
        class="tab-home ml-2 size-7 flex-none self-center rounded-lg aria-[pressed=true]:bg-secondary aria-[pressed=true]:text-secondary-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring focus-visible:ring-0"
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
  <div
    class="tabstrip flex min-w-0 flex-1 items-stretch gap-0.5 overflow-x-auto px-2 pt-0.5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
    role="tablist"
    aria-label="Open repositories"
    tabindex="-1"
    bind:this={strip}
    style:mask-image={stripMask}
    onkeydown={onKeydown}
    oncontextmenu={(e) => e.preventDefault()}
  >
    {#each tabStore.tabs as tab, index (tab.id)}
      {@const isActive = tab.id === tabStore.activeId}
      {@const dropping = dropTargetId === tab.id && dragTabId !== null}
      {@const deviceConnected =
        tab.deviceId !== null && remoteDevices.statusOf(tab.deviceId) === "connected"}
      <!-- Dragover/drop live on the tab cell (label + close area both accept
           a drop); dragstart only from the label — the close button stays a
           plain click target. -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="tab relative flex min-w-0 max-w-64 items-stretch rounded-t-lg border-b-2 border-transparent transition-colors"
        class:active={isActive}
        class:bg-accent={isActive}
        class:border-b-primary={isActive}
        class:opacity-45={dragTabId === tab.id}
        ondragover={(event) => onTabDragOver(event, tab)}
        ondrop={(event) => onTabDrop(event, index)}
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
        <Hint label={tab.root} side="bottom">
          {#snippet child({ props })}
            <button
              class="tab-main flex min-w-0 cursor-pointer select-none items-center gap-1 overflow-hidden text-ellipsis whitespace-nowrap rounded-t-lg border-0 bg-transparent py-1 pl-3.5 pr-1 text-sm font-medium text-muted-foreground outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
              class:text-primary={isActive}
              type="button"
              role="tab"
              aria-selected={isActive}
              {...props}
              tabindex={isActive ? 0 : -1}
              aria-label={`${tab.name} — ${tab.root}`}
              draggable="true"
              onclick={() => setActive(tab.id)}
              onauxclick={(event) => onAuxClick(event, tab.id)}
              oncontextmenu={(event) => tabMenu(event, tab)}
              ondragstart={(event) => onDragStart(event, tab.id)}
              ondragend={onDragEnd}
              onmousedown={(event) => {
                // Middle only: suppress autoscroll. The primary button keeps
                // its default so the native drag (draggable above) can start.
                if (event.button === 1) event.preventDefault();
              }}
            >
              {#if tab.deviceId !== null}
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
              <span class="name block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{tab.name}</span>
            </button>
          {/snippet}
        </Hint>
        <Hint label={`Close ${tab.name}`} side="bottom">
          {#snippet child({ props })}
            <Button
              class="tab-close ml-0.5 h-auto flex-none rounded-tr-lg px-1.5 text-muted-foreground/70 hover:text-foreground focus-visible:bg-accent focus-visible:text-foreground"
              variant="ghost"
              size="icon-xs"
              type="button"
              {...props}
              onclick={() => void closeTab(tab.id)}
            >
              <Icon icon={iconClose} size={13} />
            </Button>
          {/snippet}
        </Hint>
      </div>
    {/each}

    <Hint label="Open a repository" side="bottom">
      {#snippet child({ props })}
        <Button
          class="tab-new my-0.5 h-auto flex-none px-2.5 text-muted-foreground hover:text-primary focus-visible:bg-accent focus-visible:text-primary"
          variant="ghost"
          size="icon-sm"
          type="button"
          {...props}
          onclick={() => openFolder?.()}
        >
          <Icon icon={iconAdd} size={16} />
        </Button>
      {/snippet}
    </Hint>
  </div>

  {#if trailing}
    <div class="tabbar-trailing m-0.5 flex flex-none items-center gap-1 border-l border-border px-2">{@render trailing()}</div>
  {/if}
</div>
