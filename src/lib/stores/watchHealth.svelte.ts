/**
 * Watch-health store (Svelte 5 runes) — watcher backend state per open
 * repository, from the backend's `repo-watch-state` events (health
 * transitions only: poll fallback engaged, upgrade back to recommended,
 * death, revival on reopen).
 *
 * `start()` subscribes once through the shared event module (lazy: nothing
 * is subscribed until a surface asks for it — the StatusBar calls it on
 * mount; tests drive `apply` directly). Every event replaces the repo's
 * entry (`{alive, mode, since}` — `since` is the local receive time the
 * status bar's tooltip formats). Repos the backend never reported stay
 * absent: healthy + recommended is the default state and emits nothing, so
 * absence means "nothing to show".
 *
 * Closed repos evict through the per-repo eviction registry
 * (`$lib/state/repoEvictors`) — closeTab is the single choke point.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/events/repoEvents`.
 */

import { onRepoWatchState } from "$lib/events/repoEvents";
import type { RepoWatchStateEvent, WatchMode } from "$lib/ipc/types";
import { registerRepoEvictor } from "$lib/state/repoEvictors";

/** One repo's watcher health, as last reported by the backend. */
export interface WatchHealth {
  /** False = even the poll fallback failed; the repo no longer refreshes. */
  alive: boolean;
  /** The watcher backend (`recommended` | `poll` | `dead`). */
  mode: WatchMode;
  /** Local receive time of the last event (ms epoch). */
  since: number;
}

/** Which status-bar chip a watch state warrants (`null` = none — keep the bar calm). */
export type WatchChipKind = "poll" | "dead" | null;

/**
 * Chip visibility rule (pure, tested): healthy + recommended shows nothing;
 * the poll fallback shows the muted chip; a dead watcher shows the danger
 * chip with the reopen action.
 */
export function watchChipKind(state: WatchHealth | null): WatchChipKind {
  if (state === null) return null;
  if (!state.alive || state.mode === "dead") return "dead";
  if (state.mode === "poll") return "poll";
  return null;
}

export class WatchHealthStore {
  /** repoId → last reported watch state (absent = nothing reported). */
  states: Record<string, WatchHealth> = $state({});

  #unregister: (() => void) | null = null;

  /**
   * Subscribes to `repo-watch-state` (once; idempotent). Lazy by design:
   * the first surface that wants the state starts the transport.
   */
  start(): void {
    if (this.#unregister !== null) return;
    this.#unregister = onRepoWatchState({
      filter: () => true,
      handle: (event) => this.apply(event),
    });
  }

  /** The repo's watch state, or `null` when the backend reported nothing. */
  state(repoId: string): WatchHealth | null {
    return this.states[repoId] ?? null;
  }

  /** Applies one event (public for tests; `start` routes through here). */
  apply(event: RepoWatchStateEvent): void {
    this.states = {
      ...this.states,
      [event.repo_id]: { alive: event.alive, mode: event.mode, since: Date.now() },
    };
  }

  /** Evicts a closed repo's entry (the per-repo eviction contract). */
  clear(repoId: string): void {
    if (!(repoId in this.states)) return;
    const next = { ...this.states };
    delete next[repoId];
    this.states = next;
  }

  /** Deregisters and clears everything (test helper / teardown). */
  stop(): void {
    this.#unregister?.();
    this.#unregister = null;
    this.states = {};
  }
}

/** The application-wide watch-health store. */
export const watchHealth = new WatchHealthStore();

// Caches per-repo state: registered so closeTab (the single choke point)
// drops the closed repo's entry.
registerRepoEvictor((repoId) => watchHealth.clear(repoId));

/** Starts the singleton's event subscription (idempotent). */
export function startWatchHealth(): void {
  watchHealth.start();
}
