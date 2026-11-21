/**
 * Reactive active view for the §10 Stacked fallback (<800px) — which
 * full-screen workspace region the bottom tab bar shows.
 *
 * Shell-level state (one workspace, whatever repo is active), persisted
 * globally through a `mygitui.*` envelope key — like every other shell
 * pref, not per-repo. {@link refresh} hydrates ONCE per session (the
 * persisted view, else the caller's context fallback); later calls are
 * no-ops so in-session switches always win, and re-entering Stacked from
 * another width mode resumes the last stacked view instead of re-deriving
 * it. {@link setActive} switches and writes through (immediate, unlike the
 * debounced split prefs — one small write per explicit user action).
 *
 * The active view is a plain PanelId: any panel can be a stacked view
 * (the bar's five primaries plus everything behind "More"). Panels that
 * are not currently in the layout tree are placed by the caller
 * (RepoView) before activation — the store only tracks the choice.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

import { PANEL_IDS, type PanelId } from "./layoutModel";
import { STACKED_DEFAULT_VIEW } from "./stackedPlan";
import { loadVersioned, writeEnvelope } from "$lib/state/persist";

/** Storage key carrying the last stacked view (envelope v1, plain id). */
export const STACKED_VIEW_PREF_KEY = "mygitui.stacked.view";

/** Parses a persisted view id; `null` when missing/malformed/unknown. */
function parseView(data: unknown): PanelId | null {
  return typeof data === "string" &&
    (PANEL_IDS as readonly string[]).includes(data)
    ? (data as PanelId)
    : null;
}

/** Reads the persisted view; `null` when absent/malformed/storage off. */
function readPersisted(): PanelId | null {
  try {
    return loadVersioned<PanelId>({
      storage: globalThis.localStorage ?? null,
      key: STACKED_VIEW_PREF_KEY,
      parse: parseView,
    });
  } catch {
    return null;
  }
}

/** Writes the view (storage failures swallowed — the session keeps it). */
function writePersisted(view: PanelId): void {
  try {
    writeEnvelope(globalThis.localStorage ?? null, STACKED_VIEW_PREF_KEY, view);
  } catch {
    // Storage unavailable: the session still switches views.
  }
}

const view = $state<{ current: PanelId }>({ current: STACKED_DEFAULT_VIEW });

/** Whether the store has hydrated from storage (once per session). */
let hydrated = false;

export const workspaceStacked = {
  /** The active stacked view (the full-screen region the bar shows). */
  get active(): PanelId {
    return view.current;
  },

  /**
   * Hydrates once: the persisted view, else `fallback` (the caller's
   * context — the panel the user was just looking at in the wider mode).
   * Later calls keep the in-session choice.
   */
  refresh(fallback: PanelId): void {
    if (hydrated) return;
    hydrated = true;
    view.current = readPersisted() ?? fallback;
  },

  /** Switches the stacked view and persists it. */
  setActive(panel: PanelId): void {
    view.current = panel;
    writePersisted(panel);
  },
};
