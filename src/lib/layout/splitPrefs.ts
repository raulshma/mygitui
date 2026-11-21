/**
 * Persisted preferences for panel-internal splitters (history list ↔
 * commit detail, detail meta ↔ diff) and the collapsible overlay panes
 * they host. Global UX preferences — one storage key per splitter, not per
 * repo — following the layout store's conventions: `mygitui.*` key prefix,
 * 150 ms debounced writes, clamped reads, and all storage access swallowed
 * in a try/catch (privacy modes / SSR / tests may not have localStorage).
 *
 * Payload (versioned envelope — plan §4.2): `{ v: 2, data: { ratio?,
 * modes? } }`. Version 1 was the bare ratio written unversioned (legacy
 * `writeSplitRatio` output; `readEnvelope` reads it as v1) — the v1→v2
 * upgrader wraps it as `{ ratio }` so old saved ratios survive intact.
 * `modes` is the per-width-mode open/closed dimension overlay drawers
 * persist alongside the ratio (Wide/Medium/Narrow/Stacked, see
 * `widthMode.svelte.ts`); absent entries mean "default (open)". The
 * version target is scoped to this key family — the shared
 * `ENVELOPE_VERSION` in `$lib/state/persist` stays 1, and other stores'
 * v1 payloads must keep loading without an upgrader.
 */
import { clampRatio } from "./layoutModel";
import { WIDTH_MODES, type WidthMode } from "./widthMode.svelte";
import { loadVersioned } from "$lib/state/persist";

const KEY_PREFIX = "mygitui.split.";
const DEBOUNCE_MS = 150;

/** Storage payload version for split prefs (see `$lib/state/persist`). */
export const SPLIT_PREFS_VERSION = 2;

/** One splitter key's persisted data (envelope v2). */
export interface SplitPrefData {
  /** Splitter ratio, clamped. Absent until the first drag writes one. */
  ratio?: number;
  /** Per-width-mode open/closed state for collapsible overlay panes. */
  modes?: Partial<Record<WidthMode, boolean>>;
}

/** One pending write per key (drag streams writes; only the last lands). */
const pending = new Map<
  string,
  { timer: ReturnType<typeof setTimeout>; data: SplitPrefData }
>();

/** Hydrated reads (merged writes must not clobber un-persisted state). */
const cache = new Map<string, SplitPrefData>();

/** v1 payloads: the bare clamped ratio (written unversioned). */
function migrateV1(data: unknown): unknown {
  if (typeof data === "number" && Number.isFinite(data)) {
    return { ratio: data };
  }
  if (typeof data === "string") {
    const ratio = Number(data);
    if (Number.isFinite(ratio)) return { ratio };
  }
  return data; // malformed → parse() drops it (caller re-defaults)
}

/** Parses a v2 payload, dropping malformed fields; `null` when empty. */
function parseSplitData(data: unknown): SplitPrefData | null {
  if (typeof data !== "object" || data === null) return null;
  const v = data as Record<string, unknown>;
  const out: SplitPrefData = {};
  if (typeof v.ratio === "number" && Number.isFinite(v.ratio)) {
    out.ratio = clampRatio(v.ratio);
  }
  if (typeof v.modes === "object" && v.modes !== null) {
    const modes: Partial<Record<WidthMode, boolean>> = {};
    const raw = v.modes as Record<string, unknown>;
    for (const mode of WIDTH_MODES) {
      if (typeof raw[mode] === "boolean") modes[mode] = raw[mode] as boolean;
    }
    if (Object.keys(modes).length > 0) out.modes = modes;
  }
  return out.ratio !== undefined || out.modes !== undefined ? out : null;
}

/**
 * Reads a key's data through the envelope (migrating v1 → v2 in memory);
 * `null` when missing/malformed. Reads hydrate the cache so a merge write
 * never drops a just-read mode state.
 */
function loadData(key: string): SplitPrefData | null {
  const cached = cache.get(key);
  if (cached) return cached;
  let loaded: SplitPrefData | null = null;
  try {
    loaded = loadVersioned<SplitPrefData>({
      storage: globalThis.localStorage ?? null,
      key: KEY_PREFIX + key,
      toVersion: SPLIT_PREFS_VERSION,
      migrations: { 1: migrateV1 },
      parse: parseSplitData,
    });
  } catch {
    loaded = null;
  }
  if (loaded) cache.set(key, loaded);
  return loaded;
}

/** The latest known data for a key (pending write wins over the cache). */
function currentData(key: string): SplitPrefData | null {
  return pending.get(key)?.data ?? loadData(key);
}

/** Writes one data snapshot; all storage access swallowed (privacy modes). */
function persist(key: string, data: SplitPrefData): void {
  try {
    globalThis.localStorage?.setItem(
      KEY_PREFIX + key,
      JSON.stringify({ v: SPLIT_PREFS_VERSION, data }),
    );
  } catch {
    // Storage unavailable: resize still works this session.
  }
}

/** Schedules a debounced write of `data` (replacing any pending one). */
function schedule(key: string, data: SplitPrefData): void {
  cache.set(key, data);
  const existing = pending.get(key);
  if (existing) clearTimeout(existing.timer);
  pending.set(key, {
    timer: setTimeout(() => {
      pending.delete(key);
      persist(key, data);
    }, DEBOUNCE_MS),
    data,
  });
}

/** Reads a persisted ratio; `fallback` when missing/malformed/storage off. */
export function readSplitRatio(key: string, fallback: number): number {
  try {
    const ratio = loadData(key)?.ratio;
    return typeof ratio === "number" ? ratio : fallback;
  } catch {
    return fallback;
  }
}

/**
 * Records a ratio (debounced; clamped through the same range as drags),
 * preserving any per-mode state already stored under the key.
 */
export function writeSplitRatio(key: string, ratio: number): void {
  const base = currentData(key);
  schedule(key, { ...base, ratio: clampRatio(ratio) });
}

/**
 * Reads a mode's open/closed state for a collapsible overlay pane;
 * `fallback` when nothing was persisted for that mode (fresh installs and
 * modes the user never toggled — the pane's own default applies).
 */
export function readSplitModeOpen(
  key: string,
  mode: WidthMode,
  fallback: boolean,
): boolean {
  try {
    const open = loadData(key)?.modes?.[mode];
    return typeof open === "boolean" ? open : fallback;
  } catch {
    return fallback;
  }
}

/**
 * Records a mode's open/closed state (debounced), preserving the stored
 * ratio and the other modes' entries.
 */
export function writeSplitModeOpen(
  key: string,
  mode: WidthMode,
  open: boolean,
): void {
  const base = currentData(key);
  const modes: Partial<Record<WidthMode, boolean>> = { ...base?.modes, [mode]: open };
  schedule(key, { ...base, modes });
}

/**
 * Immediate synchronous flush of all pending writes. Called on `pagehide`
 * (see `App.svelte`): closing/reloading the window inside the debounce
 * window must not drop the last change.
 */
export function flushSplits(): void {
  for (const [key, entry] of [...pending]) {
    clearTimeout(entry.timer);
    pending.delete(key);
    persist(key, entry.data);
  }
}
