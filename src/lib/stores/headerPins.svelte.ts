/**
 * Persisted header sync pins (Svelte 5 runes).
 *
 * Verbs from the header sync button's Fetch / Pull / Push menu can be pinned
 * so they get their own always-visible icon segment in the sync control
 * group (mirroring the activity-bar rail pins). The pin list is app-wide
 * (the sync control exists in every repo workspace) and persisted under a
 * single `mygitui.*` key, following the layout store's storage conventions:
 * parse-validated reads and all storage access swallowed in a try/catch
 * (privacy modes / SSR / tests may not have localStorage).
 */

import type { SyncVerb } from "$lib/git/sync";

/** Storage key for the header pin list. */
export const HEADER_PINS_KEY = "mygitui.headerPins";

/** Every sync verb is pinnable, in the order the menu lists them. */
const SYNC_VERBS: readonly SyncVerb[] = ["fetch", "pull", "push"];

/** Parses persisted pins JSON, dropping unknown/duplicate verbs (never throws). */
export function parseHeaderPins(raw: string | null): SyncVerb[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const pins: SyncVerb[] = [];
  for (const entry of parsed) {
    if (
      typeof entry === "string" &&
      SYNC_VERBS.includes(entry as SyncVerb) &&
      !pins.includes(entry as SyncVerb)
    ) {
      pins.push(entry as SyncVerb);
    }
  }
  return pins;
}

/** Reads the persisted pin list (empty when none/unavailable). */
export function loadHeaderPins(): SyncVerb[] {
  try {
    return parseHeaderPins(
      globalThis.localStorage?.getItem(HEADER_PINS_KEY) ?? null,
    );
  } catch {
    return [];
  }
}

function saveHeaderPins(pins: SyncVerb[]): void {
  try {
    globalThis.localStorage?.setItem(HEADER_PINS_KEY, JSON.stringify(pins));
  } catch {
    // Storage full/unavailable: pins still work this session.
  }
}

export class HeaderPinsStore {
  /** Pinned verbs in pin order (render order stays fetch/pull/push). */
  pins = $state<SyncVerb[]>(loadHeaderPins());

  /** True when `verb` is pinned to the header. */
  has(verb: SyncVerb): boolean {
    return this.pins.includes(verb);
  }

  /** Adds a pin (no-op for unknown verbs and duplicates). */
  pin(verb: SyncVerb): void {
    if (!SYNC_VERBS.includes(verb) || this.pins.includes(verb)) return;
    this.pins = [...this.pins, verb];
    saveHeaderPins(this.pins);
  }

  /** Removes a pin. */
  unpin(verb: SyncVerb): void {
    if (!this.pins.includes(verb)) return;
    this.pins = this.pins.filter((v) => v !== verb);
    saveHeaderPins(this.pins);
  }

  toggle(verb: SyncVerb): void {
    if (this.pins.includes(verb)) this.unpin(verb);
    else this.pin(verb);
  }

  /** Clears all pins (tests / reset). */
  clear(): void {
    this.pins = [];
    try {
      globalThis.localStorage?.removeItem(HEADER_PINS_KEY);
    } catch {
      // Storage unavailable: state still resets this session.
    }
  }
}

/** The application-wide header pin store. */
export const headerPins = new HeaderPinsStore();
