/**
 * i18n groundwork (M11) — the message-catalog seam.
 *
 * Every user-visible string CAN go through `t(key, vars)`; the English
 * catalog below seeds the pattern (panel labels, common dialogs, actions).
 * `Intl` keeps doing dates/numbers (locale-aware by default) — this module
 * only owns prose.
 *
 * Adding a language = one new catalog module with the same keys plus a
 * `setLocale` registration. Missing keys fall back to English, then to the
 * key itself (never undefined in the UI).
 */

export type Locale = "en";

type Catalog = Record<string, string>;

const catalogs: Record<Locale, Catalog> = {
  en: {
    // Panels
    "panel.status": "Status",
    "panel.branches": "Branches",
    "panel.remotes": "Remotes",
    "panel.stashes": "Stashes",
    "panel.worktrees": "Worktrees",
    "panel.health": "Repo",
    "panel.submodules": "Submodules",
    "panel.reflog": "Reflog",
    "panel.undo": "Undo",
    "panel.history": "History",
    "panel.diff": "Diff",
    "panel.terminal": "Terminal",
    "panel.forge": "GitHub",
    "panel.actions": "Actions",
    "panel.stats": "Stats",

    // Common actions
    "action.refresh": "Refresh",
    "action.close": "Close",
    "action.cancel": "Cancel",
    "action.confirm": "Confirm",
    "action.create": "Create",
    "action.delete": "Delete",
    "action.stage": "Stage",
    "action.unstage": "Unstage",
    "action.discard": "Discard",
    "action.push": "Push",
    "action.pull": "Pull",
    "action.fetch": "Fetch",
    "action.checkout": "Checkout",
    "action.switch": "Switch",

    // Common dialogs
    "dialog.confirm.title": "Are you sure?",
    "dialog.deleteTag.title": "Delete tag {name}?",
    "dialog.deleteBranch.title": "Delete branch {name}?",
    "dialog.discard.title": "Discard changes to {path}?",

    // Operation feedback
    "toast.copied": "Copied",
    "toast.failed": "{label} failed: {error}",
    "toast.done": "{label} done",

    // States
    "state.loading": "Loading…",
    "state.empty": "Nothing here yet.",
    "state.error": "Something went wrong.",
  },
};

let active: Locale = "en";

/** Sets the active locale (unknown locales fall back to English). */
export function setLocale(locale: Locale): void {
  active = catalogs[locale] ? locale : "en";
}

export function getLocale(): Locale {
  return active;
}

/**
 * Translates `key` with `{placeholder}` interpolation. Falls back:
 * active catalog → English → the key itself.
 */
export function t(
  key: string,
  vars: Record<string, string | number> = {},
): string {
  const template = catalogs[active][key] ?? catalogs.en[key] ?? key;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => {
    const value = vars[name];
    return value === undefined ? match : String(value);
  });
}
