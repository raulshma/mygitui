/**
 * Shared per-repo refs cache — ONE `branches`/`remotes` scan per burst.
 *
 * Five surfaces (BranchPanel, RemotePanel, WorktreePanel, ReflogPanel and
 * HistoryView's branch-scope dropdown) each used to call `branches()`
 * (and `remotes()`) independently on mount/repo switch. Both commands are
 * a full ref walk — plus a per-branch upstream ahead/behind for
 * `branches` — under the repo mutex, so a repo open serialized up to five
 * duplicate scans backend-side. This module gives those two reads ONE
 * home: in-flight calls share a single promise (simultaneous panel mounts
 * → one IPC call), and landed results are reused for a short
 * {@link REFS_CACHE_TTL_MS} window so staggered loads (tab flips, popout
 * mounts) don't rescan either. Panel-SPECIFIC lists (stashes, reflog,
 * worktrees) stay uncached — only the two cross-panel reads live here.
 *
 * Panels keep their own local `$state` lists + stale-response guards and
 * route their reads through {@link branchesFor}/{@link remotesFor}; the
 * returned arrays are SHARED between panels and must be treated as
 * read-only (copy before mutating).
 *
 * Invalidation (refs moved):
 *   - significant `repo-changed` events (see {@link isSignificant}) drop
 *     the affected repo's slots via ONE module-lifetime subscription
 *     (the ops.svelte pattern: registration idempotent, the subscription
 *     lifecycle owned by `$lib/events/repoEvents`);
 *   - panels call {@link invalidateRefs} after their OWN mutations — the
 *     watcher event for those may still be in flight/batched, and a bare
 *     reload would otherwise hit the still-fresh TTL slot and re-show the
 *     pre-mutation list.
 *
 * Lives in a `.svelte.ts` module for store-suite parity; it holds no
 * `$state` (the cache is promise-shaped — panels own the reactive lists).
 * Backend access goes exclusively through `$lib/ipc/client` (mocked with
 * `vi.mock` in tests).
 */

import { branches, remotes } from "$lib/ipc/client";
import type { BranchInfo, RemoteInfo, RepoId } from "$lib/ipc/types";
import { isSignificant, onRepoChange } from "$lib/events/repoEvents";

/**
 * How long a landed branch/remote list is served without a rescan. 15 s
 * covers a repo open's burst of panel mounts plus quick tab flips, while
 * every actual ref move bypasses it anyway (significant repo-change drops
 * the slot; panel mutations invalidate explicitly) — the TTL only bridges
 * windows where nothing could have changed. Longer windows would start
 * masking non-significant ref activity (e.g. a branch created without a
 * HEAD move in a surface that forgot to invalidate).
 */
export const REFS_CACHE_TTL_MS = 15_000;

/**
 * One cached read: the shared promise plus its expiry. In-flight slots
 * hold `Number.POSITIVE_INFINITY` so callers joining mid-flight always
 * share (never start a racing scan); the landing handler re-arms the
 * expiry at LANDING time — the TTL measures data freshness, not how long
 * the request took.
 */
interface RefsSlot<T> {
  promise: Promise<T>;
  expiresAt: number;
}

export class RefsCache {
  #branches = new Map<RepoId, RefsSlot<BranchInfo[]>>();
  #remotes = new Map<RepoId, RefsSlot<RemoteInfo[]>>();
  #unregister: (() => void) | null = null;

  /**
   * Subscribes the invalidation listener (idempotent; the subscription
   * lifecycle lives in `$lib/events/repoEvents`). Started lazily by the
   * first {@link branchesFor}/{@link remotesFor} — importing the module
   * alone stays side-effect-free — and then lives for the module's
   * lifetime, exactly like `OpStore.startOpsEvents`.
   */
  startEvents(): void {
    if (this.#unregister !== null) return;
    this.#unregister = onRepoChange({
      // Index-only churn cannot move refs; drop the slots only when the
      // commit-graph-relevant parts of the repo changed.
      filter: isSignificant,
      handle: (change) => this.invalidate(change.repo_id),
    });
  }

  /**
   * Cached `branches` for the repo: shares an in-flight call, reuses a
   * landed list inside the TTL, refetches otherwise (and after every
   * invalidation). Rejections are never cached — a failed scan retries on
   * the next caller.
   */
  async branchesFor(repoId: RepoId): Promise<BranchInfo[]> {
    this.startEvents();
    return this.#slotted(this.#branches, repoId, () => branches(repoId));
  }

  /** Cached `remotes` for the repo ({@link branchesFor} semantics). */
  async remotesFor(repoId: RepoId): Promise<RemoteInfo[]> {
    this.startEvents();
    return this.#slotted(this.#remotes, repoId, () => remotes(repoId));
  }

  /**
   * Read-through slot machine shared by both lists. `#slotted` itself is
   * not a mutation path — the maps only ever swap whole slots.
   */
  async #slotted<T>(
    map: Map<RepoId, RefsSlot<T>>,
    repoId: RepoId,
    fetch: () => Promise<T>,
  ): Promise<T> {
    const cached = map.get(repoId);
    if (cached && cached.expiresAt > Date.now()) return cached.promise;
    const slot: RefsSlot<T> = {
      promise: fetch(),
      // In-flight = always joinable; armed at landing below.
      expiresAt: Number.POSITIVE_INFINITY,
    };
    map.set(repoId, slot);
    try {
      const data = await slot.promise;
      // An invalidation (or newer fetch) replaced/dropped this slot while
      // it was in flight: hand the data to THIS caller, but don't cache
      // it — post-invalidation results must not re-arm a TTL.
      if (map.get(repoId) === slot) {
        slot.expiresAt = Date.now() + REFS_CACHE_TTL_MS;
      }
      return data;
    } catch (err) {
      if (map.get(repoId) === slot) map.delete(repoId);
      throw err;
    }
  }

  /** Drops the repo's cached branch and remote lists (refs/config moved). */
  invalidate(repoId: RepoId): void {
    this.#branches.delete(repoId);
    this.#remotes.delete(repoId);
  }

  /** Deregisters and clears (test helper / teardown). */
  stop(): void {
    this.#unregister?.();
    this.#unregister = null;
    this.#branches.clear();
    this.#remotes.clear();
  }
}

/** The application-wide refs cache. */
export const refsCache = new RefsCache();

// Standalone function API over the singleton (what components import). --

export function branchesFor(repoId: RepoId): Promise<BranchInfo[]> {
  return refsCache.branchesFor(repoId);
}

export function remotesFor(repoId: RepoId): Promise<RemoteInfo[]> {
  return refsCache.remotesFor(repoId);
}

/** Explicit post-mutation invalidation (see the module header). */
export function invalidateRefs(repoId: RepoId): void {
  refsCache.invalidate(repoId);
}
