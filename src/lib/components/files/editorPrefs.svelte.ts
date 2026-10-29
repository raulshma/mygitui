/**
 * File-editor preferences (module singleton): line-wrap toggle, persisted
 * under `mygitui.files.wrap` like the diff viewer's `mygitui.diff.wrap`.
 */

const WRAP_KEY = "mygitui.files.wrap";

function readWrap(): boolean {
  try {
    return localStorage.getItem(WRAP_KEY) === "1";
  } catch {
    return false;
  }
}

let wrap = $state(readWrap());

export const editorPrefs = {
  get wrap(): boolean {
    return wrap;
  },
  setWrap(value: boolean): void {
    wrap = value;
    try {
      localStorage.setItem(WRAP_KEY, value ? "1" : "0");
    } catch {
      // Keep the in-memory preference.
    }
  },
};
