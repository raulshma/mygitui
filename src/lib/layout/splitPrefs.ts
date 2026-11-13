/**
 * Persisted split-ratio preferences for panel-internal splitters (history
 * list ↔ commit detail, detail meta ↔ diff). Global UX preferences — one
 * storage key per splitter, not per repo — following the layout store's
 * conventions: `mygitui.*` key prefix, 150 ms debounced writes, clamped
 * reads, and all storage access swallowed in a try/catch (privacy modes /
 * SSR / tests may not have localStorage).
 */
import { clampRatio } from "./layoutModel";

const KEY_PREFIX = "mygitui.split.";
const DEBOUNCE_MS = 150;

/** One pending write per key (drag streams writes; only the last lands). */
const pending = new Map<
  string,
  { timer: ReturnType<typeof setTimeout>; ratio: number }
>();

/** Writes one ratio; all storage access swallowed (privacy modes / SSR). */
function persist(key: string, ratio: number): void {
  try {
    globalThis.localStorage?.setItem(KEY_PREFIX + key, String(clampRatio(ratio)));
  } catch {
    // Storage unavailable: resize still works this session.
  }
}

/** Reads a persisted ratio; `fallback` when missing/malformed/storage off. */
export function readSplitRatio(key: string, fallback: number): number {
  try {
    const raw = globalThis.localStorage?.getItem(KEY_PREFIX + key);
    if (raw === null) return fallback;
    const parsed = Number(raw);
    return Number.isFinite(parsed) ? clampRatio(parsed) : fallback;
  } catch {
    return fallback;
  }
}

/** Records a ratio (debounced); clamps through the same range as drags. */
export function writeSplitRatio(key: string, ratio: number): void {
  const existing = pending.get(key);
  if (existing) clearTimeout(existing.timer);
  pending.set(key, {
    timer: setTimeout(() => {
      pending.delete(key);
      persist(key, ratio);
    }, DEBOUNCE_MS),
    ratio,
  });
}

/**
 * Immediate synchronous flush of all pending writes. Called on `pagehide`
 * (see `App.svelte`): closing/reloading the window inside the debounce
 * window must not drop the last ratio.
 */
export function flushSplits(): void {
  for (const [key, entry] of [...pending]) {
    clearTimeout(entry.timer);
    pending.delete(key);
    persist(key, entry.ratio);
  }
}
