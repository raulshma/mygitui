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

/**
 * Canonical form used to compare repository paths. Windows lets the same
 * folder arrive with different spellings — folder dialogs yield
 * `C:\repos\alpha` while typed or dragged paths yield `C:/repos/alpha` —
 * so comparison folds `\\?\` prefixes, backslashes and trailing
 * separators, and lowercases Windows-shaped paths (drive letter or UNC),
 * whose filesystems are case-insensitive. POSIX paths keep their case.
 */
function canonicalKey(path: string): string {
  const windowsish = /^[a-z]:[\\/]/i.test(path) || path.startsWith("\\\\");
  const stripped = path
    .replace(/^\\\\\?\\/, "")
    .replace(/[\\/]+$/, "")
    .replace(/\\/g, "/");
  return windowsish ? stripped.toLowerCase() : stripped;
}

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
  const byKey = new Map<string, RecentRepo>();
  for (const item of parsed) {
    if (typeof item !== "object" || item === null) continue;
    const { path, pinned, lastOpened } = item as Record<string, unknown>;
    if (typeof path !== "string" || path === "") continue;
    const entry: RecentRepo = {
      path,
      pinned: pinned === true,
      lastOpened:
        typeof lastOpened === "number" && Number.isFinite(lastOpened)
          ? lastOpened
          : 0,
    };
    // Collapse separator/case variants of one folder that older builds
    // stored side by side: keep the newest spelling, OR the pins.
    const key = canonicalKey(path);
    const merged = byKey.get(key);
    if (merged === undefined) {
      byKey.set(key, entry);
      entries.push(entry);
    } else {
      merged.pinned = merged.pinned || entry.pinned;
      if (entry.lastOpened > merged.lastOpened) {
        merged.lastOpened = entry.lastOpened;
        merged.path = entry.path;
      }
    }
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

  /**
   * Adds (or re-touches) a repository path. Dedupes by canonical path, so
   * separator/case spellings of one folder stay a single entry.
   */
  add(path: string): void {
    if (typeof path !== "string" || path === "") return;
    const key = canonicalKey(path);
    const existing = this.entries.find(
      (entry) => canonicalKey(entry.path) === key,
    );
    if (existing) {
      existing.lastOpened = this.now();
    } else {
      this.entries.push({ path, pinned: false, lastOpened: this.now() });
    }
    this.persist();
  }

  /** Removes a repository path from the list (matched canonically). */
  remove(path: string): void {
    const key = canonicalKey(path);
    this.entries = this.entries.filter(
      (entry) => canonicalKey(entry.path) !== key,
    );
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
    const key = canonicalKey(path);
    const entry = this.entries.find((e) => canonicalKey(e.path) === key);
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
