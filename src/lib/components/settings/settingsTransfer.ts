/**
 * Settings export/import (batch B item 36) — bundle the known versioned
 * `mygitui.*` stores into one JSON file and apply such a file back.
 *
 * The bundle covers every store the Settings dialog owns data for (the
 * inventory below mirrors the STORAGE_KEY constants across the codebase):
 * global app settings, appearance, keybind overrides, rail pins, header
 * pins, auto-fetch cadences, recent repositories, per-root workspace
 * sessions, favorite repositories, and the per-root commit-authoring
 * extras (recent messages + wrap-ruler widths). Transient/session keys
 * (palette recents, AI chat, action runs, open-tabs session, …) are
 * deliberately out of scope.
 *
 * Export shape: a `mygitui-settings-export` wrapper carrying a format
 * `version`, an `exportedAt` timestamp, and `data` — a map of storage key
 * → envelope DATA (what the store's parser expects, already unwrapped).
 * Import validation is parse-and-drop in the strict direction: the
 * wrapper, version and per-key payloads are validated with the stores' own
 * parsers; any recognized key that fails validation aborts the whole
 * import (nothing partially applied). Unrecognized keys in `data` are
 * ignored — forward compatibility.
 *
 * Live refresh: only the global settings store exposes a write path that
 * re-reads its data (`update`); every other store reads localStorage at
 * construction. So after applying a bundle the app offers a confirmed
 * reload (`location.reload()`) — the import flow documents this and the
 * confirm dialog in SettingsDialog says exactly what happens.
 */

import { APP_SETTINGS_KEY } from "$lib/state/appSettings.svelte";
import { APPEARANCE_STORAGE_KEY } from "$lib/theme/appearance.svelte";
import {
  canonicalCombo,
  KEYBINDS_STORAGE_KEY,
} from "$lib/palette/keybinds";
import { parseRailPinData, RAIL_PINS_KEY } from "$lib/stores/railPins.svelte";
import { HEADER_PINS_KEY, parseHeaderPins } from "$lib/stores/headerPins.svelte";
import { AUTOFETCH_STORAGE_KEY } from "$lib/stores/autofetch.svelte";
import { STORAGE_KEY as RECENT_REPOS_KEY } from "$lib/entry/recentRepos";
import {
  parseStoredWorkspaceSession,
  WORKSPACE_SESSION_KEY_PREFIX,
} from "$lib/stores/workspaceSession.svelte";
import {
  FAVORITE_REPOS_KEY,
  parseFavoriteRepos,
} from "$lib/stores/favoriteRepos.svelte";
import {
  COMMIT_RECENTS_PREFIX,
  COMMIT_WIDTH_PREFIX,
  parseCommitRecents,
  parseCommitWidth,
} from "$lib/commit/commitExtras.svelte";
import { writeEnvelope, type StorageLike } from "$lib/state/persist";

/** Marker field of a settings export file (presence = "this is ours"). */
export const SETTINGS_EXPORT_MARKER = "mygitui-settings-export";

/** Format version of the export bundle (independent of store envelopes). */
export const SETTINGS_EXPORT_VERSION = 1;

/** The exported file shape. */
export interface SettingsExportFile {
  [SETTINGS_EXPORT_MARKER]: true;
  version: number;
  exportedAt: string;
  /** Storage key → envelope DATA (unwrapped; legacy plain keys verbatim). */
  data: Record<string, unknown>;
}

/** How an import writes one key back: envelope-wrapped or plain JSON. */
type WriteMode = "envelope" | "plain";

/** One registered storage key of the bundle. */
interface KeySpec {
  key: string;
  /** Prefix match instead of exact (per-root key families). */
  prefix?: boolean;
  /** What the store's own loader expects to find under this key. */
  mode: WriteMode;
  /** Parse-and-drop validation over the envelope DATA; null = invalid. */
  validate: (data: unknown) => boolean;
}

/** Sync verbs accepted by the header-pins validator (parseHeaderPins echo). */
function validHeaderPins(data: unknown): boolean {
  if (!Array.isArray(data)) return false;
  const parsed = parseHeaderPins(JSON.stringify(data));
  return parsed.length === data.length;
}

/** The bundle's key inventory (see the module doc). */
const KEY_SPECS: readonly KeySpec[] = [
  {
    key: APP_SETTINGS_KEY,
    mode: "envelope",
    validate: (data) => typeof data === "object" && data !== null,
  },
  {
    key: APPEARANCE_STORAGE_KEY,
    mode: "plain",
    validate: (data) => typeof data === "object" && data !== null,
  },
  {
    key: KEYBINDS_STORAGE_KEY,
    mode: "plain",
    validate: (data) => {
      if (typeof data !== "object" || data === null || Array.isArray(data)) {
        return false;
      }
      return Object.values(data).every(
        (combo) => typeof combo === "string" && canonicalCombo(combo) !== null,
      );
    },
  },
  {
    key: RAIL_PINS_KEY,
    mode: "envelope",
    validate: (data) => parseRailPinData(data) !== null,
  },
  {
    key: HEADER_PINS_KEY,
    mode: "envelope",
    validate: validHeaderPins,
  },
  {
    key: AUTOFETCH_STORAGE_KEY,
    mode: "envelope",
    validate: (data) => {
      if (typeof data !== "object" || data === null || Array.isArray(data)) {
        return false;
      }
      return Object.values(data).every(
        (v) => typeof v === "number" && Number.isFinite(v) && v >= 0,
      );
    },
  },
  {
    key: RECENT_REPOS_KEY,
    mode: "plain",
    validate: (data) =>
      Array.isArray(data) &&
      data.every(
        (entry) =>
          typeof entry === "object" &&
          entry !== null &&
          typeof (entry as { path?: unknown }).path === "string",
      ),
  },
  {
    key: WORKSPACE_SESSION_KEY_PREFIX,
    prefix: true,
    mode: "envelope",
    validate: (data) => parseStoredWorkspaceSession(data) !== null,
  },
  {
    key: FAVORITE_REPOS_KEY,
    mode: "envelope",
    validate: (data) => parseFavoriteRepos(data) !== null,
  },
  {
    key: COMMIT_RECENTS_PREFIX,
    prefix: true,
    mode: "envelope",
    validate: (data) => parseCommitRecents(data) !== null,
  },
  {
    key: COMMIT_WIDTH_PREFIX,
    prefix: true,
    mode: "envelope",
    validate: (data) => parseCommitWidth(data) !== null,
  },
];

/** Finds the spec owning a storage key (exact first, then prefixes). */
function specFor(key: string): KeySpec | null {
  const exact = KEY_SPECS.find((spec) => !spec.prefix && spec.key === key);
  if (exact) return exact;
  return KEY_SPECS.find((spec) => spec.prefix && key.startsWith(spec.key)) ?? null;
}

/** Reads and JSON-parses a raw stored value; null when absent/malformed. */
function readJson(storage: StorageLike | null, key: string): unknown {
  let raw: string | null;
  try {
    raw = storage?.getItem(key) ?? null;
  } catch {
    return null;
  }
  if (raw === null || raw === "") return null;
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

/** Collects the export bundle from `storage` (absent keys are skipped). */
export function collectSettingsExport(storage: StorageLike | null): SettingsExportFile {
  const data: Record<string, unknown> = {};
  const scan = (spec: KeySpec): void => {
    if (spec.prefix) {
      // Per-root family: copy every stored key under the prefix.
      const keys = storageKeys(storage);
      for (const key of keys) {
        if (!key.startsWith(spec.key)) continue;
        const value = readEnvelopeData(storage, key, spec.mode);
        if (value !== null) data[key] = value;
      }
      return;
    }
    const value = readEnvelopeData(storage, spec.key, spec.mode);
    if (value !== null) data[spec.key] = value;
  };
  for (const spec of KEY_SPECS) scan(spec);
  return {
    [SETTINGS_EXPORT_MARKER]: true,
    version: SETTINGS_EXPORT_VERSION,
    exportedAt: new Date().toISOString(),
    data,
  };
}

/** Every localStorage key (empty when storage is unavailable). */
function storageKeys(storage: StorageLike | null): string[] {
  const out: string[] = [];
  try {
    const full = storage as (StorageLike & { key?(i: number): string | null; length?: number }) | null;
    const n = full?.length ?? 0;
    for (let i = 0; i < n; i++) {
      const key = full?.key?.(i) ?? null;
      if (key !== null) out.push(key);
    }
  } catch {
    // Storage unavailable: nothing to enumerate.
  }
  return out;
}

/**
 * Reads a key the way its store would: envelope stores return the unwrapped
 * DATA (legacy plain payloads pass through as version-1 data, exactly like
 * `readEnvelope`); plain stores return the parsed value verbatim.
 */
function readEnvelopeData(
  storage: StorageLike | null,
  key: string,
  mode: WriteMode,
): unknown {
  const parsed = readJson(storage, key);
  if (parsed === null) return null;
  if (mode === "envelope") {
    return readEnvelopeDataOf(parsed);
  }
  return parsed;
}

function readEnvelopeDataOf(parsed: unknown): unknown {
  if (
    typeof parsed === "object" &&
    parsed !== null &&
    "v" in parsed &&
    typeof (parsed as { v: unknown }).v === "number" &&
    "data" in parsed
  ) {
    return (parsed as { data: unknown }).data;
  }
  // Legacy unversioned payload: reads as version-1 data (persist.ts).
  return parsed;
}

export type SettingsImportResult =
  | { ok: true; file: SettingsExportFile }
  | { ok: false; error: string };

/**
 * Structural validation of an imported file's text: JSON parses, the
 * wrapper marker and version are ours, and `data` is an object. Key-level
 * validation happens in {@link validateSettingsImport}.
 */
export function parseSettingsImport(text: string): SettingsImportResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { ok: false, error: "not valid JSON" };
  }
  if (typeof parsed !== "object" || parsed === null) {
    return { ok: false, error: "not a settings export file" };
  }
  const obj = parsed as Record<string, unknown>;
  if (obj[SETTINGS_EXPORT_MARKER] !== true) {
    return { ok: false, error: "missing the mygitui-settings-export marker" };
  }
  if (obj.version !== SETTINGS_EXPORT_VERSION) {
    return {
      ok: false,
      error: `unsupported export version ${JSON.stringify(obj.version)}`,
    };
  }
  if (typeof obj.data !== "object" || obj.data === null || Array.isArray(obj.data)) {
    return { ok: false, error: "the export carries no settings data" };
  }
  return {
    ok: true,
    file: {
      [SETTINGS_EXPORT_MARKER]: true,
      version: SETTINGS_EXPORT_VERSION,
      exportedAt: typeof obj.exportedAt === "string" ? obj.exportedAt : "",
      data: obj.data as Record<string, unknown>,
    },
  };
}

/**
 * Key-level validation: every recognized key in `file.data` must pass its
 * store's parse-and-drop check. One invalid payload fails the WHOLE import
 * (nothing partially applied); unrecognized keys are ignored (forward
 * compatibility). Returns the applied key list when everything passes.
 */
export function validateSettingsImport(
  file: SettingsExportFile,
): { ok: true; keys: string[] } | { ok: false; error: string } {
  const keys: string[] = [];
  for (const [key, value] of Object.entries(file.data)) {
    const spec = specFor(key);
    if (spec === null) continue; // unknown key: ignored, not an error
    if (!spec.validate(value)) {
      return { ok: false, error: `the stored data for “${key}” is not valid` };
    }
    keys.push(key);
  }
  return { ok: true, keys };
}

/**
 * Applies a validated bundle to `storage`: per-key REPLACE (the imported
 * value becomes the stored value), envelope stores re-wrapped, plain stores
 * written as plain JSON. Returns the applied keys. Storage failures are
 * swallowed per key (matching the stores' own writes); callers should treat
 * the returned list as what actually landed and offer a reload — only the
 * global settings store re-reads its data live.
 */
export function applySettingsImport(
  storage: StorageLike | null,
  file: SettingsExportFile,
): string[] {
  const applied: string[] = [];
  for (const [key, value] of Object.entries(file.data)) {
    const spec = specFor(key);
    if (spec === null) continue;
    try {
      if (storage === null) break;
      if (spec.mode === "envelope") {
        writeEnvelope(storage, key, value);
      } else {
        storage.setItem(key, JSON.stringify(value));
      }
      applied.push(key);
    } catch {
      // Storage full/unavailable: skip this key, keep applying the rest.
    }
  }
  return applied;
}
