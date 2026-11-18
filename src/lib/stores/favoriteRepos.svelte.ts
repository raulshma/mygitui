/**
 * Favorite repositories (Svelte 5 runes) — an ordered set of repo roots the
 * user starred for quick access.
 *
 * Where railPins/headerPins pin PANELS, this store pins REPOSITORIES: the
 * quick switcher lists favorites above its recents (see
 * `buildSwitcherItems`), the tab-strip context menu stars/unstars the tab's
 * repo, the palette exposes `repos.toggle-favorite` for the active repo,
 * and — with the "Open favorite repos on launch" setting on —
 * `restoreSession` opens up to five favorites that are not already open.
 *
 * Persisted under a single `mygitui.favoriteRepos` key through the versioned
 * envelope (`$lib/state/persist`), following the railPins pattern:
 * parse-validated reads, all storage access swallowed in a try/catch
 * (privacy modes / SSR / tests may not have localStorage), newest star
 * first (star order = display order). Membership compares paths with the
 * switcher's `normalizeRepoPath` so `C:\x\repo` and `C:/x/repo` cannot
 * become two favorites; the first-seen spelling is kept.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";
import { normalizeRepoPath } from "$lib/stores/switcher.svelte";

/** Storage key for the favorite-repo list. */
export const FAVORITE_REPOS_KEY = "mygitui.favoriteRepos";

/** Favorites opened by the launch restore at most (avoid launch storms). */
export const FAVORITES_LAUNCH_CAP = 5;

/** Validates an envelope-unwrapped payload; null = malformed. */
export function parseFavoriteRepos(data: unknown): string[] | null {
  if (!Array.isArray(data)) return null;
  const favorites: string[] = [];
  const seen = new Set<string>();
  for (const entry of data) {
    if (typeof entry !== "string" || entry === "") continue;
    const key = normalizeRepoPath(entry);
    if (seen.has(key)) continue;
    seen.add(key);
    favorites.push(entry);
  }
  return favorites;
}

export class FavoriteReposStore {
  /** Favorite roots, newest star first (persisted on change). */
  favorites = $state<string[]>([]);

  #storage: StorageLike | null;

  constructor(storage: StorageLike | null = defaultStorage()) {
    this.#storage = storage;
    const loaded = loadVersioned<string[]>({
      storage,
      key: FAVORITE_REPOS_KEY,
      parse: parseFavoriteRepos,
    });
    if (loaded !== null) this.favorites = loaded;
  }

  /** True when `root` is favorited (path-normalized compare). */
  has(root: string): boolean {
    const key = normalizeRepoPath(root);
    return this.favorites.some((f) => normalizeRepoPath(f) === key);
  }

  /** Favorites a root (deduped, newest first); `false` when already there. */
  add(root: string): boolean {
    if (root === "" || this.has(root)) return false;
    this.favorites = [root, ...this.favorites];
    this.#persist();
    return true;
  }

  /** Unfavorites a root; `false` when it was not favorited. */
  remove(root: string): boolean {
    const key = normalizeRepoPath(root);
    const next = this.favorites.filter((f) => normalizeRepoPath(f) !== key);
    if (next.length === this.favorites.length) return false;
    this.favorites = next;
    this.#persist();
    return true;
  }

  /** Flips the favorite flag for a root; returns the new state. */
  toggle(root: string): boolean {
    if (this.has(root)) {
      this.remove(root);
      return false;
    }
    this.add(root);
    return true;
  }

  /** The first {@link FAVORITES_LAUNCH_CAP} favorites (launch-restore order). */
  launchList(cap: number = FAVORITES_LAUNCH_CAP): string[] {
    return this.favorites.slice(0, Math.max(0, cap));
  }

  /** Clears all favorites (tests / reset). */
  clear(): void {
    this.favorites = [];
    try {
      const storage = this.#storage as
        | (StorageLike & { removeItem?(key: string): void })
        | null;
      storage?.removeItem?.(FAVORITE_REPOS_KEY);
    } catch {
      // Storage unavailable: state still resets this session.
    }
  }

  #persist(): void {
    writeEnvelope(this.#storage, FAVORITE_REPOS_KEY, this.favorites);
  }
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** The application-wide favorite-repos store. */
export const favoriteRepos = new FavoriteReposStore();
