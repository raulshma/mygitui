/**
 * Persisted per-repository history UI state — the commit-history view's
 * filter fields, selected commit and detail-pane visibility. HistoryView
 * saves on every change and restores on mount / repo re-target, so a
 * relaunched app (or a repo tab revisited) comes back where it was.
 *
 * Follows the layout store's storage conventions: `mygitui.*` key prefix
 * keyed by repo root (the stable cross-launch identity — repo ids are
 * per-session), 150 ms debounced writes, parse-validated reads, and all
 * storage access swallowed in a try/catch (privacy modes / SSR / tests may
 * not have localStorage).
 */
import { EMPTY_FILTER, type HistoryFilterFields } from "./history-logic";

/** Storage key prefix; the repo root is appended verbatim. */
export const HISTORY_PREFS_PREFIX = "mygitui.history.";

/** Debounce window for persisted writes. */
export const HISTORY_PREFS_DEBOUNCE_MS = 150;

/** What gets persisted per repo root. */
export interface HistoryPrefs {
  /** Filter-bar fields as typed (restored before the first stream). */
  filter: HistoryFilterFields;
  /** Selected commit sha, or null. */
  selectedSha: string | null;
  /** Whether the commit-detail pane was open for the selection. */
  detailOpen: boolean;
}

/** One pending write per root (typing streams writes; only the last lands). */
const pending = new Map<
  string,
  { timer: ReturnType<typeof setTimeout>; prefs: HistoryPrefs }
>();

const SHA_RE = /^[0-9a-f]{4,64}$/i;
const DAY_RE = /^\d{4}-\d{2}-\d{2}$/;

/** Parses persisted prefs JSON, dropping malformed fields (never throws). */
export function parseHistoryPrefs(raw: string | null): HistoryPrefs | null {
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const v = parsed as Record<string, unknown>;
  const f = (v.filter ?? null) as Record<string, unknown> | null;
  const str = (x: unknown): string => (typeof x === "string" ? x : "");
  const day = (x: unknown): string => {
    const s = str(x);
    return DAY_RE.test(s) ? s : "";
  };
  const filter: HistoryFilterFields = {
    text: str(f?.text),
    regex: f?.regex === true,
    author: str(f?.author),
    path: str(f?.path),
    after: day(f?.after),
    before: day(f?.before),
    pickaxe: str(f?.pickaxe),
    pickaxeRegex: str(f?.pickaxeRegex),
  };
  const selectedSha =
    typeof v.selectedSha === "string" && SHA_RE.test(v.selectedSha)
      ? v.selectedSha
      : null;
  return {
    filter,
    selectedSha,
    // Detail visibility only means something with a selection to show
    // (compare-bar results are transient and never persisted).
    detailOpen: v.detailOpen === true && selectedSha !== null,
  };
}

/** Reads the persisted history state for a repo root (null when none). */
export function loadHistoryPrefs(root: string): HistoryPrefs | null {
  if (root === "") return null;
  try {
    return parseHistoryPrefs(
      globalThis.localStorage?.getItem(HISTORY_PREFS_PREFIX + root) ?? null,
    );
  } catch {
    return null;
  }
}

/** Records history state for a root (debounced per root). */
export function saveHistoryPrefs(root: string, prefs: HistoryPrefs): void {
  if (root === "") return;
  const entry = pending.get(root);
  if (entry) clearTimeout(entry.timer);
  pending.set(root, {
    timer: setTimeout(() => {
      pending.delete(root);
      writeNow(root, prefs);
    }, HISTORY_PREFS_DEBOUNCE_MS),
    prefs,
  });
}

/** Immediate synchronous flush of all pending writes (test/teardown aid). */
export function flushHistoryPrefs(): void {
  for (const [root, entry] of [...pending]) {
    clearTimeout(entry.timer);
    pending.delete(root);
    writeNow(root, entry.prefs);
  }
}

function writeNow(root: string, prefs: HistoryPrefs): void {
  try {
    globalThis.localStorage?.setItem(
      HISTORY_PREFS_PREFIX + root,
      JSON.stringify(prefs),
    );
  } catch {
    // Storage full/unavailable: state still works this session.
  }
}

/** The prefs value used before anything was persisted for a root. */
export function emptyPrefs(): HistoryPrefs {
  return { filter: { ...EMPTY_FILTER }, selectedSha: null, detailOpen: false };
}
