/**
 * Per-repo conflicts cache (Svelte 5 runes).
 *
 * The RepoView conflict banner polls `conflicts(repoId)` while a merge /
 * rebase / sequencer operation is in progress; this store caches the last
 * result per repo so the banner and the ConflictEditor read the same list
 * without refetching, and `clear` drops it the moment the operation is
 * resolved or aborted.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client` (mocked in
 * tests via `vi.mock`).
 */

import { conflicts } from "$lib/ipc/client";
import type { ConflictFile, RepoId } from "$lib/ipc/types";

/** Cached state for one repository. */
export interface ConflictsEntry {
  files: ConflictFile[];
  loading: boolean;
  error: string | null;
}

const EMPTY: ConflictsEntry = { files: [], loading: false, error: null };

export class ConflictsStore {
  #byRepo: Record<string, ConflictsEntry> = $state({});
  /** In-flight `load` promises, keyed by repo (dedupes concurrent calls). */
  #inflight = new Map<string, Promise<ConflictFile[]>>();

  /** The raw entry for `repoId` (EMPTY when never loaded). */
  entry(repoId: RepoId): ConflictsEntry {
    return this.#byRepo[repoId] ?? EMPTY;
  }

  /** Cached conflict files for `repoId` (empty when unknown). */
  files(repoId: RepoId): ConflictFile[] {
    return this.entry(repoId).files;
  }

  /** Cached conflict count for `repoId` (0 when unknown). */
  count(repoId: RepoId): number {
    return this.files(repoId).length;
  }

  isLoading(repoId: RepoId): boolean {
    return this.entry(repoId).loading;
  }

  error(repoId: RepoId): string | null {
    return this.entry(repoId).error;
  }

  /**
   * Fetches (or joins an in-flight fetch of) the conflict list for `repoId`
   * and caches it. Never throws: failures are stored as `error` and resolve
   * to the previous cached list (empty when none).
   */
  load(repoId: RepoId): Promise<ConflictFile[]> {
    const pending = this.#inflight.get(repoId);
    if (pending) return pending;

    this.#set(repoId, { ...this.entry(repoId), loading: true, error: null });
    const promise = conflicts(repoId)
      .then((files) => {
        this.#set(repoId, { files, loading: false, error: null });
        return files;
      })
      .catch((err: unknown) => {
        const message = err instanceof Error ? err.message : String(err);
        this.#set(repoId, {
          files: this.entry(repoId).files,
          loading: false,
          error: message,
        });
        return this.entry(repoId).files;
      })
      .finally(() => {
        this.#inflight.delete(repoId);
      });
    this.#inflight.set(repoId, promise);
    return promise;
  }

  /** Drops the cached list for `repoId` (after resolve / abort / clean status). */
  clear(repoId: RepoId): void {
    delete this.#byRepo[repoId];
  }

  /** Drops everything (test helper / repo close). */
  clearAll(): void {
    this.#byRepo = {};
  }

  #set(repoId: RepoId, entry: ConflictsEntry): void {
    this.#byRepo[repoId] = entry;
  }
}

/** The application-wide conflicts cache. */
export const conflictsStore = new ConflictsStore();
