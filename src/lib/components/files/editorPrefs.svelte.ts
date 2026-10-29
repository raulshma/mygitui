/**
 * File-editor preferences (module singleton): line-wrap toggle, persisted
 * under `mygitui.files.wrap` like the diff viewer's `mygitui.diff.wrap`;
 * markdown preview layout (`mygitui.files.md-view`) — the segmented
 * Edit/Split/Preview choice, remembered across sessions per the app.
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
};
