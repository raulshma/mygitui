/**
 * Status-panel tree state: per-repo collapsed sections and accordions
 * (Staged Changes / Changes headers, the repository accordion in
 * submodule mode, and per-submodule accordions + sections), persisted
 * so the list reopens the way you left it.
 *
 * Storage follows the per-feature localStorage convention (try/catch —
 * never throws; injectable `StorageLike` for tests). The collapsed map
 * is stored per repo root under `mygitui.status.collapsed.<root>` in
 * the versioned envelope (`$lib/state/persist`), parse-and-drop like
 * every other store: a malformed payload re-defaults to all-expanded,
 * never throws. Writes are debounced so flipping ten chevrons in a
 * second costs one write, like `fileTreePrefs.svelte.ts`. Values are
 * flat string→boolean records — the component owns the key vocabulary
 * (`section:<id>`, `repo`, `mod:<path>`, `sub:<path>:<section>`).
 */

import {
  loadVersioned,
  writeEnvelope,
  type StorageLike,
} from "$lib/state/persist";

const COLLAPSED_KEY_PREFIX = "mygitui.status.collapsed.";

/** Debounce window for persisted writes (exported for tests). */
export const STATUS_TREE_PREFS_DEBOUNCE_MS = 150;

function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

interface StatusTreePrefsOptions {
  storage?: StorageLike | null;
}

/** Parse-and-drop: booleans survive, anything else re-defaults to {} (all-expanded). */
export function parseCollapsedMap(data: unknown): Record<string, boolean> {
  if (typeof data !== "object" || data === null || Array.isArray(data)) return {};
  const out: Record<string, boolean> = {};
  for (const [key, value] of Object.entries(data)) {
    if (typeof value === "boolean") out[key] = value;
  }
  return out;
}

export class StatusTreePrefs {
  #storage: StorageLike | null;
  #timers = new Map<string, ReturnType<typeof setTimeout>>();
  #states = new Map<string, Record<string, boolean>>();

  constructor(options: StatusTreePrefsOptions = {}) {
    this.#storage =
      options.storage !== undefined ? options.storage : defaultStorage();
  }

  /**
   * Hydrate one repo's collapsed map and return it (cheap; called on
   * repo switch — never inside `$derived`, the store mutates its cache).
   */
  snapshot(root: string): Record<string, boolean> {
    let state = this.#states.get(root);
    if (!state) {
      state =
        loadVersioned<Record<string, boolean>>({
          storage: this.#storage,
          key: COLLAPSED_KEY_PREFIX + root,
          parse: parseCollapsedMap,
        }) ?? {};
      this.#states.set(root, state);
    }
    return state;
  }

  /** Record one key's collapsed state and persist (debounced). */
  set(root: string, key: string, collapsed: boolean): void {
    const state = this.snapshot(root);
    state[key] = collapsed;
    this.scheduleWrite(root, state);
  }

  private scheduleWrite(root: string, state: Record<string, boolean>): void {
    const existing = this.#timers.get(root);
    if (existing) clearTimeout(existing);
    this.#timers.set(
      root,
      setTimeout(() => {
        this.#timers.delete(root);
        writeEnvelope(this.#storage, COLLAPSED_KEY_PREFIX + root, state);
      }, STATUS_TREE_PREFS_DEBOUNCE_MS),
    );
  }
}

/** App singleton (localStorage-backed). */
export const statusTreePrefs = new StatusTreePrefs();
