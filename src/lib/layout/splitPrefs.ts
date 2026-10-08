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

/** One pending timer per key (drag streams writes; only the last lands). */
const pending = new Map<string, ReturnType<typeof setTimeout>>();

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
  const timer = pending.get(key);
  if (timer) clearTimeout(timer);
  pending.set(
    key,
    setTimeout(() => {
      pending.delete(key);
      try {
        globalThis.localStorage?.setItem(KEY_PREFIX + key, String(clampRatio(ratio)));
      } catch {
        // Storage unavailable: resize still works this session.
      }
    }, DEBOUNCE_MS),
  );
}
