/**
 * Global app settings (Svelte 5 runes).
 *
 * The coarse switches the Settings dialog manages that no feature store
 * owns by itself — checkpointing before destructive git mutations, the
 * terminal's font size, the default auto-fetch cadence for newly opened
 * repos. Persisted under `mygitui.settings` with the versioned envelope
 * (`$lib/state/persist`), parse-and-drop like every other store: a
 * malformed payload re-defaults, never throws.
 *
 * The exported {@link appSettings} singleton is the single read/write
 * surface; components bind to it, tests inject `localStorage`.
 */

import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";

/** localStorage key for the global settings envelope. */
export const APP_SETTINGS_KEY = "mygitui.settings";

/** What persists across launches. */
export interface AppSettingsData {
  /**
   * Create a restorable checkpoint before destructive working-copy
   * mutations (discard, clean). Off = mutate directly, nothing to undo
   * from the checkpoints panel.
   */
  checkpointsEnabled: boolean;
  /** Terminal font size in px (clamped to a readable range). */
  terminalFontSize: number;
  /**
   * Auto-fetch cadence (minutes) applied when a repo has no per-repo
   * override; `0` disables auto-fetch by default.
   */
  defaultAutofetchMinutes: number;
  /**
   * Ask for confirmation before discarding working-copy changes.
   * (The per-action confirm dialog already exists; this is the master
   * switch for users who find it noisy.)
   */
  confirmDiscard: boolean;
}

export const APP_SETTINGS_DEFAULTS: AppSettingsData = {
  checkpointsEnabled: true,
  terminalFontSize: 13,
  defaultAutofetchMinutes: 0,
  confirmDiscard: true,
};

/** Terminal font size bounds shared by the setting and the terminal store. */
export const TERMINAL_FONT_MIN = 9;
export const TERMINAL_FONT_MAX = 28;

/** Auto-fetch cadence bounds (minutes; 0 = off). */
export const AUTOFETCH_MINUTES_MIN = 0;
export const AUTOFETCH_MINUTES_MAX = 720;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** Parse-and-drop loader: unknown/malformed fields re-default. */
export function parseAppSettings(data: unknown): AppSettingsData {
  if (typeof data !== "object" || data === null) return { ...APP_SETTINGS_DEFAULTS };
  const raw = data as Partial<AppSettingsData>;
  const out: AppSettingsData = { ...APP_SETTINGS_DEFAULTS };
  if (typeof raw.checkpointsEnabled === "boolean") out.checkpointsEnabled = raw.checkpointsEnabled;
  if (typeof raw.terminalFontSize === "number" && Number.isFinite(raw.terminalFontSize)) {
    out.terminalFontSize = Math.round(
      clamp(raw.terminalFontSize, TERMINAL_FONT_MIN, TERMINAL_FONT_MAX),
    );
  }
  if (
    typeof raw.defaultAutofetchMinutes === "number" &&
    Number.isFinite(raw.defaultAutofetchMinutes)
  ) {
    out.defaultAutofetchMinutes = Math.round(
      clamp(raw.defaultAutofetchMinutes, AUTOFETCH_MINUTES_MIN, AUTOFETCH_MINUTES_MAX),
    );
  }
  if (typeof raw.confirmDiscard === "boolean") out.confirmDiscard = raw.confirmDiscard;
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

export class AppSettingsStore {
  #storage: StorageLike | null;
  #value = $state<AppSettingsData>({ ...APP_SETTINGS_DEFAULTS });

  constructor(storage: StorageLike | null = safeStorage()) {
    this.#storage = storage;
    const loaded = loadVersioned<AppSettingsData>({
      storage,
      key: APP_SETTINGS_KEY,
      parse: parseAppSettings,
    });
    if (loaded !== null) this.#value = loaded;
  }

  /** Current settings (reactive). */
  get value(): AppSettingsData {
    return this.#value;
  }

  get checkpointsEnabled(): boolean {
    return this.#value.checkpointsEnabled;
  }

  get terminalFontSize(): number {
    return this.#value.terminalFontSize;
  }

  get defaultAutofetchMinutes(): number {
    return this.#value.defaultAutofetchMinutes;
  }

  get confirmDiscard(): boolean {
    return this.#value.confirmDiscard;
  }

  /** Patch settings and persist (partial update, validated on write). */
  update(patch: Partial<AppSettingsData>): void {
    const next = parseAppSettings({ ...this.#value, ...patch });
    this.#value = next;
    writeEnvelope(this.#storage, APP_SETTINGS_KEY, next);
  }

  /** Reset to defaults and persist. */
  reset(): void {
    this.update(APP_SETTINGS_DEFAULTS);
  }
}

/** App-wide settings singleton. */
export const appSettings = new AppSettingsStore();
