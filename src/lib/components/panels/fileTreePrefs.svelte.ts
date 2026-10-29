/**
 * Files-panel preferences: per-repo expanded-directory sets (persisted for
 * the next visit, so the tree reopens the way you left it) and the global
 * "show ignored files" toggle.
 *
 * Storage follows the per-feature localStorage convention (try/catch —
 * never throws; injectable `StorageLike` for tests). Expanded sets are
 * stored per repo root under `mygitui.files.expanded.<root>`; writes are
 * debounced so flipping ten chevrons in a second costs one write, like
 * `layout.svelte.ts`.
 */

const EXPANDED_KEY_PREFIX = "mygitui.files.expanded.";
const SHOW_IGNORED_KEY = "mygitui.files.show-ignored";
const PERSIST_DEBOUNCE_MS = 150;

/** Storage backend (localStorage in the app; injectable for tests). */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem?(key: string): void;
}

function defaultStorage(): StorageLike | null {
  try {
    if (typeof localStorage !== "undefined") return localStorage;
  } catch {
    // Unavailable (tests / non-browser).
  }
  return null;
}

interface FileTreePrefsOptions {
  storage?: StorageLike | null;
}

export class FileTreePrefs {
  private storage: StorageLike | null;
  private timers = new Map<string, ReturnType<typeof setTimeout>>();
  private expandedSets = new Map<string, Set<string>>();

  /** Global toggle: bypass the backend gitignore filter. */
  showIgnored = $state(false);

  constructor(options: FileTreePrefsOptions = {}) {
    this.storage =
      options.storage !== undefined ? options.storage : defaultStorage();
    this.showIgnored = this.readShowIgnored();
  }

  // -- expanded sets ----------------------------------------------------

  /** Hydrate one repo's expanded set (cheap; called from a `$effect`). */
  ensure(root: string): void {
    if (this.expandedSets.has(root)) return;
    const stored = this.readExpanded(root);
    this.expandedSets.set(root, stored);
  }

  /** Whether `path` is expanded for this repo ("" is always expanded). */
  isExpanded(root: string, path: string): boolean {
    if (path === "") return true;
    return this.expandedSets.get(root)?.has(path) ?? false;
  }

  /** Flip one directory's expanded state and persist (debounced). */
  setExpanded(root: string, path: string, expanded: boolean): void {
    if (path === "") return;
    this.ensure(root);
    const set = this.expandedSets.get(root)!;
    if (expanded) {
      set.add(path);
    } else {
      set.delete(path);
    }
    this.scheduleWrite(root, set);
  }

  /** Drop every per-repo cache (tab closed; nothing persisted). */
  forget(root: string): void {
    const timer = this.timers.get(root);
    if (timer) {
      clearTimeout(timer);
      this.timers.delete(root);
    }
    this.expandedSets.delete(root);
  }

  private readExpanded(root: string): Set<string> {
    try {
      const raw = this.storage?.getItem(EXPANDED_KEY_PREFIX + root);
      if (!raw) return new Set();
      const parsed: unknown = JSON.parse(raw);
      if (!Array.isArray(parsed)) return new Set();
      return new Set(parsed.filter((p): p is string => typeof p === "string"));
    } catch {
      return new Set();
    }
  }

  private scheduleWrite(root: string, set: Set<string>): void {
    const existing = this.timers.get(root);
    if (existing) clearTimeout(existing);
    this.timers.set(
      root,
      setTimeout(() => {
        this.timers.delete(root);
        try {
          this.storage?.setItem(
            EXPANDED_KEY_PREFIX + root,
            JSON.stringify([...set]),
          );
        } catch {
          // Keep the in-memory state if storage is unavailable.
        }
      }, PERSIST_DEBOUNCE_MS),
    );
  }

  // -- show-ignored toggle -----------------------------------------------

  setShowIgnored(value: boolean): void {
    this.showIgnored = value;
    try {
      this.storage?.setItem(SHOW_IGNORED_KEY, value ? "1" : "0");
    } catch {
      // Keep the in-memory preference.
    }
  }

  private readShowIgnored(): boolean {
    try {
      return this.storage?.getItem(SHOW_IGNORED_KEY) === "1";
    } catch {
      return false;
    }
  }
}

/** App singleton (localStorage-backed). */
export const fileTreePrefs = new FileTreePrefs();
