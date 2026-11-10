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
   * Overflow: the strip scrolls horizontally (vertical wheel is translated),
   * fades whichever edge has more content, and scrolls the active tab back
   * into view whenever activation or membership changes.
   *
   * Focus order is sane: each close button follows its tab in DOM order, so
   * Tab runs tab → its close → next reachable control. Overflowed (ellipsis-
   * truncated) names announce in full: each tab's accessible name is
   * "name — root" (label-in-name compliant) and the title tooltip shows the
   * root on hover.
   */
  import type { Snippet } from "svelte";
  import {
    closeTab,
    setActive,
    showRecentRepos,
    SSH_SCHEME,
    tabStore,
    type Tab,
  } from "$lib/stores/tabs.svelte";
  import { showMenuAt, type MenuEntry } from "$lib/components/menu/contextMenuStore.svelte";
  import { isTauri } from "$lib/entry/dragdrop";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { describeError } from "$lib/errors";
  import { toast } from "$lib/toast";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
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
  <Button
    class="tab-home ml-2 size-7 flex-none self-center rounded-lg aria-[pressed=true]:bg-secondary aria-[pressed=true]:text-secondary-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring focus-visible:ring-0"
    variant="ghost"
    size="icon-sm"
    type="button"
    aria-label="Recent repositories"
    aria-pressed={tabStore.recentsOpen}
    title="Recent repositories"
    onclick={showRecentRepos}
  >
    <Icon icon={iconHome} size={16} />
  </Button>
  <div
    class="tabstrip flex min-w-0 flex-1 items-stretch gap-0.5 overflow-x-auto px-2 pt-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
    role="tablist"
    aria-label="Open repositories"
    tabindex="-1"
    bind:this={strip}
    style:mask-image={stripMask}
    onkeydown={onKeydown}
    oncontextmenu={(e) => e.preventDefault()}
  >
    {#each tabStore.tabs as tab (tab.id)}
      {@const isActive = tab.id === tabStore.activeId}
      <div
        class="tab flex min-w-0 max-w-64 items-stretch rounded-t-lg border-b-2 border-transparent transition-colors"
        class:active={isActive}
        class:bg-accent={isActive}
        class:border-b-primary={isActive}
      >
        <!-- font-weight stays constant across active/inactive: a
             selected-state weight bump widens the label, resizing the tab
             and shifting every sibling tab. -->
        <button
          class="tab-main min-w-0 cursor-pointer select-none overflow-hidden text-ellipsis whitespace-nowrap rounded-t-lg border-0 bg-transparent py-2 pl-3.5 pr-1 text-sm font-medium text-muted-foreground outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
          class:text-primary={isActive}
          type="button"
          role="tab"
          aria-selected={isActive}
          tabindex={isActive ? 0 : -1}
          title={tab.root}
          aria-label={`${tab.name} — ${tab.root}`}
          onclick={() => setActive(tab.id)}
          onauxclick={(event) => onAuxClick(event, tab.id)}
          oncontextmenu={(event) => tabMenu(event, tab)}
          onmousedown={(event) => {
            if (event.button === 1) event.preventDefault();
          }}
        >
          <span class="name block overflow-hidden text-ellipsis whitespace-nowrap">{tab.name}</span>
        </button>
        <Button
          class="tab-close ml-0.5 h-auto flex-none rounded-tr-lg px-1.5 text-muted-foreground/70 hover:text-foreground focus-visible:bg-accent focus-visible:text-foreground"
          variant="ghost"
          size="icon-xs"
          type="button"
          aria-label="Close {tab.name}"
          title="Close {tab.name}"
          onclick={() => void closeTab(tab.id)}
        >
          <Icon icon={iconClose} size={13} />
        </Button>
      </div>
    {/each}

    <Button
      class="tab-new my-0.5 h-auto flex-none px-2.5 text-muted-foreground hover:text-primary focus-visible:bg-accent focus-visible:text-primary"
      variant="ghost"
      size="icon-sm"
      type="button"
      aria-label="Open a repository"
      title="Open a repository"
      onclick={() => openFolder?.()}
    >
      <Icon icon={iconAdd} size={16} />
    </Button>
  </div>

  {#if trailing}
    <div class="tabbar-trailing m-1 flex flex-none items-center gap-1 border-l border-border px-2">{@render trailing()}</div>
  {/if}
</div>
