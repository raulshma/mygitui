/**
 * Persisted per-repository Agent-mode session — the durable home for the
 * agent workspace's layout and selection: the changes-column split ratio,
 * the right-column split ratio (when the PR/chat column is shown), and the
 * selected change path + diff side. AgentView restores from it on repo
 * re-entry (and app relaunch) instead of resetting selection on every
 * switch.
 *
 * Keyed by repo ROOT (the stable cross-launch identity — repo ids are
 * per-session) like `workspaceSession` / `historyPrefs`. Storage follows
 * the per-feature localStorage convention: `mygitui.agent.<root>` in the
 * versioned envelope (`$lib/state/persist`), 150 ms debounced writes,
 * parse-validated reads (a malformed payload is dropped, never thrown),
 * all storage access swallowed in a try/catch (privacy modes / SSR / tests
 * may not have localStorage). Inject `storage: null` for hermetic tests.
 */

import { clampRatio } from "$lib/layout/layoutModel";
import {
  loadVersioned,
  writeEnvelope,
  type StorageLike,
} from "$lib/state/persist";

/** Storage key prefix; the repo root is appended verbatim. */
export const AGENT_SESSION_KEY_PREFIX = "mygitui.agent.";

/** Debounce window for persisted writes (exported for tests). */
export const AGENT_SESSION_DEBOUNCE_MS = 150;

/** Which working-copy comparison the agent diff pane shows. */
export type AgentSide = "worktree" | "index";

/** What gets persisted per repo root. */
export interface StoredAgentSession {
  /** The change whose diff the center pane shows. */
  selectedPath: string | null;
  /** The selected change's diff side (index = staged view). */
  selectedSide: AgentSide;
  /** Changes↔diff splitter (left pane's width fraction). */
  leftRatio: number;
  /** Diff↔PR/chat splitter (center pane's width fraction). */
  rightRatio: number;
}

/** The session value used before anything was persisted for a root. */
export function emptyAgentSession(): StoredAgentSession {
  return {
    selectedPath: null,
    selectedSide: "worktree",
    leftRatio: 0.24,
    rightRatio: 0.7,
  };
}

function isAgentSide(value: unknown): value is AgentSide {
  return value === "worktree" || value === "index";
}

/** A stored ratio must be a finite number; it is clamped on read so a
 *  hand-edited payload cannot push a divider off-screen. */
function parseRatio(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value)
    ? clampRatio(value)
    : fallback;
}

/**
 * Parse-and-drop validation for a persisted payload: a wrong-typed present
 * field drops the WHOLE session (the caller re-defaults to empty, like
 * every other store); absent fields take the defaults.
 */
export function parseStoredAgentSession(data: unknown): StoredAgentSession | null {
  if (typeof data !== "object" || data === null || Array.isArray(data)) {
    return null;
  }
  const v = data as Record<string, unknown>;
  const defaults = emptyAgentSession();
  if (
    v.selectedPath !== undefined &&
    v.selectedPath !== null &&
    typeof v.selectedPath !== "string"
  ) {
    return null;
  }
  if (v.selectedSide !== undefined && !isAgentSide(v.selectedSide)) {
    return null;
  }
  return {
    selectedPath: typeof v.selectedPath === "string" ? v.selectedPath : null,
    selectedSide: isAgentSide(v.selectedSide) ? v.selectedSide : defaults.selectedSide,
    leftRatio: parseRatio(v.leftRatio, defaults.leftRatio),
    rightRatio: parseRatio(v.rightRatio, defaults.rightRatio),
  };
}

function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

interface AgentSessionStoreOptions {
  storage?: StorageLike | null;
}

export class AgentSessionStore {
  #storage: StorageLike | null;
  #timers = new Map<string, ReturnType<typeof setTimeout>>();
  #memory = new Map<string, StoredAgentSession>();

  constructor(options: AgentSessionStoreOptions = {}) {
    this.#storage =
      options.storage !== undefined ? options.storage : defaultStorage();
  }

  /** The root's persisted session (memory → storage → empty default). */
  ensure(root: string): StoredAgentSession {
    return this.#memoryFor(root);
  }

  /** The root's memory record, hydrating from storage (or empty) on demand. */
  #memoryFor(root: string): StoredAgentSession {
    let memory = this.#memory.get(root);
    if (!memory) {
      memory =
        loadVersioned<StoredAgentSession>({
          storage: this.#storage,
          key: AGENT_SESSION_KEY_PREFIX + root,
          parse: parseStoredAgentSession,
        }) ?? emptyAgentSession();
      this.#memory.set(root, memory);
    }
    return memory;
  }

  /** Merges `partial` into the root's session and persists (debounced). */
  patch(root: string, partial: Partial<StoredAgentSession>): void {
    const memory = this.#memoryFor(root);
    Object.assign(memory, partial);
    this.scheduleWrite(root);
  }

  /** Evicts the root's session entirely (memory + pending write + storage). */
  forget(root: string): void {
    const timer = this.#timers.get(root);
    if (timer) {
      clearTimeout(timer);
      this.#timers.delete(root);
    }
    this.#memory.delete(root);
    const storage = this.#storage as
      | (StorageLike & { removeItem?(key: string): void })
      | null;
    try {
      storage?.removeItem?.(AGENT_SESSION_KEY_PREFIX + root);
    } catch {
      // Storage unavailable: the session just stays gone from memory.
    }
  }

  /** Immediate synchronous flush of every pending write (pagehide / tests). */
  flush(): void {
    for (const [root, timer] of [...this.#timers]) {
      clearTimeout(timer);
      this.#timers.delete(root);
      const memory = this.#memory.get(root);
      if (memory) this.#writeNow(root, memory);
    }
  }

  /** Drops all memory + pending timers (test reset; storage is untouched). */
  reset(): void {
    for (const timer of this.#timers.values()) clearTimeout(timer);
    this.#timers.clear();
    this.#memory.clear();
  }

  private scheduleWrite(root: string): void {
    const existing = this.#timers.get(root);
    if (existing) clearTimeout(existing);
    this.#timers.set(
      root,
      setTimeout(() => {
        this.#timers.delete(root);
        const memory = this.#memory.get(root);
        if (memory) this.#writeNow(root, memory);
      }, AGENT_SESSION_DEBOUNCE_MS),
    );
  }

  #writeNow(root: string, stored: StoredAgentSession): void {
    writeEnvelope(this.#storage, AGENT_SESSION_KEY_PREFIX + root, stored);
  }
}

/** App singleton (localStorage-backed). */
export const agentSession = new AgentSessionStore();

// Standalone function API over the singleton (what components import). --

/** The root's persisted agent session (see {@link AgentSessionStore.ensure}). */
export function getAgentSession(root: string): StoredAgentSession {
  return agentSession.ensure(root);
}

/** Merges state into the root's session (see {@link AgentSessionStore.patch}). */
export function patchAgentSession(
  root: string,
  partial: Partial<StoredAgentSession>,
): void {
  agentSession.patch(root, partial);
}

/** Evicts a closed repo's session (see {@link AgentSessionStore.forget}). */
export function forgetAgentSession(root: string): void {
  agentSession.forget(root);
}

/** Flushes every pending write (see {@link AgentSessionStore.flush}). */
export function flushAgentSessions(): void {
  agentSession.flush();
}

/** Test reset for the module singleton. */
export function resetAgentSessions(): void {
  agentSession.reset();
}
