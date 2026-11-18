/**
 * Shortcuts-cheatsheet overlay state (the `?` dialog).
 *
 * Lives in a `.svelte.ts` module because the open flag is a `$state` rune.
 * The palette registry (`$lib/palette/commands`) imports this store for its
 * `help.shortcuts` command, so it must import nothing back from the
 * registry. Like the palette's own store (registry #10) it registers on the
 * shared overlay stack, so the overlay's Escape defers to whatever sits
 * above it; the component handles focus, filtering and scrim clicks.
 */

import { OverlayLayer, overlayStack } from "$lib/state/overlays.svelte";

/** The overlay-stack id this cheatsheet registers under. */
const SHORTCUTS_OVERLAY_ID = "shortcuts";

let open = $state(false);

/** True while the cheatsheet overlay is shown. */
export function isShortcutsOpen(): boolean {
  return open;
}

/** Opens the cheatsheet (re-opening while open just stays open). */
export function openShortcuts(): void {
  open = true;
  overlayStack.open(SHORTCUTS_OVERLAY_ID, OverlayLayer.palette);
}

/** Closes the cheatsheet. */
export function closeShortcuts(): void {
  open = false;
  overlayStack.close(SHORTCUTS_OVERLAY_ID);
}
