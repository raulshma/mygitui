<script lang="ts">
  /**
   * SplitPane — generic two-pane splitter for panel-internal resize handles
   * (history list ↔ commit detail, detail meta ↔ diff), independent of the
   * workspace `LayoutNode` tree that `SplitContainer` renders.
   *
   * `axis: "x"` puts pane `a` left of the divider and `b` right (ratio =
   * a's width fraction); `axis: "y"` puts `a` on top (ratio = a's height
   * fraction). The divider is a focusable `role="separator"`: pointer drag
   * (clamped to RATIO_MIN–RATIO_MAX by the caller) plus arrow keys nudge
   * ±RATIO_STEP and Home/End jump to the clamps. Ratio state lives with the
   * caller — this component only reports drags through `onRatio`, matching
   * SplitContainer's contract.
   */
  import type { Snippet } from "svelte";
  import { RATIO_MAX, RATIO_MIN, RATIO_STEP, clampRatio } from "$lib/layout/layoutModel";

  let {
    axis,
    ratio,
    onRatio,
    label,
    a,
    b,
  }: {
    /** Drag axis: `"x"` = panes side by side, `"y"` = panes stacked. */
    axis: "x" | "y";
    /** Pane `a`'s fraction (width for x, height for y). */
    ratio: number;
    /** Drag/keyboard resize reported here (raw; callers clamp or pass
     *  through `clampRatio`). */
    onRatio: (ratio: number) => void;
    /** Accessible name for the separator ("Resize commit list and detail"). */
    label: string;
    a: Snippet;
    b: Snippet;
  } = $props();

  // -- divider dragging (same pointer-capture pattern as SplitContainer) ----

  let dragging = $state(false);
  let box = $state<{ x: number; y: number; w: number; h: number } | null>(null);

  function dividerDown(event: PointerEvent): void {
    const host = event.currentTarget as HTMLElement;
    const rect = host.parentElement?.getBoundingClientRect();
    if (!rect || rect.width === 0 || rect.height === 0) return;
    event.preventDefault();
    dragging = true;
    box = { x: rect.left, y: rect.top, w: rect.width, h: rect.height };
    try {
      host.setPointerCapture(event.pointerId);
    } catch {
      // Pointer capture is best-effort; moves still work while hovered.
    }
  }

  function dividerMove(event: PointerEvent): void {
    if (!box) return;
    const raw =
      axis === "x" ? (event.clientX - box.x) / box.w : (event.clientY - box.y) / box.h;
    onRatio(clampRatio(raw));
  }

  function dividerUp(event: PointerEvent): void {
    if (!dragging) return;
    try {
      (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
    } catch {
      // Already released (pointercancel path).
    }
    dragging = false;
    box = null;
  }

  function dividerKey(event: KeyboardEvent): void {
    const before = axis === "x" ? "ArrowLeft" : "ArrowUp";
    const after = axis === "x" ? "ArrowRight" : "ArrowDown";
    if (event.key === before) {
      event.preventDefault();
      onRatio(clampRatio(ratio - RATIO_STEP));
    } else if (event.key === after) {
      event.preventDefault();
      onRatio(clampRatio(ratio + RATIO_STEP));
    } else if (event.key === "Home") {
      event.preventDefault();
      onRatio(RATIO_MIN);
    } else if (event.key === "End") {
      event.preventDefault();
      onRatio(RATIO_MAX);
    }
  }
</script>

<div class="split" class:y={axis === "y"}>
  <div class="pane" style:flex={`0 0 ${ratio * 100}%`}>
    {@render a()}
  </div>
  <!-- A focusable separator is the ARIA-recommended pattern for split
       dividers (keyboard resize); the a11y lint can't see the role. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="divider"
    class:dragging
    role="separator"
    aria-orientation={axis === "x" ? "vertical" : "horizontal"}
    aria-label={label}
    aria-valuemin={Math.round(RATIO_MIN * 100)}
    aria-valuemax={Math.round(RATIO_MAX * 100)}
    aria-valuenow={Math.round(ratio * 100)}
    tabindex="0"
    onpointerdown={dividerDown}
    onpointermove={dividerMove}
    onpointerup={dividerUp}
    onpointercancel={dividerUp}
    onkeydown={dividerKey}
  ></div>
  <div class="pane grow">
    {@render b()}
  </div>
</div>

<style>
  .split {
    display: flex;
    width: 100%;
    height: 100%;
    min-width: 0;
    min-height: 0;
  }

  /* Stacked panes: the divider runs horizontally. */
  .split.y {
    flex-direction: column;
  }

  .pane {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .pane.grow {
    flex: 1 1 0%;
  }

  .pane > :global(*) {
    flex: 1;
    min-height: 0;
  }

  .divider {
    flex: none;
    width: 5px;
    margin: 0 -2px;
    z-index: 1;
    cursor: col-resize;
    touch-action: none;
    background:
      linear-gradient(
        to right,
        transparent 2px,
        var(--m3-outline-variant, var(--m3-primary)) 2px,
        var(--m3-outline-variant, var(--m3-primary)) 3px,
        transparent 3px
      );
  }

  .split.y .divider {
    width: auto;
    height: 5px;
    margin: -2px 0;
    cursor: row-resize;
    background:
      linear-gradient(
        to bottom,
        transparent 2px,
        var(--m3-outline-variant, var(--m3-primary)) 2px,
        var(--m3-outline-variant, var(--m3-primary)) 3px,
        transparent 3px
      );
  }

  .divider:hover,
  .divider:focus-visible,
  .divider.dragging {
    background: var(--m3-primary);
  }

  .divider:focus-visible {
    outline: none;
  }
</style>
