<script lang="ts">
  /**
   * OverlayDrawer — a NON-modal drawer anchored to a window edge (plan
   * §4.3; §10 uses it for FILES/HISTORY in Medium/Narrow). The vendored
   * sheet/drawer primitives are modal (bits-ui Dialog / vaul) and would
   * block the rest of the UI; this drawer does not — it overlays the panel
   * content while the workspace around it stays live.
   *
   * Behavior contract (§10/§11):
   * - `role="dialog"` + `aria-label`, `aria-modal="false"` (non-modal);
   * - focus moves into the drawer on open and returns to the invoking
   *   element on close (captured at open time);
   * - Tab wraps within the drawer while focus is inside it; clicking
   *   outside the drawer moves focus freely (non-modal);
   * - Esc closes (scoped to the drawer's focus, so nested floating layers
   *   and outer surfaces keep their own Esc semantics);
   * - `prefers-reduced-motion` collapses the slide (120ms in / 100ms out)
   *   to effectively instant (the global rule in `app.css`);
   * - the content stays MOUNTED while closed (`visibility: hidden` +
   *   off-screen transform) so panel state survives open/close toggles.
   *
   * Persistence of `open` per width mode is the caller's job (splitPrefs
   * `readSplitModeOpen`/`writeSplitModeOpen`) — this component only owns
   * the `open`/`onOpenChange` surface.
   */
  import type { Snippet } from "svelte";
  import { cn } from "$lib/utils.js";

  let {
    open = $bindable(false),
    onOpenChange = undefined,
    side = "left",
    label,
    class: className = "",
    children,
  }: {
    /** Open state (bindable; persistence is the caller's per-width-mode job). */
    open?: boolean;
    onOpenChange?: (next: boolean) => void;
    /** Which window edge the drawer anchors to (the rail side is "left"). */
    side?: "left" | "right";
    /** Accessible name of the drawer (dialog). */
    label: string;
    /** Extra classes for the drawer panel (width — default `w-80`). */
    class?: string;
    /** Drawer body (stays mounted while closed). */
    children: Snippet;
  } = $props();

  let panelEl = $state<HTMLElement | undefined>(undefined);

  /** The element focus returns to on close (the invoking toggle). */
  let returnFocusTo: HTMLElement | null = null;

  const FOCUSABLE =
    "a[href], button:not([disabled]), textarea, input, select, iframe, [tabindex]:not([tabindex='-1'])";

  function close(): void {
    open = false;
    onOpenChange?.(false);
  }

  function focusables(): HTMLElement[] {
    const node = panelEl;
    if (!node) return [];
    return [...node.querySelectorAll<HTMLElement>(FOCUSABLE)];
  }

  /** Tab wraps inside the drawer; focus already outside is left alone. */
  function trapTab(event: KeyboardEvent): void {
    const node = panelEl;
    if (!node) return;
    const items = focusables();
    if (items.length === 0) {
      // Nothing focusable: keep Tab from leaving the drawer entirely.
      event.preventDefault();
      return;
    }
    const active = document.activeElement;
    if (!(active instanceof Node) || !node.contains(active)) return;
    const first = items[0]!;
    const last = items[items.length - 1]!;
    if (event.shiftKey) {
      if (active === first) {
        event.preventDefault();
        last.focus();
      }
    } else if (active === last) {
      event.preventDefault();
      first.focus();
    }
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      // A nested floating layer (menu/popover) that already handled Esc
      // marks the event; don't close the drawer out from under it.
      if (event.defaultPrevented) return;
      event.stopPropagation();
      close();
    } else if (event.key === "Tab") {
      trapTab(event);
    }
  }

  // A press on the drawer's non-focusable surface would otherwise drop
  // focus to <body> (and strand the Esc close); anchor it back to the panel.
  function onPointerdown(event: PointerEvent): void {
    const target = event.target;
    if (target instanceof Element && target.closest(FOCUSABLE)) return;
    panelEl?.focus();
  }

  $effect(() => {
    if (open) {
      // Capture the invoker BEFORE focus moves into the drawer. Pointer
      // clicks don't focus buttons in every engine (Safari/WebKitGTK), so
      // keyboard users get exact focus return while pointer users keep
      // whatever focus they had — the standard activeElement capture.
      const active = document.activeElement;
      returnFocusTo =
        active instanceof HTMLElement && active !== document.body ? active : null;
      panelEl?.focus();
    } else if (returnFocusTo) {
      // Close (Esc or the caller flipping `open`): hand focus back.
      returnFocusTo.focus();
      returnFocusTo = null;
    }
  });
</script>

<div
  bind:this={panelEl}
  data-slot="overlay-drawer"
  data-side={side}
  data-state={open ? "open" : "closed"}
  role="dialog"
  aria-modal="false"
  aria-label={label}
  tabindex="-1"
  class={cn(
    "fixed inset-y-0 z-40 flex w-80 flex-col overflow-hidden bg-card shadow-overlay outline-none transition-[transform,visibility] ease-out",
    // Motion grammar (app.css): enter ≤150ms, exit ≤100ms.
    open ? "duration-[120ms]" : "duration-[100ms]",
    side === "left" ? "left-0 border-r border-border" : "right-0 border-l border-border",
    open
      ? "visible translate-x-0"
      : side === "left"
        ? "invisible -translate-x-full"
        : "invisible translate-x-full",
    className,
  )}
  onkeydown={onKeydown}
  onpointerdown={onPointerdown}
>
  {@render children()}
</div>
