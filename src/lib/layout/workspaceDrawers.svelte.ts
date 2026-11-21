/**
 * Reactive open/closed state for the workspace overlay drawers (plan §10
 * mechanics: "open/closed state persisted per mode").
 *
 * The state is shell-level (one workspace, whatever repo is active) and
 * persists per WIDTH MODE through `splitPrefs` — closing the Files drawer
 * in Narrow does not close it in Medium, and reopening the app in the same
 * mode restores the last state. `refreshMode` re-reads the persisted state
 * whenever the width mode flips (called from `RepoView`'s mode effect);
 * `setOpen`/`toggle` write through to the same per-mode store.
 *
 * Reads are only meaningful for panels the §10 matrix drawerizes at the
 * current mode (`drawerSide(panel, mode) !== null`); callers gate on that
 * (stale values from a previous mode are ignored, never written back).
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

import {
  DRAWER_PANELS,
  DRAWER_PREF_KEY,
  drawerFallbackOpen,
  drawerSide,
  type DrawerPanel,
} from "./drawerPlan";
import { readSplitModeOpen, writeSplitModeOpen } from "./splitPrefs";
import type { WidthMode } from "./widthMode.svelte";

const open = $state<Record<DrawerPanel, boolean>>({
  files: false,
  status: false,
  history: false,
});

export const workspaceDrawers = {
  /** Whether `panel`'s drawer is open (see the module note on gating). */
  isOpen(panel: DrawerPanel): boolean {
    return open[panel];
  },

  /**
   * Re-reads the persisted per-mode open state for every panel drawerized
   * at `mode`. Panels the mode keeps docked keep their stale value — the
   * next `refreshMode` that drawerizes them re-reads it.
   */
  refreshMode(mode: WidthMode): void {
    for (const panel of DRAWER_PANELS) {
      if (drawerSide(panel, mode) === null) continue;
      open[panel] = readSplitModeOpen(
        DRAWER_PREF_KEY[panel],
        mode,
        drawerFallbackOpen(panel, mode),
      );
    }
  },

  /** Sets + persists `panel`'s open state for `mode`. */
  setOpen(panel: DrawerPanel, mode: WidthMode, next: boolean): void {
    open[panel] = next;
    writeSplitModeOpen(DRAWER_PREF_KEY[panel], mode, next);
  },

  /** Flips + persists `panel`'s open state; returns the new state. */
  toggle(panel: DrawerPanel, mode: WidthMode): boolean {
    const next = !open[panel];
    this.setOpen(panel, mode, next);
    return next;
  },
};
