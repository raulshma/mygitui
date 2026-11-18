/**
 * Per-repo eviction registry — one fan-out point for "repo closed, drop
 * its cached state".
 *
 * Contract: every store caching per-repo state registers here; closeTab
 * (`$lib/stores/tabs.svelte.ts`) is the single choke point that runs the
 * registered evictors before its own cleanup. New per-repo caches register
 * an evictor instead of growing closeTab's call list — the registry exists
 * so a forgotten eviction cannot leak a closed repo's state into the next
 * open of the same path.
 *
 * Evictors receive the closing tab's `repoId` (the id closeTab owns). Stores
 * keyed by repo id (conflicts/stats/projects caches) register their `clear`
 * directly; root-keyed stores (workspace/agent sessions) stay direct calls
 * in closeTab because only the closing tab knows the repoId → root mapping.
 *
 * Plain data, no runes — unit-testable without a component. `resetForTests`
 * gives suites a clean registry (the Set is module state).
 */

/** One eviction callback: drops everything cached for `repoId`. */
export type RepoEvictor = (repoId: string) => void;

const evictors = new Set<RepoEvictor>();

/**
 * Registers an evictor and returns its unregister function (safe to call
 * more than once; unregistering an unknown callback is a no-op).
 */
export function registerRepoEvictor(evictor: RepoEvictor): () => void {
  evictors.add(evictor);
  return () => {
    evictors.delete(evictor);
  };
}

/**
 * Runs every registered evictor for `repoId`, insertion order. One throwing
 * evictor cannot starve the others (per-evictor catch, like the event
 * fan-out) — closeTab must always get through its cleanup.
 */
export function runRepoEvictors(repoId: string): void {
  for (const evictor of evictors) {
    try {
      evictor(repoId);
    } catch (err) {
      console.error(`[evictors] eviction failed for ${repoId}:`, err);
    }
  }
}

/** Test teardown: drops every registration. */
export function resetRepoEvictors(): void {
  evictors.clear();
}
