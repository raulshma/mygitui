/**
 * Shortcuts-hint dismissal state (plan §6.4): whether the STATUS panel's
 * keyboard-shortcuts hint row is shown, persisted so "default ON for first
 * run only" holds — the very first session shows the row (nothing stored),
 * and once the user dismisses it the row stays off in every later session.
 * Re-opening it from the PanelHeader overflow ("Keyboard shortcuts") is a
 * real request, so it also clears the persisted dismissal.
 *
 * Storage follows the per-feature localStorage convention (try/catch —
 * never throws; injectable `StorageLike` for tests) under
 * `mygitui.status.shortcutsHint` in the versioned envelope
 * (`$lib/state/persist`), parse-and-drop like every other store.
 */

import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";

/** localStorage key for the shortcuts-hint dismissal envelope. */
export const SHORTCUTS_HINT_KEY = "mygitui.status.shortcutsHint";

/** What persists: the row's dismissed flag (absent = never dismissed). */
export interface ShortcutsHintData {
  dismissed: boolean;
}

/** Parse-and-drop: only the boolean survives; junk re-defaults to shown. */
export function parseShortcutsHint(data: unknown): ShortcutsHintData | null {
  if (typeof data !== "object" || data === null) return null;
  const dismissed = (data as { dismissed?: unknown }).dismissed;
  return typeof dismissed === "boolean" ? { dismissed } : null;
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/**
 * The dismissal store — construct for tests; the app uses the module-level
 * {@link shortcutsHintPrefs} singleton.
 */
export class ShortcutsHintPrefs {
  #dismissed = $state(false);
  readonly #storage: StorageLike | null;

  constructor(storage: StorageLike | null = defaultStorage()) {
    this.#storage = storage;
    const loaded = loadVersioned<ShortcutsHintData>({
      storage,
      key: SHORTCUTS_HINT_KEY,
      parse: parseShortcutsHint,
    });
    // No stored flag = first run: the hint row shows (plan §6.4).
    if (loaded !== null) this.#dismissed = loaded.dismissed;
  }

  /** Whether the shortcuts hint row is hidden. */
  get dismissed(): boolean {
    return this.#dismissed;
  }

  /** Dismisses the row and persists the flag (default off hereafter). */
  dismiss(): void {
    this.#dismissed = true;
    this.#write();
  }

  /**
   * Re-opens the row (PanelHeader overflow "Keyboard shortcuts") and clears
   * the persisted dismissal — an explicit request keeps it on.
   */
  reopen(): void {
    this.#dismissed = false;
    this.#write();
  }

  #write(): void {
    writeEnvelope(this.#storage, SHORTCUTS_HINT_KEY, { dismissed: this.#dismissed });
  }
}

/** The app-wide shortcuts-hint singleton. */
export const shortcutsHintPrefs = new ShortcutsHintPrefs();
