/**
 * Commit-authoring extras store (Svelte 5 runes) — per-repo state the commit
 * bar needs beyond the draft itself.
 *
 * - Recent commit messages: the last five ACCEPTED commit messages per repo
 *   root, persisted under `mygitui.commitRecents.<root>` through the
 *   versioned envelope (`$lib/state/persist`) so the dropdown can recall
 *   one. Newest first, deduped (re-using a message moves it back to the
 *   top).
 * - Wrap-ruler width per repo root: the commit-width guide column
 *   (git's 72-character convention), persisted under
 *   `mygitui.commitWidth.<root>` through the same envelope.
 * - Template-attempted bookkeeping (memory only): the commit template
 *   prefills once per repo per session, never re-overwriting a draft.
 *
 * Storage follows the house conventions: parse-validated reads, writes
 * swallowed in a try/catch (privacy modes / tests may not have
 * localStorage). Lives in a `.svelte.ts` module because `$state` only
 * compiles there.
 */

import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";

/** Storage key prefix for per-root recent commit messages. */
export const COMMIT_RECENTS_PREFIX = "mygitui.commitRecents.";

/** Storage key prefix for the per-root commit wrap-ruler width. */
export const COMMIT_WIDTH_PREFIX = "mygitui.commitWidth.";

/** How many accepted commit messages are kept per repo. */
export const COMMIT_RECENTS_LIMIT = 5;

/** The default wrap-ruler column (git convention: summary ≤ 50, body ≤ 72). */
export const COMMIT_WIDTH_DEFAULT = 72;
/** Wrap-ruler width bounds (columns). */
export const COMMIT_WIDTH_MIN = 40;
export const COMMIT_WIDTH_MAX = 120;

/**
 * Pure recents-ring insert: moves `message` to the front (deduped,
 * whitespace-trimmed), capped at `limit`. Empty/blank messages are ignored;
 * the input array is never mutated.
 */
export function pushRecentMessage(
  messages: readonly string[],
  message: string,
  limit: number = COMMIT_RECENTS_LIMIT,
): string[] {
  const trimmed = message.trim();
  if (trimmed === "") return [...messages];
  const rest = messages.filter((m) => m !== trimmed);
  return [trimmed, ...rest].slice(0, Math.max(1, limit));
}

/** Clamps a wrap-ruler width into the valid column range (integer). */
export function normalizeCommitWidth(width: number): number {
  if (!Number.isFinite(width)) return COMMIT_WIDTH_DEFAULT;
  return Math.round(
    Math.min(COMMIT_WIDTH_MAX, Math.max(COMMIT_WIDTH_MIN, width)),
  );
}

/**
 * The wrap-ruler's CSS `left` offset inside the commit textarea: the box
 * pads `paddingRem` (tailwind `px-2`) and the text is monospace, so column
 * `width` lands at `padding + width × 1ch`. Returned as a CSS calc value.
 */
export function rulerLeftCss(
  width: number,
  paddingRem: number = 0.5,
): string {
  return `calc(${paddingRem}rem + ${normalizeCommitWidth(width)}ch)`;
}

/**
 * Commit-template prefill gate (pure): the template applies only when the
 * setting is on, the repo has not been attempted yet this session, and the
 * draft is still empty.
 */
export function shouldPrefillTemplate(opts: {
  enabled: boolean;
  tried: boolean;
  message: string;
}): boolean {
  return opts.enabled && !opts.tried && opts.message.trim() === "";
}

/**
 * Strips a `commit.template`-style message body down to the draft to
 * prefill: comment lines (`#…`, as git inserts for the editor) go, leading
 * blanks go, trailing blanks go. The result may be "" (template is all
 * comments) — the caller then prefills nothing.
 */
export function templateFromBody(template: string): string {
  const kept = template
    .split("\n")
    .filter((line) => !line.trimStart().startsWith("#"));
  return kept.join("\n").replace(/^\s*\n/, "").trimEnd();
}

/** Parses a persisted recents payload (envelope-unwrapped); null = bad. */
export function parseCommitRecents(data: unknown): string[] | null {
  if (!Array.isArray(data)) return null;
  const out: string[] = [];
  for (const entry of data) {
    if (typeof entry !== "string" || entry.trim() === "") continue;
    if (out.includes(entry)) continue;
    out.push(entry);
  }
  return out;
}

/** Parses a persisted wrap-ruler width payload; null = bad. */
export function parseCommitWidth(data: unknown): number | null {
  return typeof data === "number" && Number.isFinite(data)
    ? normalizeCommitWidth(data)
    : null;
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export class CommitExtrasStore {
  /**
   * Recent accepted commit messages per repo root (newest first).
   * Hydration from storage is lazy but must happen OUTSIDE derived reads —
   * call {@link ensure} from an `$effect` (the branchColorStore contract).
   */
  #recents: Record<string, string[]> = $state({});
  /** Wrap-ruler width per repo root (unset roots follow the default). */
  #widths: Record<string, number> = $state({});
  /** Roots whose template prefill was already attempted this session. */
  #templateTried = new Set<string>();

  #storage: StorageLike | null;

  constructor(storage: StorageLike | null = defaultStorage()) {
    this.#storage = storage;
  }

  /**
   * Hydrates `root`'s persisted recents + width (idempotent; call from an
   * `$effect` — see the class doc).
   */
  ensure(root: string): void {
    if (!(root in this.#recents)) {
      this.#recents = {
        ...this.#recents,
        [root]:
          loadVersioned<string[]>({
            storage: this.#storage,
            key: COMMIT_RECENTS_PREFIX + root,
            parse: parseCommitRecents,
          }) ?? [],
      };
    }
    if (!(root in this.#widths)) {
      this.#widths = {
        ...this.#widths,
        [root]:
          loadVersioned<number>({
            storage: this.#storage,
            key: COMMIT_WIDTH_PREFIX + root,
            parse: parseCommitWidth,
          }) ?? COMMIT_WIDTH_DEFAULT,
      };
    }
  }

  /** Accepted commit messages for a root, newest first (empty when none). */
  recentsFor(root: string): string[] {
    return this.#recents[root] ?? [];
  }

  /** Records an accepted commit message for a root (deduped, capped). */
  rememberMessage(root: string, message: string): void {
    const next = pushRecentMessage(this.#recents[root] ?? [], message);
    this.#recents = { ...this.#recents, [root]: next };
    writeEnvelope(this.#storage, COMMIT_RECENTS_PREFIX + root, next);
  }

  /** The wrap-ruler width for a root (default when unset). */
  widthFor(root: string): number {
    return this.#widths[root] ?? COMMIT_WIDTH_DEFAULT;
  }

  /** Sets (and persists) the wrap-ruler width for a root. */
  setWidth(root: string, width: number): void {
    const safe = normalizeCommitWidth(width);
    this.#widths = { ...this.#widths, [root]: safe };
    writeEnvelope(this.#storage, COMMIT_WIDTH_PREFIX + root, safe);
  }

  /**
   * True when the template may prefill for a root right now (setting on,
   * not yet attempted, draft empty). Marks the root attempted as a side
   * effect so the gate fires once per repo per session.
   */
  claimTemplatePrefill(root: string, enabled: boolean, message: string): boolean {
    const allowed = shouldPrefillTemplate({
      enabled,
      tried: this.#templateTried.has(root),
      message,
    });
    this.#templateTried.add(root);
    return allowed;
  }

  /** Drops per-session bookkeeping (tests). */
  reset(): void {
    this.#templateTried.clear();
  }
}

/** The application-wide commit-extras store. */
export const commitExtras = new CommitExtrasStore();
