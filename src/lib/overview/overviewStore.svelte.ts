/**
 * Overview store (Svelte 5 runes) — the standalone Overview view's switch
 * and view preferences. Persisted under `mygitui.overview` with the
 * versioned envelope (`$lib/state/persist`), parse-and-drop like every
 * other store: a malformed payload re-defaults, never throws.
 *
 * `enabled` mirrors the old recents-home overlay (`tabStore.recentsOpen`):
 * the zero-tabs landing is always the Overview, and the header toggle or
 * the old recents button/command enable it over open tabs. Esc or the
 * toggle (or any tab activation) disables it again. Filter/sort/chat-rail
 * preferences persist across launches; row expansion is session-only.
 */

import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";
import type {
  OverviewFilterId,
  OverviewSortId,
  SortDirection,
} from "./overviewModel";

/** localStorage key for the overview envelope. */
export const OVERVIEW_STORAGE_KEY = "mygitui.overview";

/** localStorage prefix of the deleted Agent mode's keys (first-run wipe). */
export const LEGACY_AGENT_KEY_PREFIX = "mygitui.agent";

/** What persists across launches. */
export interface OverviewState {
  /** Overview shown over open tabs (the zero-tabs landing ignores this). */
  enabled: boolean;
  /** Active filter preset; null = all rows. */
  filter: OverviewFilterId | null;
  /** Sort column + direction (persisted choice). */
  sort: OverviewSortId;
  sortDir: SortDirection;
  /** Chat rail expanded (hidden below the 1280px breakpoint regardless). */
  chatRailOpen: boolean;
}

export const OVERVIEW_DEFAULTS: OverviewState = {
  enabled: false,
  filter: null,
  sort: "attention",
  sortDir: "asc",
  chatRailOpen: true,
};

const FILTER_IDS: OverviewFilterId[] = [
  "needs-push",
  "needs-pull",
  "dirty",
  "has-prs",
  "failing-checks",
  "missing",
];

const SORT_IDS: OverviewSortId[] = [
  "attention",
  "name",
  "last-commit",
  "ahead",
  "behind",
  "changes",
  "prs",
];

/** Parse-and-drop loader: any wrong field drops the WHOLE payload. */
export function parseOverviewState(data: unknown): OverviewState | null {
  if (typeof data !== "object" || data === null || Array.isArray(data)) return null;
  const raw = data as Record<string, unknown>;
  if (Object.keys(raw).length !== 5) return null;
  if (typeof raw.enabled !== "boolean") return null;
  if (
    raw.filter !== null &&
    !FILTER_IDS.includes(raw.filter as OverviewFilterId)
  ) {
    return null;
  }
  if (!SORT_IDS.includes(raw.sort as OverviewSortId)) return null;
  if (raw.sortDir !== "asc" && raw.sortDir !== "desc") return null;
  if (typeof raw.chatRailOpen !== "boolean") return null;
  return {
    enabled: raw.enabled,
    filter: raw.filter as OverviewFilterId | null,
    sort: raw.sort as OverviewSortId,
    sortDir: raw.sortDir,
    chatRailOpen: raw.chatRailOpen,
  };
}

/** Returns `localStorage` when available, else `null` (never throws). */
export function safeStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/**
 * One-time hygiene: removes the deleted Agent mode's storage (`mygitui.agent`
 * and the per-root `mygitui.agent.<root>` sessions). Idempotent; safe to call
 * at every app start. Returns the removed keys (for tests).
 */
export function wipeLegacyAgentKeys(
  storage: (StorageLike & { removeItem?(key: string): void; key?(index: number): string | null; length?: number }) | null = safeStorage(),
): string[] {
  if (!storage) return [];
  const removed: string[] = [];
  try {
    const length = storage.length ?? 0;
    const keys: string[] = [];
    for (let i = 0; i < length; i += 1) {
      const key = storage.key?.(i) ?? null;
      if (
        key !== null &&
        (key === LEGACY_AGENT_KEY_PREFIX ||
          key.startsWith(`${LEGACY_AGENT_KEY_PREFIX}.`))
      ) {
        keys.push(key);
      }
    }
    for (const key of keys) {
      storage.removeItem?.(key);
      removed.push(key);
    }
  } catch {
    // Storage unavailable/unreadable: nothing to wipe this session.
  }
  return removed;
}

export class OverviewStore {
  #storage: StorageLike | null;
  #state = $state<OverviewState>({ ...OVERVIEW_DEFAULTS });

  constructor(storage: StorageLike | null = safeStorage()) {
    this.#storage = storage;
    const loaded = loadVersioned<OverviewState>({
      storage,
      key: OVERVIEW_STORAGE_KEY,
      parse: parseOverviewState,
    });
    this.#state = loaded ?? { ...OVERVIEW_DEFAULTS };
  }

  /** Whether the Overview is shown over open tabs (reactive). */
  get enabled(): boolean {
    return this.#state.enabled;
  }

  get filter(): OverviewFilterId | null {
    return this.#state.filter;
  }

  get sort(): OverviewSortId {
    return this.#state.sort;
  }

  get sortDir(): SortDirection {
    return this.#state.sortDir;
  }

  get chatRailOpen(): boolean {
    return this.#state.chatRailOpen;
  }

  /** The whole state snapshot (tests / reset). */
  get snapshot(): OverviewState {
    return { ...this.#state };
  }

  #set(patch: Partial<OverviewState>): void {
    this.#state = { ...this.#state, ...patch };
    writeEnvelope(this.#storage, OVERVIEW_STORAGE_KEY, this.#state);
  }

  /** Shows the Overview and persists. */
  enable(): void {
    this.#set({ enabled: true });
  }

  /** Hides the Overview (back to the workspace) and persists. */
  disable(): void {
    this.#set({ enabled: false });
  }

  /** Flips the Overview; returns the new value. */
  toggle(): boolean {
    const next = !this.#state.enabled;
    this.#set({ enabled: next });
    return next;
  }

  /** Sets the active filter preset (null clears it). */
  setFilter(filter: OverviewFilterId | null): void {
    this.#set({ filter });
  }

  /** Sets the sort column; re-clicking the column flips the direction. */
  setSort(sort: OverviewSortId): void {
    if (this.#state.sort === sort) {
      this.#set({ sortDir: this.#state.sortDir === "asc" ? "desc" : "asc" });
    } else {
      this.#set({ sort, sortDir: sort === "name" ? "asc" : "desc" });
    }
  }

  /** Sets the sort direction outright. */
  setSortDir(sortDir: SortDirection): void {
    this.#set({ sortDir });
  }

  /** Shows/hides the chat rail. */
  setChatRailOpen(open: boolean): void {
    this.#set({ chatRailOpen: open });
  }

  /** Drops all state to the defaults (tests; storage untouched). */
  reset(): void {
    this.#state = { ...OVERVIEW_DEFAULTS };
  }
}

/** Overview singleton (localStorage-backed). */
export const overviewStore = new OverviewStore();
