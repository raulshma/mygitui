/**
 * Versioned storage envelope (registry #7, `docs/state-machines.md`).
 *
 * Every payload persisted under a `mygitui.*` key is written as
 * `{ v: <version>, data: <payload> }` so a shape change can MIGRATE
 * instead of being silently dropped by parse-and-drop. Reads accept
 * legacy unversioned payloads as version 1 (pre-envelope data keeps
 * loading), and {@link loadVersioned} runs registered upgraders until
 * the payload reaches the version the store's parser expects. Anything
 * malformed resolves `null` — the caller re-defaults, exactly like the
 * per-store parse-and-drop this generalizes.
 *
 * Adopt this when a persisted shape changes next: bump the version in
 * the store's `loadVersioned` call, add an upgrader, done — no more
 * heuristics like autofetch's legacy-key guess.
 */

/** Minimal storage surface (subset of DOM `Storage`), single-sourced. */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** The version freshly written payloads carry. */
export const ENVELOPE_VERSION = 1;

/** The on-disk shape. */
export interface Envelope<T> {
  v: number;
  data: T;
}

function isEnvelope(value: unknown): value is Envelope<unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    "v" in value &&
    typeof (value as { v: unknown }).v === "number" &&
    "data" in value
  );
}

/**
 * Reads `key` and unwraps it: `{version, data}` for an envelope; a
 * legacy unversioned payload reads as `{version: 1, data: <payload>}`.
 * `null` when the key is absent, the storage is unavailable, or the JSON
 * does not parse.
 */
export function readEnvelope(
  storage: StorageLike | null,
  key: string,
): { version: number; data: unknown } | null {
  if (storage === null) return null;
  let parsed: unknown;
  try {
    const raw = storage.getItem(key);
    if (raw === null || raw === "") return null;
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (isEnvelope(parsed)) return { version: parsed.v, data: parsed.data };
  // Legacy unversioned payload (pre-envelope writes).
  return { version: 1, data: parsed };
}

/** Writes `{ v: ENVELOPE_VERSION, data }` (storage failures swallowed). */
export function writeEnvelope(
  storage: StorageLike | null,
  key: string,
  data: unknown,
): void {
  if (storage === null) return;
  try {
    const envelope: Envelope<unknown> = { v: ENVELOPE_VERSION, data };
    storage.setItem(key, JSON.stringify(envelope));
  } catch {
    // Storage full/unavailable: the session keeps working in memory.
  }
}

/**
 * The one entry point stores should use: unwrap, migrate up to
 * `toVersion` (default {@link ENVELOPE_VERSION}), then validate with
 * `parse`. `migrations` is keyed by FROM version; each upgrader returns
 * the payload one version newer. A version with no registered upgrader
 * (a payload written by a newer build) resolves `null` — the caller
 * re-defaults rather than acting on a shape it cannot know.
 */
export function loadVersioned<T>(opts: {
  storage: StorageLike | null;
  key: string;
  parse: (data: unknown) => T | null;
  toVersion?: number;
  migrations?: Record<number, (data: unknown) => unknown>;
}): T | null {
  const entry = readEnvelope(opts.storage, opts.key);
  if (entry === null) return null;
  const toVersion = opts.toVersion ?? ENVELOPE_VERSION;
  let { version, data } = entry;
  while (version < toVersion) {
    const upgrade = opts.migrations?.[version];
    if (upgrade === undefined) return null;
    data = upgrade(data);
    version += 1;
  }
  if (version !== toVersion) return null;
  return opts.parse(data);
}
