/**
 * User-defined "Open with" actions (Svelte 5 runes).
 *
 * Complements the machine-detected editors (`editors.rs` backend table) with
 * commands the user defines themselves — any editor/tool command line, where
 * `{path}` is substituted with the repository root at launch. A template
 * without `{path}` gets the root appended as the final (quoted) argument —
 * the backend runs the line through the platform shell (`cmd /C` on Windows,
 * `sh -c` elsewhere), so the quotes protect paths with spaces. With `{path}`
 * present, quoting is the template author's job.
 *
 * Examples: `code {path}` / `kitty {path}` / `C:\Tools\gvim.exe {path}`.
 *
 * Persisted under `mygitui.openwith.custom` with the versioned envelope
 * (`$lib/state/persist`), parse-and-drop like every other store: a malformed
 * payload re-defaults to an empty list, never throws. Nothing is persisted
 * backend-side — the full spec is sent per launch (`open_with_custom`).
 *
 * Ids are `crypto.randomUUID()` (with a fallback when crypto is
 * unavailable). They are stored bare here; the header menu namespaces them
 * with {@link CUSTOM_ID_PREFIX} (`custom:<id>`) when writing the
 * `mygitui.openwith.last` last-used key, so they can never collide with a
 * detected app id — resolve through {@link CustomAppsStore.byId} after
 * stripping the prefix (see `isCustomId`/`fromMenuId`).
 */

import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";

/** localStorage key for the custom-app list envelope. */
export const CUSTOM_APPS_KEY = "mygitui.openwith.custom";

/** One user-defined open-with action. */
export interface CustomApp {
  id: string;
  /** Menu/display name (e.g. "Neovim (kitty)"). */
  name: string;
  /**
   * Command template run through the platform shell; `{path}` becomes the
   * repository root, or the root is appended (quoted) when absent.
   */
  command: string;
  /** Reserved for a future icon picker; unused by the UI today. */
  icon?: string;
}

/**
 * Namespace prefix used in persisted last-used ids (`mygitui.openwith.last`)
 * for custom apps, so they never collide with a detected app id.
 */
export const CUSTOM_ID_PREFIX = "custom:";

/** True when `id` is a namespaced custom-app id (`custom:<uuid>`). */
export function isCustomId(id: string): boolean {
  return id.startsWith(CUSTOM_ID_PREFIX);
}

/** Strips {@link CUSTOM_ID_PREFIX} (pass-through for detected ids). */
export function fromMenuId(id: string): string {
  return isCustomId(id) ? id.slice(CUSTOM_ID_PREFIX.length) : id;
}

/** Parse-and-drop loader: malformed entries are dropped, not fatal. */
export function parseStored(data: unknown): CustomApp[] {
  if (!Array.isArray(data)) return [];
  const out: CustomApp[] = [];
  for (const raw of data) {
    if (typeof raw !== "object" || raw === null) continue;
    const r = raw as Partial<CustomApp>;
    if (typeof r.id !== "string" || r.id === "") continue;
    if (typeof r.name !== "string" || r.name.trim() === "") continue;
    if (typeof r.command !== "string" || r.command.trim() === "") continue;
    const app: CustomApp = { id: r.id, name: r.name.trim(), command: r.command.trim() };
    if (typeof r.icon === "string" && r.icon !== "") app.icon = r.icon;
    out.push(app);
  }
  return out;
}

/** Returns `localStorage` when available, else `null` (never throws). */
function safeStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Minted ids: `crypto.randomUUID()` with a fallback for old runtimes. */
function makeId(): string {
  return (
    globalThis.crypto?.randomUUID?.() ??
    `custom-${Date.now()}-${Math.random().toString(16).slice(2)}`
  );
}

/** The write shape of {@link CustomAppsStore.add}/{@link CustomAppsStore.update}. */
export interface CustomAppInput {
  name: string;
  command: string;
  icon?: string;
}

export class CustomAppsStore {
  #storage: StorageLike | null;
  #apps = $state<CustomApp[]>([]);

  constructor(storage?: StorageLike | null) {
    this.#storage = storage !== undefined ? storage : safeStorage();
    const stored = loadVersioned<CustomApp[]>({
      storage: this.#storage,
      key: CUSTOM_APPS_KEY,
      parse: parseStored,
    });
    if (stored !== null) this.#apps = stored;
  }

  /** The current custom apps (reactive). */
  get apps(): CustomApp[] {
    return this.#apps;
  }

  /** Same list as {@link CustomAppsStore.apps} (explicit CRUD alias). */
  list(): CustomApp[] {
    return this.#apps;
  }

  /** Looks up one app by its bare (unprefixed) id. */
  byId(id: string): CustomApp | undefined {
    return this.#apps.find((a) => a.id === id);
  }

  /** Appends a new app (ids are minted here) and persists. */
  add(input: CustomAppInput): CustomApp {
    const app: CustomApp = {
      id: makeId(),
      name: input.name.trim(),
      command: input.command.trim(),
    };
    if (app.name === "" || app.command === "") {
      throw new Error("custom app needs a name and a command");
    }
    if (typeof input.icon === "string" && input.icon !== "") app.icon = input.icon;
    this.#apps = [...this.#apps, app];
    this.#persist();
    return app;
  }

  /** Patches an existing app and persists; `undefined` for an unknown id. */
  update(id: string, patch: Partial<CustomAppInput>): CustomApp | undefined {
    const index = this.#apps.findIndex((a) => a.id === id);
    if (index < 0) return undefined;
    const next: CustomApp = {
      ...this.#apps[index],
      name: patch.name !== undefined ? patch.name.trim() : this.#apps[index].name,
      command: patch.command !== undefined ? patch.command.trim() : this.#apps[index].command,
    };
    if (next.name === "" || next.command === "") {
      throw new Error("custom app needs a name and a command");
    }
    if (patch.icon !== undefined) {
      if (patch.icon === "") delete next.icon;
      else next.icon = patch.icon;
    }
    this.#apps = this.#apps.map((a, i) => (i === index ? next : a));
    this.#persist();
    return next;
  }

  /** Removes an app; `true` when it existed. */
  remove(id: string): boolean {
    const before = this.#apps.length;
    this.#apps = this.#apps.filter((a) => a.id !== id);
    if (this.#apps.length === before) return false;
    this.#persist();
    return true;
  }

  /** Persists the list (best-effort; storage failures are swallowed). */
  #persist(): void {
    writeEnvelope(this.#storage, CUSTOM_APPS_KEY, this.#apps);
  }
}

/** Shared app-wide instance (the header menu and the manager section bind to it). */
export const customApps = new CustomAppsStore();
