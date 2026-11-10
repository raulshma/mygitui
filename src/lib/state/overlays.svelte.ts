/**
 * Overlay stack owner (registry #10, `docs/state-machines.md`): one place
 * that knows which full-screen overlays are open and which one is on top,
 * so an overlay's close affordance (Escape in particular) can defer to
 * whatever visually sits above it instead of every handler acting on its
 * own.
 *
 * Layered, not FIFO: overlays occupy fixed visual tiers (a context menu
 * floats above the palette, a modal dialog above the menu), so "top" is
 * the highest layer among the open overlays — registration order is only
 * the tiebreak within one layer. An overlay whose Escape closes just
 * itself and cannot be covered (focus-scoped bits-ui dialogs) does not
 * need to consult this; the owner exists for overlays that can stack
 * (palette over the conflict editor, context menu over the palette, …).
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

/**
 * Visual tiers, lowest first. Keep the numbers together — "top" is the
 * highest layer of the open set.
 */
export const OverlayLayer = {
  /** Panel-embedded surfaces (conflict editor). */
  panel: 10,
  /** The command palette's scrim covers the workspace. */
  palette: 30,
  /** Context menus float above everything except modals. */
  contextMenu: 40,
  /** Modal dialogs (clone, prompt/confirm, device, settings). */
  dialog: 50,
} as const;

export type OverlayLayerId = keyof typeof OverlayLayer;

export class OverlayStack {
  /** id → visual layer, open overlays only. */
  #open = $state<Map<string, number>>(new Map());

  /** Marks `id` open at `layer` (idempotent; re-opening re-layers). */
  open(id: string, layer: number): void {
    const next = new Map(this.#open);
    next.set(id, layer);
    this.#open = next;
  }

  /** Marks `id` closed (no-op when it was not open). */
  close(id: string): void {
    if (!this.#open.has(id)) return;
    const next = new Map(this.#open);
    next.delete(id);
    this.#open = next;
  }

  /** True when `id` is open. */
  isOpen(id: string): boolean {
    return this.#open.has(id);
  }

  /**
   * True when `id` is the topmost open overlay: the highest layer, and
   * among equals the most recently (re-)opened. Escape/close affordances
   * of lower overlays defer to this.
   */
  isTop(id: string): boolean {
    return this.#open.has(id) && this.top === id;
  }

  /** The topmost open overlay id, or `null` when nothing is open. */
  get top(): string | null {
    let bestId: string | null = null;
    let bestLayer = -Infinity;
    // `>=`: within one layer, the later entry (insertion order) wins —
    // the most recently opened overlay is the one on screen.
    for (const [candidate, candidateLayer] of this.#open) {
      if (candidateLayer >= bestLayer) {
        bestId = candidate;
        bestLayer = candidateLayer;
      }
    }
    return bestId;
  }
}

/** The application-wide overlay stack. */
export const overlayStack = new OverlayStack();
