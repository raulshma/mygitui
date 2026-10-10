/**
 * Persisted "wrap diff lines" preference (`mygitui.diff.wrap`, default ON —
 * long lines wrap so nothing hides behind a horizontal scrollbar; turning it
 * off restores per-diff horizontal scrolling). Same localStorage conventions
 * as the other prefs: `mygitui.*` key, parse-validated read, guarded access.
 */

const KEY = "mygitui.diff.wrap";

export function readWrapPref(): boolean {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null) return true;
    return JSON.parse(raw) === false ? false : true;
  } catch {
    return true;
  }
}

export function writeWrapPref(wrap: boolean): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(wrap));
  } catch {
    // Persistence is best-effort.
  }
}
