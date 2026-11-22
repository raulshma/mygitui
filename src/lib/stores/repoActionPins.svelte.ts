/**
 * Persisted repo-action pins (Svelte 5 runes).
 *
 * Entries of the header's "More repository actions" menu (layout preset,
 * open-with, bisect, clean, …) can be pinned so they get their own
 * always-visible button on the header bar — the railPins idiom for the
 * header row (the sync verbs' fetch/pull/push pins live in
 * `headerPins.svelte.ts`). The pin list is app-wide (the header exists in
 * every repo workspace) and persisted under a single `mygitui.*` key,
 * following the layout store's storage conventions: parse-validated reads
 * and all storage access swallowed in a try/catch (privacy modes / SSR /
 * tests may not have localStorage). Written through the versioned
 * envelope (`$lib/state/persist`) so a shape change can migrate.
 *
 * Ids are validated only structurally (unique, capped strings) — the
 * rendered buttons come from the live `headerActions` prop intersected
 * with this list, so a stale persisted id is simply not rendered rather
 * than an error.
 */

import { loadVersioned, writeEnvelope } from "$lib/state/persist";

/** Storage key for the action pin list. */
export const REPO_ACTION_PINS_KEY = "mygitui.repoActionPins";

/** Caps a runaway pin list (the header bar is one row — 12 buttons max). */
const MAX_PINS = 12;

/** Validates a pin payload (envelope-unwrapped or legacy); null = malformed. */
export function parseRepoActionPinData(data: unknown): string[] | null {
  if (!Array.isArray(data)) return null;
  const pins: string[] = [];
  for (const entry of data) {
    if (typeof entry === "string" && entry !== "" && !pins.includes(entry)) {
      pins.push(entry);
    }
  }
  return pins.slice(0, MAX_PINS);
}

/** Parses persisted pins JSON, dropping unknown/duplicate ids (never throws). */
export function parseRepoActionPins(raw: string | null): string[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  // Legacy raw payloads predate the envelope; envelope data unwraps to the
  // same list shape.
  const data =
    typeof parsed === "object" &&
    parsed !== null &&
    "v" in parsed &&
    "data" in parsed
      ? (parsed as { data: unknown }).data
      : parsed;
  return parseRepoActionPinData(data) ?? [];
}

/** Reads the persisted pin list (empty when none/unavailable). */
export function loadRepoActionPins(): string[] {
  try {
    return (
      loadVersioned({
        storage: globalThis.localStorage ?? null,
        key: REPO_ACTION_PINS_KEY,
        parse: parseRepoActionPinData,
      }) ?? []
    );
  } catch {
    return [];
  }
}

function saveRepoActionPins(pins: string[]): void {
  try {
    writeEnvelope(globalThis.localStorage ?? null, REPO_ACTION_PINS_KEY, pins);
  } catch {
    // Storage full/unavailable: pins still work this session.
  }
}

export class RepoActionPinsStore {
  /** Pinned action ids in pin order (render order = pin order). */
  pins = $state<string[]>(loadRepoActionPins());

  has(id: string): boolean {
    return this.pins.includes(id);
  }

  /** Adds a pin (no-op for duplicates; the cap drops the oldest pin). */
  pin(id: string): void {
    if (id === "" || this.pins.includes(id)) return;
    this.pins = [...this.pins, id].slice(-MAX_PINS);
    saveRepoActionPins(this.pins);
  }

  /** Removes a pin. */
  unpin(id: string): void {
    if (!this.pins.includes(id)) return;
    this.pins = this.pins.filter((p) => p !== id);
    saveRepoActionPins(this.pins);
  }

  toggle(id: string): void {
    if (this.pins.includes(id)) this.unpin(id);
    else this.pin(id);
  }

  /** Clears all pins (tests / reset). */
  clear(): void {
    this.pins = [];
    try {
      globalThis.localStorage?.removeItem(REPO_ACTION_PINS_KEY);
    } catch {
      // Storage unavailable: state still resets this session.
    }
  }
}

/** The application-wide repo-action pin store. */
export const repoActionPins = new RepoActionPinsStore();
