/**
 * Persisted diff layout preference (`mygitui.diff.mode`, default split).
 * Uses the same guarded localStorage conventions as the other diff prefs.
 */

import type { DiffMode } from "$lib/components/diff/rowModel";

const KEY = "mygitui.diff.mode";

export function readDiffModePref(): DiffMode {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null) return "split";
    const value: unknown = JSON.parse(raw);
    return value === "unified" ? "unified" : "split";
  } catch {
    return "split";
  }
}

export function writeDiffModePref(mode: DiffMode): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(mode));
  } catch {
    // Persistence is best-effort.
  }
}
