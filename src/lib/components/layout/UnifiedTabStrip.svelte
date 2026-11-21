<script lang="ts" module>
  /**
   * Shared strip types (module script: instance scripts cannot export).
   */
  /** Minimum shape the engine needs from a tab; surfaces pass richer objects. */
  export interface StripTabItem {
    /** Unique key: callbacks, keyed each, overflow identity. */
    id: string;
    /** Fallback label for the ⋯ overflow menu (ignored with `overflowItem`). */
    label?: string;
  }

  /** Per-tab context handed to the `tab` snippet. */
  export interface StripTabCell {
    active: boolean;
    index: number;
    /**
     * Spread onto the element carrying `role="tab"` — AFTER tooltip props,
     * BEFORE the surface's own handlers: role, roving `tabindex`,
     * `aria-selected`, `data-tab-index` (strip measurement queries it) and
     * the Enter/Space activation handler.
     */
    triggerProps: Record<string, unknown>;
  }
</script>

<script lang="ts" generics="T extends StripTabItem">
  /**
   * UnifiedTabStrip — the ONE canonical tab-strip engine (plan §4.3), born
   * from merging the three implementations the app had:
   *
   * - `TabStrip.svelte` (repo tabs, top-level row),
   * - `TabGroup.svelte` (workspace/panel tabs),
   * - the tab rendering inside `FileWorkspace.svelte` (open-file tabs).
   *
   * The strip owns the SHARED behavior and structure; each surface keeps
   * its exact look and its per-tab affordances through the `tab` snippet:
   *
   * - the `tablist` element + aria semantics (§11: tabs are a tablist),
   * - roving-tabindex keyboard navigation: the active tab is Tab-reachable
   *   (tabindex 0), the rest programmatic (-1); Arrow/Home/End move focus,
   *   Enter/Space activate (`tab` snippet elements spread `triggerProps`,
   *   which carries role/tabindex/aria-selected/data-tab-index and the
   *   Enter/Space activation handler — so plain `div` tabs, like
   *   FileWorkspace's, are keyboard-activatable too);
   *   `activateOnArrow` (TabGroup's idiom) makes ArrowLeft/Right also
   *   activate the tab they land on;
   * - horizontal scroll chrome: vertical wheel translated to horizontal
   *   scroll (trackpad gestures untouched), edge fades tracking where more
   *   strip hides (`edgeFades`), and the active tab scrolled back into view
   *   whenever activation or membership changes;
   * - the "⋯" overflow menu listing the tabs currently hidden by the strip
   *   width (measured per tab against the tablist rect); picking one
   *   activates it and the reveal effect scrolls it into view. Hidden for
   *   zero-width containers (jsdom, display:none) and when nothing hides.
   *
   * Per-surface behavior differences that predate the merge stay with the
   * surfaces: middle-click close, context menus, drag-to-reorder, dirty
   * marks and close buttons are all rendered/handled inside the `tab`
   * snippet (the strip never closes a tab on its own).
   *
   * Slots: `inStrip` renders trailing strip content INSIDE the scroll
   * container (TabGroup's "+ Add panel"); the ⋯ button renders between the
   * tablist and whatever the caller puts after this component.
   */
  import type { Snippet } from "svelte";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import OverflowMenu from "./OverflowMenu.svelte";
  import { cn } from "$lib/utils.js";

  let {
    tabs,
    activeId = null,
    label,
    tab,
    overflowItem = undefined,
    inStrip = undefined,
    onActivate = undefined,
    activateOnArrow = false,
    edgeFades = true,
    overflowLabel = "More tabs",
    class: className = "",
    stripClass = "",
    onStripContextMenu = undefined,
  }: {
    tabs: T[];
    activeId?: string | null;
    /** Accessible name for the tablist. */
    label: string;
    /** One tab cell per item (the surface's own markup + affordances). */
    tab: Snippet<[T, StripTabCell]>;
    /** Custom ⋯ menu entry (icon etc.); default renders `item.label`. */
    overflowItem?: Snippet<[T]>;
    /** Trailing content inside the scroll container (e.g. "+ Add panel"). */
    inStrip?: Snippet;
    /**
     * Keyboard Enter/Space/arrow activation (per `activateOnArrow`) and the
     * ⋯ menu selection. Pointer activation stays in the `tab` snippet.
     * Receives the typed tab item (ids can be narrower than string, e.g.
     * PanelId).
     */
    onActivate?: (item: T) => void;
    /** ArrowLeft/Right also activate (TabGroup idiom) — default focus-only. */
    activateOnArrow?: boolean;
    /** Edge fades where the strip hides content (repo-strip idiom). */
    edgeFades?: boolean;
    /** Accessible name for the ⋯ overflow trigger. */
    overflowLabel?: string;
    /** Root wrapper classes (add `flex-1` in a flex-row parent). */
    class?: string;
    /** Classes for the tablist scroll element (per-surface chrome). */
    stripClass?: string;
    /**
     * Right-click on the tablist BACKGROUND (between tabs). Omitted on the
     * repo/panel strips to keep their "no browser menu here" suppression;
     * surfaces with menus on tabs only (FileWorkspace) leave it unset.
     */
    onStripContextMenu?: (event: MouseEvent) => void;
  } = $props();

  const activeIndex = $derived(
    activeId === null ? -1 : tabs.findIndex((item) => item.id === activeId),
  );

  function triggerPropsFor(item: T, index: number): Record<string, unknown> {
    const active = item.id === activeId;
    return {
      role: "tab",
      "data-tab-index": index,
      tabindex: active ? 0 : -1,
      "aria-selected": active,
      onkeydown: (event: KeyboardEvent) => {
        // Enter/Space activate: native for the surfaces' <button> tabs, and
        // the only keyboard path for FileWorkspace's div tabs. preventDefault
        // keeps the native button click from double-firing activation.
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        onActivate?.(item);
      },
    };
  }

  // -- keyboard: roving tabindex + arrows/Home/End ---------------------------

  function tabElements(): HTMLElement[] {
    const node = stripEl;
    if (!node) return [];
    return [...node.querySelectorAll<HTMLElement>("[data-tab-index]")];
  }

  function onStripKeydown(event: KeyboardEvent): void {
    if (
      event.key !== "ArrowRight" &&
      event.key !== "ArrowLeft" &&
      event.key !== "Home" &&
      event.key !== "End"
    ) {
      return;
    }
    if (tabs.length === 0) return;
    const elements = tabElements();
    if (elements.length === 0) return;

    // Focus-only movement (repo/file strips) walks from the focused tab;
    // activate-on-arrow (TabGroup) walks from the ACTIVE tab.
    const current = elements.findIndex((element) => element === document.activeElement);
    const base =
      activateOnArrow && activeIndex !== -1 ? activeIndex : current;
    const last = elements.length - 1;

    let next: number;
    if (event.key === "ArrowRight") {
      next = base === -1 ? 0 : (base + 1) % elements.length;
    } else if (event.key === "ArrowLeft") {
      next = base === -1 ? last : (base - 1 + elements.length) % elements.length;
    } else if (event.key === "Home") {
      next = 0;
    } else {
      next = last;
    }

    event.preventDefault();
    elements[next]?.focus();
    if (
      activateOnArrow &&
      (event.key === "ArrowRight" || event.key === "ArrowLeft")
    ) {
      onActivate?.(tabs[next]!);
    }
  }

  // -- scroll chrome: wheel translation, edge fades, reveal ------------------

  let stripEl: HTMLDivElement | undefined = $state();
  let overflowLeft = $state(false);
  let overflowRight = $state(false);

  /** Edge fades only where there is actually more strip to scroll to. */
  const stripMask = $derived(
    !edgeFades
      ? "none"
      : overflowLeft && overflowRight
        ? "linear-gradient(to right, transparent, black 1.25rem, black calc(100% - 1.25rem), transparent)"
        : overflowLeft
          ? "linear-gradient(to right, transparent, black 1.25rem)"
          : overflowRight
            ? "linear-gradient(to left, transparent, black 1.25rem)"
            : "none",
  );

  function syncOverflow(): void {
    const node = stripEl;
    if (!node) return;
    const max = node.scrollWidth - node.clientWidth;
    // Sub-pixel scrollLeft/rounding would otherwise flicker the far fade.
    overflowLeft = node.scrollLeft > 1;
    overflowRight = max > 1 && node.scrollLeft < max - 1;
    measureHidden();
  }

  /**
   * Tabs currently hidden by the strip width, in tab order — the ⋯ menu's
   * contents. Measured against the tablist rect (the fade mask does not
   * clip rects). Skipped when the strip has no layout (clientWidth 0 in
   * jsdom / display:none) so tests and unrendered strips show no ⋯.
   */
  let hiddenIds = $state<string[]>([]);

  function measureHidden(): void {
    const node = stripEl;
    if (!node || node.clientWidth <= 0) {
      hiddenIds = [];
      return;
    }
    const bounds = node.getBoundingClientRect();
    if (bounds.width <= 0) {
      hiddenIds = [];
      return;
    }
    const hidden: string[] = [];
    const elements = tabElements();
    for (let index = 0; index < elements.length; index++) {
      const rect = elements[index]!.getBoundingClientRect();
      if (rect.width <= 0) continue; // not laid out — not "hidden"
      if (rect.left < bounds.left - 0.5 || rect.right > bounds.right + 0.5) {
        hidden.push(tabs[index]!.id);
      }
    }
    hiddenIds = hidden;
  }

  /** Shared reveal math: scrolls the strip so `bounds` is fully in view. */
  function revealBounds(bounds: DOMRect): void {
    const node = stripEl;
    if (!node) return;
    const view = node.getBoundingClientRect();
    const pad = 8;
    const left = bounds.left - view.left + node.scrollLeft;
    const right = left + bounds.width;
    let target = node.scrollLeft;
    if (left < target + pad) target = left - pad;
    else if (right > target + node.clientWidth - pad) target = right - node.clientWidth + pad;
    const max = Math.max(0, node.scrollWidth - node.clientWidth);
    target = Math.min(Math.max(target, 0), max);
    if (Math.abs(target - node.scrollLeft) < 1) return;
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    node.scrollTo({ left: target, behavior: reduce ? "auto" : "smooth" });
  }

  /**
   * Keeps the active tab visible after a switch, a close, or a window
   * resize. Scrolls the strip only — `scrollIntoView` would also drag
   * every scrollable ancestor.
   */
  function revealActive(): void {
    const node = stripEl;
    if (!node) return;
    const active = node.querySelector<HTMLElement>("[role='tab'][aria-selected='true']");
    if (!active) return;
    revealBounds(active.getBoundingClientRect());
  }

  $effect(() => {
    // Re-run on membership changes as well as activation: closing the last
    // tab shifts everything left of it.
    void tabs.length;
    void activeId;
    const node = stripEl;
    if (!node) return;
    revealActive();
    syncOverflow();
  });

  $effect(() => {
    const node = stripEl;
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
</script>

<div class={cn("unified-tabstrip flex min-w-0 items-stretch", className)}>
  <div
    class={cn(
      "flex min-w-0 flex-1 items-stretch overflow-x-auto",
      stripClass,
    )}
    role="tablist"
    aria-label={label}
    tabindex="-1"
    bind:this={stripEl}
    style:mask-image={stripMask}
    onkeydown={onStripKeydown}
    oncontextmenu={onStripContextMenu}
  >
    {#each tabs as item, index (item.id)}
      {@render tab(item, {
        active: item.id === activeId,
        index,
        triggerProps: triggerPropsFor(item, index),
      })}
    {/each}
    {#if inStrip}{@render inStrip()}{/if}
  </div>

  {#if hiddenIds.length > 0}
    <OverflowMenu label={overflowLabel} class="self-center mx-0.5" align="end">
      {#snippet content()}
        {#each hiddenIds as id (id)}
          {@const item = tabs.find((entry) => entry.id === id)}
          {#if item}
            <DropdownMenu.Item
              class="cursor-pointer"
              onclick={() => onActivate?.(item)}
            >
              {#if overflowItem}{@render overflowItem(item)}{:else}{item.label ?? item.id}{/if}
            </DropdownMenu.Item>
          {/if}
        {/each}
      {/snippet}
    </OverflowMenu>
  {/if}
</div>
