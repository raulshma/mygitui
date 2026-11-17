/**
 * Agent mode (Svelte 5 runes).
 *
 * The single "agent mode" switch for the upcoming Agent UI. Persisted
 * under `mygitui.agent` with the versioned envelope (`$lib/state/persist`),
 * parse-and-drop like every other store: a malformed payload re-defaults
 * to disabled, never throws.
 *
 * The exported {@link agentMode} singleton is the single read/write
 * surface; components bind to it, tests inject `localStorage`.
 */

import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";

/** localStorage key for the agent mode envelope. */
export const AGENT_STORAGE_KEY = "mygitui.agent";

/** What persists across launches. */
export interface AgentState {
  /** Agent mode on = the Agent UI is active in the workspace. */
  enabled: boolean;
}

export const AGENT_MODE_DEFAULTS: AgentState = { enabled: false };

/** Parse-and-drop loader: anything but `{ enabled: boolean }` is dropped. */
export function parseAgentState(data: unknown): AgentState | null {
  if (typeof data !== "object" || data === null) return null;
  const raw = data as Record<string, unknown>;
  if (Object.keys(raw).length !== 1 || typeof raw.enabled !== "boolean") return null;
  return { enabled: raw.enabled };
}

/** Returns `localStorage` when available, else `null` (never throws). */
function safeStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export class AgentModeStore {
  #storage: StorageLike | null;
  #value = $state<AgentState>({ ...AGENT_MODE_DEFAULTS });

  constructor(storage: StorageLike | null = safeStorage()) {
    this.#storage = storage;
    const loaded = loadVersioned<AgentState>({
      storage,
      key: AGENT_STORAGE_KEY,
      parse: parseAgentState,
    });
    this.#value = loaded ?? { ...AGENT_MODE_DEFAULTS };
  }

  /** Whether agent mode is on (reactive). */
  get enabled(): boolean {
    return this.#value.enabled;
  }

  /** Set agent mode and persist. */
  set(value: boolean): void {
    this.#value = { enabled: value };
    writeEnvelope(this.#storage, AGENT_STORAGE_KEY, this.#value);
  }

  /** Flip agent mode; returns the new value. */
  toggle(): boolean {
    const next = !this.#value.enabled;
    this.set(next);
    return next;
  }
}

/** Agent mode singleton. */
export const agentMode = new AgentModeStore();
