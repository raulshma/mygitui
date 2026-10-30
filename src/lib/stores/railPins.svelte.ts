/**
 * Persisted activity-bar rail pins (Svelte 5 runes).
 *
 * Panels from the rail's "More panels" overflow menu can be pinned so they
 * get their own always-visible rail button (like the default
 * `RAIL_PANELS`). The pin list is app-wide (the rail exists in every repo
 * workspace) and persisted under a single `mygitui.*` key, following the
 * layout store's storage conventions: parse-validated reads and all storage
 * access swallowed in a try/catch (privacy modes / SSR / tests may not have
 * localStorage).
 */

import { PANEL_IDS, RAIL_PANELS, type PanelId } from "$lib/layout/layoutModel";

/** Storage key for the pin list. */
export const RAIL_PINS_KEY = "mygitui.railPins";

/**
 * Panels that can never be pinned: `history` keeps its own bottom rail
 * button (right sidebar toggle) and `diff` is the center editor, so neither
 * is a primary-sidebar panel (matches `RepoView`'s `activityPanels`).
 */
const UNPINNABLE: readonly PanelId[] = ["history", "diff"];

/** Parses persisted pins JSON, dropping unknown/duplicate ids (never throws). */
export function parseRailPins(raw: string | null): PanelId[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const pins: PanelId[] = [];
  for (const entry of parsed) {
    if (
      typeof entry === "string" &&
      PANEL_IDS.includes(entry as PanelId) &&
      !RAIL_PANELS.includes(entry as PanelId) &&
      !UNPINNABLE.includes(entry as PanelId) &&
      !pins.includes(entry as PanelId)
    ) {
      pins.push(entry as PanelId);
    }
  }
  return pins;
}

/** Reads the persisted pin list (empty when none/unavailable). */
export function loadRailPins(): PanelId[] {
  try {
    return parseRailPins(globalThis.localStorage?.getItem(RAIL_PINS_KEY) ?? null);
  } catch {
    return [];
  }
}

function saveRailPins(pins: PanelId[]): void {
  try {
    globalThis.localStorage?.setItem(RAIL_PINS_KEY, JSON.stringify(pins));
  } catch {
    // Storage full/unavailable: pins still work this session.
  }
}

export class RailPinsStore {
  /** Pinned panels in pin order (default `RAIL_PANELS` are not included). */
  pins = $state<PanelId[]>(loadRailPins());

  /** True when `panel` is pinned (defaults count — they are always shown). */
  has(panel: PanelId): boolean {
    return this.pins.includes(panel);
  }

  /** Adds a pin (no-op for defaults, unpinnable panels, duplicates). */
  pin(panel: PanelId): void {
    if (
      RAIL_PANELS.includes(panel) ||
      UNPINNABLE.includes(panel) ||
      this.pins.includes(panel)
    ) {
      return;
    }
    this.pins = [...this.pins, panel];
    saveRailPins(this.pins);
  }

  /** Removes a pin; default `RAIL_PANELS` cannot be unpinned. */
  unpin(panel: PanelId): void {
    if (!this.pins.includes(panel)) return;
    this.pins = this.pins.filter((p) => p !== panel);
    saveRailPins(this.pins);
  }

  toggle(panel: PanelId): void {
    if (this.pins.includes(panel)) this.unpin(panel);
    else this.pin(panel);
  }

  /** Clears all pins (tests / reset). */
  clear(): void {
    this.pins = [];
    try {
      globalThis.localStorage?.removeItem(RAIL_PINS_KEY);
    } catch {
      // Storage unavailable: state still resets this session.
    }
  }
}

/** The application-wide rail pin store. */
export const railPins = new RailPinsStore();
