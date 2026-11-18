/**
 * Persisted per-repository workspace session — the durable home for the
 * center editor's state: open file tabs (+ active tab), the open working-copy
 * diff, the shown commit detail, and the scroll positions of the open file
 * and diff. RepoView mirrors every workspace mutation into it and restores
 * from it on repo re-entry (in-app tab switches) and app relaunch — the
 * state that used to be wiped on every switch.
 *
 * Keyed by repo ROOT (the stable cross-launch identity — repo ids are
 * per-session), like `historyPrefs` / `fileTreePrefs`. Storage follows the
 * per-feature localStorage convention: `mygitui.workspace.<root>` in the
 * versioned envelope (`$lib/state/persist`), 150 ms debounced writes,
 * parse-validated reads (a malformed payload is dropped, never thrown), all
 * storage access swallowed in a try/catch (privacy modes / SSR / tests may
 * not have localStorage). Inject `storage: null` for hermetic tests.
 *
 * Two things stay IN MEMORY ONLY (they die with the process on purpose):
 * the full `CommitInfo` of an open commit detail (so an in-app switch
 * round-trip restores it instantly; a cold launch re-opens it through
 * HistoryView's persisted-selection resolve) and the dirty-buffer doc text
 * of open editors (captured at switch time — strings are too big to write
 * on every keystroke).
 */

import type { CommitInfo } from "$lib/ipc/types";
import type { DiffScope } from "$lib/components/panels/statusModel";
import {
  loadVersioned,
  writeEnvelope,
  type StorageLike,
} from "$lib/state/persist";

/** Storage key prefix; the repo root is appended verbatim. */
export const WORKSPACE_SESSION_KEY_PREFIX = "mygitui.workspace.";

/** Debounce window for persisted writes (exported for tests). */
export const WORKSPACE_SESSION_DEBOUNCE_MS = 150;

/** Center modes that persist (compare / run views are transient → "empty"). */
export type SessionCenterKind = "empty" | "files" | "diff" | "commit";

/** One open working-copy diff (list shape — only one is open today). */
export interface StoredDiffRef {
  path: string;
  scope: DiffScope;
}

/** What gets persisted per repo root. */
export interface StoredWorkspaceSession {
  /** Open file paths, tab order = open order. */
  openFiles: string[];
  /** Active (visible) workspace tab. */
  activeFile: string | null;
  /** The center editor's persisted mode. */
  centerKind: SessionCenterKind;
  /** Open working-copy diffs. */
  diffs: StoredDiffRef[];
  /** The diff the center editor shows. */
  activeDiffPath: string | null;
  /** The commit detail's sha (matches the in-memory commit when present). */
  commitSha: string | null;
  /** Scroll positions, keys prefixed `file:<path>` / `diff:<path>`. */
  scrolls: Record<string, number>;
}

/** Scroll-record key for an open file's editor. */
export function fileScrollKey(path: string): string {
  return `file:${path}`;
}

/** Scroll-record key for the open working-copy diff. */
export function diffScrollKey(path: string): string {
  return `diff:${path}`;
}

/**
 * Scroll-record key for an open diff's FIRST VISIBLE ROW (the scroll-stable
 * restore position: a row index survives wrap/mode height changes that
 * would drift a px offset). Stored in the same `scrolls` map as the px
 * keys, so parsing needs no schema change.
 */
export function diffRowKey(path: string): string {
  return `diffrow:${path}`;
}

/** The session value used before anything was persisted for a root. */
export function emptyStoredSession(): StoredWorkspaceSession {
  return {
    openFiles: [],
    activeFile: null,
    centerKind: "empty",
    diffs: [],
    activeDiffPath: null,
    commitSha: null,
    scrolls: {},
  };
}

const CENTER_KINDS: readonly SessionCenterKind[] = [
  "empty",
  "files",
  "diff",
  "commit",
];

function isSessionCenterKind(value: unknown): value is SessionCenterKind {
  return CENTER_KINDS.includes(value as SessionCenterKind);
}

function isDiffScope(value: unknown): value is DiffScope {
  return value === "staged" || value === "unstaged";
}

/**
 * Parse-and-drop validation for a persisted payload: required collections
 * and enums must have the right shape or the WHOLE session is dropped (the
 * caller re-defaults to empty, like every other store); optional nullable
 * strings and individual scroll entries degrade instead of dropping.
 */
export function parseStoredWorkspaceSession(
  data: unknown,
): StoredWorkspaceSession | null {
  if (typeof data !== "object" || data === null || Array.isArray(data)) {
    return null;
  }
  const v = data as Record<string, unknown>;
  if (
    !Array.isArray(v.openFiles) ||
    !v.openFiles.every((p) => typeof p === "string")
  ) {
    return null;
  }
  if (!isSessionCenterKind(v.centerKind)) return null;
  if (!Array.isArray(v.diffs)) return null;
  const diffs: StoredDiffRef[] = [];
  for (const entry of v.diffs) {
    if (typeof entry !== "object" || entry === null) return null;
    const ref = entry as Record<string, unknown>;
    if (typeof ref.path !== "string" || !isDiffScope(ref.scope)) return null;
    diffs.push({ path: ref.path, scope: ref.scope });
  }
  const scrolls: Record<string, number> = {};
  if (v.scrolls !== undefined) {
    if (typeof v.scrolls !== "object" || v.scrolls === null || Array.isArray(v.scrolls)) {
      return null;
    }
    for (const [key, top] of Object.entries(v.scrolls)) {
      if (typeof top === "number" && Number.isFinite(top) && top >= 0) {
        scrolls[key] = top;
      }
    }
  }
  const nullableString = (x: unknown): string | null =>
    typeof x === "string" ? x : null;
  return {
    openFiles: v.openFiles as string[],
    activeFile: nullableString(v.activeFile),
    centerKind: v.centerKind,
    diffs,
    activeDiffPath: nullableString(v.activeDiffPath),
    commitSha: nullableString(v.commitSha),
    scrolls,
  };
}

function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/**
 * In-memory side (never persisted): the full commit object behind
 * `commitSha` and the dirty-buffer doc text per open file.
 */
interface MemorySession {
  stored: StoredWorkspaceSession;
  commit: CommitInfo | null;
  buffers: Map<string, string>;
}

/** What {@link WorkspaceSessionStore.patch} may overwrite. */
export type WorkspaceSessionPatch = Partial<
  Omit<StoredWorkspaceSession, "scrolls">
>;

interface WorkspaceSessionStoreOptions {
  storage?: StorageLike | null;
}

export class WorkspaceSessionStore {
  #storage: StorageLike | null;
  #timers = new Map<string, ReturnType<typeof setTimeout>>();
  #memory = new Map<string, MemorySession>();

  constructor(options: WorkspaceSessionStoreOptions = {}) {
    this.#storage =
      options.storage !== undefined ? options.storage : defaultStorage();
  }

  /** Hydrates the root's session (storage → empty default) and returns it. */
  ensure(root: string): StoredWorkspaceSession {
    return this.#memoryFor(root).stored;
  }

  /** The root's memory record, hydrating from storage (or empty) on demand. */
  #memoryFor(root: string): MemorySession {
    let memory = this.#memory.get(root);
    if (!memory) {
      memory = {
        stored:
          loadVersioned<StoredWorkspaceSession>({
            storage: this.#storage,
            key: WORKSPACE_SESSION_KEY_PREFIX + root,
            parse: parseStoredWorkspaceSession,
          }) ?? emptyStoredSession(),
        commit: null,
        buffers: new Map(),
      };
      this.#memory.set(root, memory);
    }
    return memory;
  }

  /**
   * The root's persisted session, or `null` when nothing was ever stored
   * for it (a repo only touched before this store existed). Unlike
   * {@link ensure} this does not materialize an empty session.
   */
  get(root: string): StoredWorkspaceSession | null {
    const memory = this.#memory.get(root);
    if (memory) return memory.stored;
    const parsed = loadVersioned<StoredWorkspaceSession>({
      storage: this.#storage,
      key: WORKSPACE_SESSION_KEY_PREFIX + root,
      parse: parseStoredWorkspaceSession,
    });
    if (parsed === null) return null;
    this.#memory.set(root, { stored: parsed, commit: null, buffers: new Map() });
    return parsed;
  }

  /** Merges `partial` into the root's session and persists (debounced). */
  patch(root: string, partial: WorkspaceSessionPatch): void {
    const memory = this.#memoryFor(root);
    Object.assign(memory.stored, partial);
    this.scheduleWrite(root);
  }

  /** Records one scroll position and persists (debounced). */
  setScroll(root: string, key: string, top: number): void {
    const memory = this.#memoryFor(root);
    memory.stored.scrolls[key] = top;
    this.scheduleWrite(root);
  }

  /** The in-memory commit object for the root's open detail (null = none). */
  commitFor(root: string): CommitInfo | null {
    return this.#memory.get(root)?.commit ?? null;
  }

  /** Remembers the commit object behind the root's open detail (memory only). */
  setCommit(root: string, commit: CommitInfo | null): void {
    this.#memoryFor(root).commit = commit;
  }

  /** Captures a dirty editor's doc text at switch time (memory only). */
  setBuffer(root: string, path: string, text: string): void {
    this.#memoryFor(root).buffers.set(path, text);
  }

  /** Drains the root's captured buffers (restore consumes them once). */
  takeBuffers(root: string): Record<string, string> {
    const memory = this.#memory.get(root);
    if (!memory || memory.buffers.size === 0) return {};
    const out: Record<string, string> = {};
    for (const [path, text] of memory.buffers) out[path] = text;
    memory.buffers.clear();
    return out;
  }

  /**
   * A file tab closed: drop its scroll entry and any captured buffer so a
   * later restore cannot resurrect state for a path the user dismissed.
   */
  forgetFile(root: string, path: string): void {
    const memory = this.#memory.get(root);
    if (!memory) return;
    memory.buffers.delete(path);
    if (fileScrollKey(path) in memory.stored.scrolls) {
      delete memory.stored.scrolls[fileScrollKey(path)];
      this.scheduleWrite(root);
    }
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
      if (storage?.removeItem) {
        storage.removeItem(WORKSPACE_SESSION_KEY_PREFIX + root);
      } else if (storage) {
        // Minimal StorageLike without removal: persist the empty session.
        writeEnvelope(storage, WORKSPACE_SESSION_KEY_PREFIX + root, emptyStoredSession());
      }
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
      if (memory) this.#writeNow(root, memory.stored);
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
        if (memory) this.#writeNow(root, memory.stored);
      }, WORKSPACE_SESSION_DEBOUNCE_MS),
    );
  }

  #writeNow(root: string, stored: StoredWorkspaceSession): void {
    writeEnvelope(this.#storage, WORKSPACE_SESSION_KEY_PREFIX + root, stored);
  }
}

/** App singleton (localStorage-backed). */
export const workspaceSession = new WorkspaceSessionStore();

// Standalone function API over the singleton (what components import). --

/** The root's persisted workspace session (see {@link WorkspaceSessionStore.get}). */
export function getWorkspaceSession(
  root: string,
): StoredWorkspaceSession | null {
  return workspaceSession.get(root);
}

/** Merges state into the root's session (see {@link WorkspaceSessionStore.patch}). */
export function patchWorkspaceSession(
  root: string,
  partial: WorkspaceSessionPatch,
): void {
  workspaceSession.patch(root, partial);
}

/** Records a scroll position (see {@link WorkspaceSessionStore.setScroll}). */
export function setWorkspaceScroll(
  root: string,
  key: string,
  top: number,
): void {
  workspaceSession.setScroll(root, key, top);
}

/** Evicts a closed repo's session (see {@link WorkspaceSessionStore.forget}). */
export function forgetWorkspaceSession(root: string): void {
  workspaceSession.forget(root);
}

/** Flushes every pending write (see {@link WorkspaceSessionStore.flush}). */
export function flushWorkspaceSessions(): void {
  workspaceSession.flush();
}

/** Test reset for the module singleton. */
export function resetWorkspaceSessions(): void {
  workspaceSession.reset();
}
