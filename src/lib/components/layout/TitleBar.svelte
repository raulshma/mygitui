<script lang="ts">
  /**
   * Custom application title bar (window chrome). Replaces the OS title
   * bar: the windows are undecorated on Windows/Linux (`set_decorations`
   * in `src-tauri/src/lib.rs`, `decorations(false)` in popout.rs) and use
   * the macOS overlay style (`titleBarStyle: "Overlay"` + `hiddenTitle` in
   * tauri.conf.json / the popout builder) so the native traffic lights
   * float over this bar.
   *
   * Since Workstream A (§5.2) this is the ONE top chrome row: the
   * repository tab strip renders inside it (the `tabs` snippet, App.svelte)
   * so title bar + repo tabs cost a single ~40px row instead of two.
   *
   * Native window functionality is preserved through the Tauri window API:
   * - Dragging: the bar is `data-tauri-drag-region="deep"` — every click in
   *   the subtree starts the OS move loop (edge snapping / GNOME tiling
   *   work as with a native bar); interactive elements (buttons, the tab
   *   strip's labels) block it.
   * - Double-click maximize: Tauri's injected drag script toggles
   *   maximization on drag-region double-clicks itself (this component
   *   must NOT add its own dblclick handler — it would toggle twice).
   * - Controls: minimize / maximize-restore / close buttons at the right
   *   edge; a right-click opens the native-style window menu (Minimize /
   *   Maximize-Restore / Close) through the shared context-menu store.
   * - Resizing: undecorated Linux/GTK windows lose the native resize
   *   borders, so eight invisible hit zones around the viewport call
   *   `startResizeDragging` (Windows' tao hit-test keeps working; the
   *   zones complement it). Hidden while maximized/fullscreen.
   *
   * Maximized/fullscreen state is tracked via `onResized` so the maximize
   * button swaps to its restore glyph and fullscreen hides the controls
   * and resize zones (native bars do the same). Popout windows get the
   * same bar and read the title `open_popout` set on the native window.
   *
   * Guarded with the shared `isTauri()` check (see `$lib/entry/dragdrop`):
   * outside a Tauri webview (plain browser / e2e / vitest) the bar renders
   * as plain chrome — no controls, no zones — so the layout stays stable.
   * macOS detection is user-agent based (best-effort; there is no os
   * plugin in the app) — it only decides whether the custom controls show.
   */

  import type { Snippet } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import type { Window as TauriWindow } from "@tauri-apps/api/window";
  import { isTauri } from "$lib/entry/dragdrop";
  import { parsePopoutQuery } from "$lib/layout/popout";
  import { windowTitle } from "$lib/stores/windowTitle.svelte";
  import {
    showMenuAt,
    type MenuEntry,
  } from "$lib/components/menu/contextMenuStore.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import iconBrand from "@ktibow/iconset-material-symbols/account-tree";
  import iconClose from "@ktibow/iconset-material-symbols/close";
  import iconMinimize from "@ktibow/iconset-material-symbols/remove";
  import iconMaximize from "@ktibow/iconset-material-symbols/crop-square";
  import iconRestore from "@ktibow/iconset-material-symbols/filter-none";

  /** Fallback when the native title cannot be read (popouts). */
  const APP_NAME = "mygitui";

  const native = isTauri();

  /** macOS keeps the native traffic lights — no custom controls, and the
   *  bar pads its left edge so the brand does not sit under them. */
  const isMac =
    native &&
    typeof navigator !== "undefined" &&
    /mac/i.test(navigator.userAgent);

  /** Popout contract (`?panel=…`): render the native title instead of the
   *  tab store's one (popout webviews have no tabs). */
  const popout = parsePopoutQuery(
    typeof location !== "undefined" ? location.search : "",
  );

  /** Maximized/fullscreen tracking for the glyph swap + zone visibility. */
  let maximized = $state(false);
  let fullscreen = $state(false);

  /**
   * Workstream A (§5.2): the repository tab strip renders INSIDE this bar —
   * one 40px chrome row instead of a title bar + tab strip stack. The
   * snippet carries the whole strip (home button, repo tabs, trailing
   * status cluster + overflow); with it absent (popouts, home, launch) the
   * bar shows the plain brand + window title and stays a pure title bar.
   * Drag semantics are untouched: the `deep` drag region still covers the
   * subtree, and interactive elements inside the snippet block dragging.
   */
  let { tabs = undefined }: { tabs?: Snippet } = $props();

  /** Native title of a popout window, fetched once on mount. */
  let popoutTitle = $state<string | null>(null);

  const displayTitle = $derived(
    popout !== null ? (popoutTitle ?? APP_NAME) : windowTitle.current,
  );

  $effect(() => {
    if (!native || popout === null) return;
    let cancelled = false;
    // `title` is a core:window:default getter — no extra permission; on
    // failure the app-name fallback stays.
    getCurrentWindow()
      .title()
      .then((title) => {
        if (!cancelled && title !== "") popoutTitle = title;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  });

  $effect(() => {
    if (!native) return;
    const win = getCurrentWindow();
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    // Resizes fire per frame during a drag; the getters are cheap and the
    // state writes are no-ops while the values stay the same.
    const refresh = (): void => {
      void win
        .isMaximized()
        .then((value) => {
          if (!cancelled) maximized = value;
        })
        .catch(() => {});
      void win
        .isFullscreen()
        .then((value) => {
          if (!cancelled) fullscreen = value;
        })
        .catch(() => {});
    };
    refresh();
    void win
      .onResized(refresh)
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  });

  /** Runs a window action, degrading permission failures to a warning
   *  (same contract as the title store — never crash the bar). */
  function runWindowAction(action: (win: TauriWindow) => Promise<void>): void {
    if (!native) return;
    try {
      action(getCurrentWindow()).catch((err: unknown) => {
        console.warn("[mygitui] window control failed:", err);
      });
    } catch (err) {
      console.warn("[mygitui] window control failed:", err);
    }
  }

  const minimize = (): void => runWindowAction((win) => win.minimize());
  const toggleMaximize = (): void =>
    runWindowAction((win) => win.toggleMaximize());
  const close = (): void => runWindowAction((win) => win.close());

  /** Native-style window menu on right-click (Restore/Minimize/Maximize/
   *  Close, like the OS bar's system menu). */
  function windowMenu(event: MouseEvent): void {
    if (!native) return;
    const entries: MenuEntry[] = [
      { id: "win-minimize", label: "Minimize", run: minimize },
      maximized
        ? { id: "win-restore", label: "Restore", run: toggleMaximize }
        : { id: "win-maximize", label: "Maximize", run: toggleMaximize },
      { separator: true },
      { id: "win-close", label: "Close", run: close },
    ];
    showMenuAt(event, entries);
  }

  // -- edge & corner resize zones (undecorated Linux windows have no
  //    native resize borders; see the module comment) ----------------------

  type ResizeDirection =
    | "East"
    | "North"
    | "NorthEast"
    | "NorthWest"
    | "South"
    | "SouthEast"
    | "SouthWest"
    | "West";

  interface ResizeZone {
    direction: ResizeDirection;
    class: string;
  }

  /** 5px edges between 14px corners — corners get the bigger grab area. */
  const RESIZE_ZONES: ResizeZone[] = [
    { direction: "North", class: "inset-x-3.5 top-0 h-[5px] cursor-n-resize" },
    { direction: "South", class: "inset-x-3.5 bottom-0 h-[5px] cursor-s-resize" },
    { direction: "West", class: "inset-y-3.5 left-0 w-[5px] cursor-w-resize" },
    { direction: "East", class: "inset-y-3.5 right-0 w-[5px] cursor-e-resize" },
    { direction: "NorthWest", class: "top-0 left-0 size-3.5 cursor-nw-resize" },
    { direction: "NorthEast", class: "top-0 right-0 size-3.5 cursor-ne-resize" },
    { direction: "SouthWest", class: "bottom-0 left-0 size-3.5 cursor-sw-resize" },
    { direction: "SouthEast", class: "bottom-0 right-0 size-3.5 cursor-se-resize" },
  ];

  /** Shared control-button chrome: flat titlebar-shaped rectangles. */
  const controlButtonClass =
    "flex w-11 items-center justify-center border-0 bg-transparent text-muted-foreground outline-none hover:bg-[color:var(--app-hover)] hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring";

  function beginResize(event: MouseEvent, direction: ResizeDirection): void {
    event.preventDefault(); // text selection / drag start behind the zone
    runWindowAction((win) => win.startResizeDragging(direction));
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<!-- Right-click opens the native-style window menu (see `windowMenu`). -->
<header
  class="titlebar {isMac ? 'pl-[78px]' : ''} flex h-10 flex-none select-none items-stretch border-b border-border bg-card"
  data-tauri-drag-region={native ? "deep" : undefined}
  oncontextmenu={windowMenu}
>
  <!-- With repo tabs in the row the title text would duplicate the active
       tab (both spell the repo name), so the brand shrinks to the mark and
       the strip takes the remaining width. -->
  <div
    class="brand flex min-w-0 items-center gap-ui-4 px-ui-45 {tabs
      ? 'flex-none'
      : 'flex-1'}"
    aria-hidden="true"
  >
    <span class="brand-mark flex-none text-primary-ink">
      <Icon icon={iconBrand} size={14} />
    </span>
    {#if !tabs}
      <span class="title truncate text-ui-meta font-medium text-muted-foreground">
        {displayTitle}
      </span>
    {/if}
  </div>

  {#if tabs}
    <!-- The merged row's center: home button + repo tabs + trailing status
         cluster / overflow (App.svelte's TabStrip, chrome-less). -->
    {@render tabs()}
  {/if}

  {#if native && !isMac}
    <div class="controls flex flex-none items-stretch">
      <button
        class={controlButtonClass}
        type="button"
        aria-label="Minimize window"
        onclick={minimize}
      >
        <Icon icon={iconMinimize} size={14} />
      </button>
      {#if maximized}
        <button
          class={controlButtonClass}
          type="button"
          aria-label="Restore window"
          onclick={toggleMaximize}
        >
          <Icon icon={iconRestore} size={13} />
        </button>
      {:else}
        <button
          class={controlButtonClass}
          type="button"
          aria-label="Maximize window"
          onclick={toggleMaximize}
        >
          <Icon icon={iconMaximize} size={13} />
        </button>
      {/if}
      <button
        class="{controlButtonClass} hover:bg-destructive hover:text-destructive-foreground"
        type="button"
        aria-label="Close window"
        onclick={close}
      >
        <Icon icon={iconClose} size={14} />
      </button>
    </div>
  {/if}
</header>

{#if native && !isMac && !maximized && !fullscreen}
  <!-- Window-wide resize zones (fixed, above in-panel floats at z-30 per
       the app.css z-index ladder, below every overlay tier). aria-hidden:
       purely pointer-driven, the OS keyboard paths (Alt+F4 & co) still own
       the window. -->
  <div class="pointer-events-none fixed inset-0 z-30" aria-hidden="true">
    {#each RESIZE_ZONES as zone (zone.direction)}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <!-- Pointer-only affordance: window edge → OS resize loop. -->
      <div
        class="pointer-events-auto absolute {zone.class}"
        onmousedown={(event) => beginResize(event, zone.direction)}
      ></div>
    {/each}
  </div>
{/if}
