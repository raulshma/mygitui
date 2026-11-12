/**
 * Shared async-data machine: one {@link AsyncData} value per cache key,
 * with the stale-response guard built in.
 *
 * This is the canonical encoding for registry archetypes 3 and 4
 * (`docs/state-machines.md`): instead of co-storing `X`, `XLoading` and
 * `XErrors` maps plus a private sequence counter per key, store one
 * union per key. Invalid combos (`loading && error`, a stale response
 * overwriting a newer one) are unrepresentable, and no async callback
 * has to remember a guard.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */
import { describeError } from "$lib/errors";

/** One fetch's lifecycle: `idle → loading → ready | error`, restartable. */
export type AsyncData<T> =
  /** Never fetched (or dropped by {@link AsyncMap.reset}). */
  | { state: "idle" }
  /** A fetch is in flight; `previous` carries ready data across a refresh. */
  | { state: "loading"; previous: T | null }
  /** The latest fetch succeeded. */
  | { state: "ready"; data: T }
  /** The latest fetch failed; `previous` keeps ready data visible. */
  | { state: "error"; message: string; previous: T | null };

/** Per-key container of {@link AsyncData} entries with stale-drop semantics. */
export class AsyncMap<T> {
  #entries = $state<Record<string, AsyncData<T>>>({});
  /** Latest-start sequence per key; only the newest run may write. */
  #seqs = new Map<string, number>();
  /** When each key last landed a ready result (`ensure`'s TTL clock). */
  #readyAt = new Map<string, number>();

  /** The entry for `key`, or idle when never fetched. */
  get(key: string): AsyncData<T> {
    return this.#entries[key] ?? { state: "idle" };
  }

  /**
   * Runs `fn` as the newest fetch for `key`: flips the entry to
   * `loading` (ready data rides along as `previous`, so a refresh never
   * blanks the view) and lands the result only if no newer run has
   * started meanwhile. Resolves with the data, or `null` when the fetch
   * failed or was superseded — callers that want the failure reason read
   * {@link get} instead.
   *
   * `silent` skips the `loading` flip (background refreshes must not
   * flash the toolbar); the result still lands through the same guard.
   * `onError` observes the failure without replacing the union's error
   * state (for `console.warn`-style logging).
   */
  async run(
    key: string,
    fn: () => Promise<T>,
    opts?: { silent?: boolean; onError?: (err: unknown) => void },
  ): Promise<T | null> {
    const seq = (this.#seqs.get(key) ?? 0) + 1;
    this.#seqs.set(key, seq);
    if (!opts?.silent) {
      const entry = this.get(key);
      const previous = entry.state === "ready" ? entry.data : null;
      this.#entries = {
        ...this.#entries,
        [key]: { state: "loading", previous },
      };
    }
    try {
      const data = await fn();
      if (seq !== this.#seqs.get(key)) return null;
      this.#entries = { ...this.#entries, [key]: { state: "ready", data } };
      this.#readyAt.set(key, Date.now());
      return data;
    } catch (err) {
      opts?.onError?.(err);
      if (seq !== this.#seqs.get(key)) return null;
      // Whatever data was visible before the failure stays visible: the
      // loading flip's `previous`, or the ready/error data of a silent run.
      const entry = this.get(key);
      const previous =
        entry.state === "ready"
          ? entry.data
          : entry.state === "loading" || entry.state === "error"
            ? entry.previous
            : null;
      this.#entries = {
        ...this.#entries,
        [key]: { state: "error", message: describeError(err), previous },
      };
      return null;
    }
  }

  /** Writes a ready entry directly (store-internal cache patches). */
  set(key: string, data: T): void {
    this.#entries = { ...this.#entries, [key]: { state: "ready", data } };
  }

  /**
   * Ensure semantics (read-through cache): a `ready` entry younger than
   * `ttlMs` (or an in-flight run) resolves with the cached/fetching data
   * and starts nothing. Panel re-activations and repo switches use this
   * instead of a bare `run` so flipping between tabs doesn't respawn
   * subprocesses for data that landed seconds ago. Errors and stale
   * entries fall through to a fresh `run`.
   */
  async ensure(
    key: string,
    fn: () => Promise<T>,
    ttlMs: number,
    opts?: { onError?: (err: unknown) => void },
  ): Promise<T | null> {
    const entry = this.get(key);
    if (entry.state === "ready") {
      const readyAt = this.#readyAt.get(key);
      if (readyAt !== undefined && Date.now() - readyAt < ttlMs) return entry.data;
    }
    return this.run(key, fn, { silent: entry.state === "loading", onError: opts?.onError });
  }

  /**
   * Drops one entry (its owner closed); an in-flight run for the key is
   * invalidated so nothing lands into the deleted slot.
   */
  reset(key: string): void {
    this.#seqs.set(key, (this.#seqs.get(key) ?? 0) + 1);
    this.#readyAt.delete(key);
    const entries = { ...this.#entries };
    delete entries[key];
    this.#entries = entries;
  }

  /**
   * Drops every entry whose key starts with `prefix` (a cached subtree,
   * e.g. one run's log views), invalidating their in-flight runs too.
   */
  resetPrefix(prefix: string): void {
    const entries = { ...this.#entries };
    let changed = false;
    for (const key of Object.keys(entries)) {
      if (key.startsWith(prefix)) {
        this.#seqs.set(key, (this.#seqs.get(key) ?? 0) + 1);
        delete entries[key];
        changed = true;
      }
    }
    if (changed) this.#entries = entries;
  }
}
