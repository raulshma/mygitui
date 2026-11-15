/**
 * File-editor preferences (module singleton): line-wrap toggle, persisted
 * under `mygitui.files.wrap` like the diff viewer's `mygitui.diff.wrap`;
 * markdown preview layout (`mygitui.files.md-view`) — the segmented
 * Edit/Split/Preview choice, remembered across sessions per the app;
 * close-all confirm (`mygitui.files.close-all-confirm`) — whether closing
 * every workspace tab at once asks first while files have unsaved edits.
 */

const WRAP_KEY = "mygitui.files.wrap";

function readWrap(): boolean {
  try {
    return localStorage.getItem(WRAP_KEY) === "1";
  } catch {
    return false;
  }
}

/** Markdown preview layout for markdown files in the editor. */
export type MdViewMode = "edit" | "split" | "preview";

const MD_VIEW_KEY = "mygitui.files.md-view";
const MD_VIEW_MODES: MdViewMode[] = ["edit", "split", "preview"];

function readMdView(): MdViewMode {
  try {
    const raw = localStorage.getItem(MD_VIEW_KEY);
    return MD_VIEW_MODES.includes(raw as MdViewMode)
      ? (raw as MdViewMode)
      : "split";
  } catch {
    return "split";
  }
}

let wrap = $state(readWrap());
let mdView = $state(readMdView());

/**
 * Parse helper for the close-all confirm flag (pure; tested directly):
 * the flag defaults to ON — anything but a stored "0" keeps asking.
 */
export function parseCloseAllConfirm(raw: string | null): boolean {
  return raw !== "0";
}

const CLOSE_ALL_CONFIRM_KEY = "mygitui.files.close-all-confirm";

function readCloseAllConfirm(): boolean {
  try {
    return parseCloseAllConfirm(localStorage.getItem(CLOSE_ALL_CONFIRM_KEY));
  } catch {
    return true;
  }
}

let closeAllConfirm = $state(readCloseAllConfirm());

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
  get mdView(): MdViewMode {
    return mdView;
  },
  setMdView(value: MdViewMode): void {
    if (!MD_VIEW_MODES.includes(value)) return;
    mdView = value;
    try {
      localStorage.setItem(MD_VIEW_KEY, value);
    } catch {
      // Keep the in-memory preference.
    }
  },
  /** Next mode in edit → split → preview → edit (palette cycle). */
  cycleMdView(): MdViewMode {
    const next = MD_VIEW_MODES[(MD_VIEW_MODES.indexOf(mdView) + 1) % MD_VIEW_MODES.length];
    this.setMdView(next);
    return next;
  },
  /** Whether close-all asks before closing while files have unsaved edits. */
  get closeAllConfirm(): boolean {
    return closeAllConfirm;
  },
  setCloseAllConfirm(value: boolean): void {
    closeAllConfirm = value;
    try {
      localStorage.setItem(CLOSE_ALL_CONFIRM_KEY, value ? "1" : "0");
    } catch {
      // Keep the in-memory preference.
    }
  },
};
