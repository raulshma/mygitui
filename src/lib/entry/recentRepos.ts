/**
 * Recent-repositories store (M0 placeholder).
 *
 * Persists to localStorage under `mygitui.recent-repos`; will be upgraded to
 * Rust-side storage in a later milestone, so the shape is deliberately simple
 * (a plain, serializable list of {@link RecentRepo} entries).
 */

/** A single recent-repository entry as persisted to storage. */
export interface RecentRepo {
  path: string;
  pinned: boolean;
  lastOpened: number;
}

/** Minimal storage surface this store needs (subset of DOM `Storage`). */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

const STORAGE_KEY = "mygitui.recent-repos";

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    const storage = globalThis.localStorage;
    return storage ?? null;
  } catch {
    // Accessing localStorage can throw (privacy modes / sandboxes).
    return null;
  }
}

/** Parses raw persisted JSON into valid entries, dropping anything malformed. */
function parseEntries(raw: string | null): RecentRepo[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];

  const entries: RecentRepo[] = [];
  const seen = new Set<string>();
  for (const item of parsed) {
    if (typeof item !== "object" || item === null) continue;
    const { path, pinned, lastOpened } = item as Record<string, unknown>;
    if (typeof path !== "string" || path === "" || seen.has(path)) continue;
    seen.add(path);
    entries.push({
      path,
      pinned: pinned === true,
      lastOpened:
        typeof lastOpened === "number" && Number.isFinite(lastOpened)
          ? lastOpened
          : 0,
    });
  }
  return entries;
}

export class RecentRepoStore {
  private readonly storage: StorageLike | null;
  private readonly now: () => number;
  private entries: RecentRepo[];

  constructor(
    storage: StorageLike | null = defaultStorage(),
    now: () => number = () => Date.now(),
  ) {
    this.storage = storage;
    this.now = now;
    this.entries = parseEntries(storage?.getItem(STORAGE_KEY) ?? null);
  }

  /** Adds (or re-touches) a repository path. Dedupes by path. */
  add(path: string): void {
    if (typeof path !== "string" || path === "") return;
    const existing = this.entries.find((entry) => entry.path === path);
    if (existing) {
      existing.lastOpened = this.now();
    } else {
      this.entries.push({ path, pinned: false, lastOpened: this.now() });
    }
    this.persist();
  }

  /** Removes a repository path from the list. */
  remove(path: string): void {
    this.entries = this.entries.filter((entry) => entry.path !== path);
    this.persist();
  }

  /**
   * Returns all entries: pinned first, then most recently opened.
   * The returned array is a detached copy.
   */
  list(): RecentRepo[] {
    return [...this.entries].sort(
      (a, b) =>
        Number(b.pinned) - Number(a.pinned) || b.lastOpened - a.lastOpened,
    );
  }

  /** Flips the pinned flag of a repository path (no-op when unknown). */
  togglePin(path: string): void {
    const entry = this.entries.find((e) => e.path === path);
    if (!entry) return;
    entry.pinned = !entry.pinned;
    this.persist();
  }

  private persist(): void {
    if (!this.storage) return;
    try {
      this.storage.setItem(STORAGE_KEY, JSON.stringify(this.entries));
    } catch {
      // Storage may be full or unavailable; keep the in-memory list usable.
    }
  }
}
