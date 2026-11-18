/**
 * Persisted diff-viewer preferences beyond wrap/mode (those live in the
 * plain wrapPref/modePref modules): whitespace handling (items: "ignore
 * whitespace" changes the backend diff query, "hide whitespace changes" is
 * a row-model filter) and word-level highlight rendering.
 *
 * Same localStorage conventions as the other prefs (`mygitui.diff.*` keys,
 * parse-validated reads, guarded writes), plus a reactive mirror so the
 * streaming parents (which build the diff query) and every mounted viewer
 * stay in sync without prop drilling.
 */

/** Reads a stored boolean; missing/garbled values fall back to `fallback`. */
function readBool(key: string, fallback: boolean): boolean {
  try {
    const raw = localStorage.getItem(key);
    if (raw === null) return fallback;
    const value: unknown = JSON.parse(raw);
    return typeof value === "boolean" ? value : fallback;
  } catch {
    return fallback;
  }
}

/** Writes a stored boolean (persistence is best-effort). */
function writeBool(key: string, value: boolean): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Persistence is best-effort.
  }
}

const IGNORE_WS_KEY = "mygitui.diff.ignore-ws";
const HIDE_WS_KEY = "mygitui.diff.hide-ws";
const WORD_HL_KEY = "mygitui.diff.word-hl";

/** Read the persisted "ignore whitespace" diff-query flag (default off). */
export function readIgnoreWsPref(): boolean {
  return readBool(IGNORE_WS_KEY, false);
}

/** Read the persisted "hide whitespace changes" row filter (default off). */
export function readHideWsPref(): boolean {
  return readBool(HIDE_WS_KEY, false);
}

/** Read the persisted word-highlight rendering gate (default on). */
export function readWordHighlightPref(): boolean {
  return readBool(WORD_HL_KEY, true);
}

/**
 * Reactive mirror of the three toggles. Diff toolbars write through the
 * setters below; streaming parents read `ignoreWs` when building their
 * diff query, so a flip re-renders every surface showing a diff.
 */
export const diffPrefs = $state({
  ignoreWs: readIgnoreWsPref(),
  hideWs: readHideWsPref(),
  wordHighlights: readWordHighlightPref(),
});

/** Sets the ignore-whitespace query flag (persisted). */
export function setIgnoreWs(value: boolean): void {
  diffPrefs.ignoreWs = value;
  writeBool(IGNORE_WS_KEY, value);
}

/** Sets the hide-whitespace-rows filter (persisted). */
export function setHideWs(value: boolean): void {
  diffPrefs.hideWs = value;
  writeBool(HIDE_WS_KEY, value);
}

/** Sets the word-highlight rendering gate (persisted). */
export function setWordHighlights(value: boolean): void {
  diffPrefs.wordHighlights = value;
  writeBool(WORD_HL_KEY, value);
}
