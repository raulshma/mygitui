/**
 * Terminal quick-command snippets (Svelte 5 runes) — the TerminalPanel
 * snippet bar's data.
 *
 * Three built-in commands (git status / log / fetch) are always shown; two
 * user-defined slots ride alongside them, persisted per repo ROOT under the
 * versioned `mygitui.*` envelope (`$lib/state/persist`) in
 * `mygitui.terminal-snippets` as `{ [root]: TerminalSnippet[] }` — the same
 * root keying as autofetch/bookmarks (roots are stable across restarts,
 * repo ids are minted per session). Malformed payloads drop to the defaults;
 * slot arrays are capped at {@link USER_SNIPPET_SLOTS}.
 *
 * Pure store, no backend: clicking a snippet in the panel routes the
 * command through the terminal store's `writeInput` (pty keystroke lane).
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";

/** localStorage key: envelope-wrapped `{ [root]: TerminalSnippet[] }`. */
export const TERMINAL_SNIPPETS_STORAGE_KEY = "mygitui.terminal-snippets";

/** One snippet: bar label + the command written to the pty on click. */
export interface TerminalSnippet {
  label: string;
  command: string;
}

/** The always-present built-ins (fixed order, not editable). */
export const BUILTIN_SNIPPETS: readonly TerminalSnippet[] = [
  { label: "git status", command: "git status" },
  { label: "git log", command: "git log --oneline -10" },
  { label: "git fetch", command: "git fetch --all --prune" },
];

/** User-defined slots per root (the manage popover edits exactly these). */
export const USER_SNIPPET_SLOTS = 2;

/** An empty user slot (rendered as "set snippet" in the manage popover). */
export function emptySnippet(): TerminalSnippet {
  return { label: "", command: "" };
}

/** True when a slot carries anything worth writing to the pty. */
export function isFilled(snippet: TerminalSnippet): boolean {
  return snippet.label.trim() !== "" || snippet.command.trim() !== "";
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Validates one persisted snippet; `null` for malformed entries. */
function parseSnippet(value: unknown): TerminalSnippet | null {
  if (typeof value !== "object" || value === null) return null;
  const { label, command } = value as { label?: unknown; command?: unknown };
  if (typeof label !== "string" || typeof command !== "string") return null;
  return { label, command };
}

/** Parses the envelope payload; drops malformed roots / entries, caps slots. */
export function parseSnippets(data: unknown): Record<string, TerminalSnippet[]> {
  if (typeof data !== "object" || data === null) return {};
  const out: Record<string, TerminalSnippet[]> = {};
  for (const [root, slots] of Object.entries(data as Record<string, unknown>)) {
    if (root === "" || !Array.isArray(slots)) continue;
    const parsed = slots
      .slice(0, USER_SNIPPET_SLOTS)
      .map(parseSnippet)
      .filter((snippet): snippet is TerminalSnippet => snippet !== null);
    if (parsed.length > 0) out[root] = parsed;
  }
  return out;
}

export class SnippetStore {
  /** User slots per repo root (missing roots = all-empty slots). */
  user: Record<string, TerminalSnippet[]> = $state({});

  #storage: StorageLike | null;
  /** Roots whose persisted payload was already hydrated. */
  readonly #loaded = new Set<string>();

  constructor(storage: StorageLike | null = defaultStorage()) {
    this.#storage = storage;
  }

  /**
   * The root's user slots, always padded to {@link USER_SNIPPET_SLOTS} so
   * the manage popover renders a stable row count. Hydrates the persisted
   * payload on first access per root (read-ensures-loaded, like the
   * envelope module's callers).
   */
  userFor(root: string): TerminalSnippet[] {
    if (!this.#loaded.has(root)) {
      this.#loaded.add(root);
      const stored = loadVersioned<Record<string, TerminalSnippet[]>>({
        storage: this.#storage,
        key: TERMINAL_SNIPPETS_STORAGE_KEY,
        parse: parseSnippets,
      });
      if (stored !== null && Array.isArray(stored[root])) {
        this.user = { ...this.user, [root]: stored[root]! };
      }
    }
    const slots = this.user[root] ?? [];
    return [...slots, ...Array.from({ length: Math.max(0, USER_SNIPPET_SLOTS - slots.length) }, emptySnippet)];
  }

  /**
   * Writes one user slot for `root` (the manage popover's Save) and
   * persists the whole per-root map through the versioned envelope.
   */
  setUser(root: string, index: number, snippet: TerminalSnippet): void {
    if (index < 0 || index >= USER_SNIPPET_SLOTS) return;
    const slots = this.userFor(root).slice(0, USER_SNIPPET_SLOTS);
    slots[index] = snippet;
    this.user = { ...this.user, [root]: slots };
    writeEnvelope(this.#storage, TERMINAL_SNIPPETS_STORAGE_KEY, this.user);
  }

  /**
   * Drops a root's in-memory slots (repo closed). Deliberately does NOT
   * touch storage: user snippets are persisted preferences (like autofetch
   * intervals) and must survive a tab close — a re-opened repo re-hydrates
   * them from the envelope.
   */
  forget(root: string): void {
    if (!(root in this.user) && !this.#loaded.has(root)) return;
    const next = { ...this.user };
    delete next[root];
    this.user = next;
    this.#loaded.delete(root);
  }

  /** Drops everything (test helper; storage is untouched). */
  reset(): void {
    this.user = {};
    this.#loaded.clear();
  }
}

/** The application-wide snippet store. */
export const terminalSnippets = new SnippetStore();
