/**
 * Commit bookmark store (Svelte 5 runes) — M7 lane I1.
 *
 * Per-repo pinned commits: `{ sha, label }[]` persisted FE-side (per
 * contracts.md) under localStorage `mygitui.bookmarks.<root>`. The graph
 * displays a small ring marker on bookmarked nodes (display-only in M7 —
 * no click behavior); the store API is the single source of truth.
 *
 * Persistence keys off the repo ROOT (stable across repo-id churn, like
 * the layout overlays). Hydration is lazy per root but must happen OUTSIDE
 * derived reads (the store writes `$state`): call `ensure(root)` from an
 * `$effect`, mirroring the layouts store contract.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Storage is injectable (tests use a map-backed fake).
 */

/** localStorage key for one repo root. */
export function bookmarksKey(root: string): string {
  return `mygitui.bookmarks.${root}`;
}

/** One pinned commit. */
export interface Bookmark {
  /** Full commit sha. */
  sha: string;
  /** Short human label (defaults to the short sha). */
  label: string;
}

/** Minimal storage surface this store needs (subset of DOM `Storage`). */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    // Accessing localStorage can throw (privacy modes / sandboxes).
    return null;
  }
}

/** Parses persisted bookmarks; drops malformed entries and duplicates. */
function parseBookmarks(raw: string | null): Bookmark[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const out: Bookmark[] = [];
  const seen = new Set<string>();
  for (const item of parsed) {
    const b = item as Partial<Bookmark> | null;
    if (typeof b?.sha === "string" && b.sha.trim() !== "" && !seen.has(b.sha)) {
      seen.add(b.sha);
      out.push({
        sha: b.sha,
        label:
          typeof b.label === "string" && b.label.trim() !== ""
            ? b.label
            : b.sha.slice(0, 7),
      });
    }
  }
  return out;
}

export class BookmarkStore {
  #byRoot: Record<string, Bookmark[]> = $state({});
  readonly #storage: StorageLike | null;
  #hydrated = new Set<string>();

  constructor(storage: StorageLike | null = defaultStorage()) {
    this.#storage = storage;
  }

  /** Hydrates `root`'s persisted list (idempotent; call from an `$effect`). */
  ensure(root: string): void {
    if (this.#hydrated.has(root)) return;
    this.#hydrated.add(root);
    this.#byRoot[root] = parseBookmarks(
      this.#storage?.getItem(bookmarksKey(root)) ?? null,
    );
  }

  /** Bookmarks of `root`, insertion order (empty when unknown). */
  list(root: string): Bookmark[] {
    return this.#byRoot[root] ?? [];
  }

  /** Bookmarked shas of `root` as a set (for the graph prop). */
  shas(root: string): Set<string> {
    return new Set(this.list(root).map((b) => b.sha));
  }

  /** True when `sha` is bookmarked in `root`. */
  has(root: string, sha: string): boolean {
    return this.list(root).some((b) => b.sha === sha);
  }

  /**
   * Pins `sha` (label defaults to the short sha). No-op (returns null) for
   * blank shas or duplicates.
   */
  add(root: string, sha: string, label?: string): Bookmark | null {
    const trimmed = sha.trim();
    if (!trimmed || this.has(root, trimmed)) return null;
    const bookmark: Bookmark = {
      sha: trimmed,
      label: label?.trim() ? label.trim() : trimmed.slice(0, 7),
    };
    this.#setList(root, [...this.list(root), bookmark]);
    return bookmark;
  }

  /** Unpins `sha`; true when something was removed. */
  remove(root: string, sha: string): boolean {
    const before = this.list(root);
    const after = before.filter((b) => b.sha !== sha);
    if (after.length === before.length) return false;
    this.#setList(root, after);
    return true;
  }

  /** Flips `sha`'s bookmark; true when it is now bookmarked. */
  toggle(root: string, sha: string, label?: string): boolean {
    if (this.remove(root, sha)) return false;
    this.add(root, sha, label);
    return true;
  }

  /** Drops all state for `root` (repo closed; test helper). */
  clear(root: string): void {
    this.#hydrated.delete(root);
    this.#setList(root, []);
  }

  #setList(root: string, list: Bookmark[]): void {
    this.#byRoot[root] = list;
    this.#storage?.setItem(bookmarksKey(root), JSON.stringify(list));
  }
}

/** The application-wide bookmark store. */
export const bookmarks = new BookmarkStore();
