/**
 * Shared derivation for every "all commands + their bindings" surface —
 * the Settings keybind editor and the `?` shortcuts cheatsheet — so the
 * two lists cannot diverge: one section grouping, one binding lookup,
 * one display formatting.
 *
 * The registry is listed whole (no `when` filtering): a cheatsheet should
 * document repo commands even on the home view, and the editor must offer
 * them for rebinding with no repository open. Pure data in, pure data out
 * — bindings are a parameter, so tests need no DOM.
 */

import { COMMANDS, type Command } from "$lib/palette/commands";
import { formatCombo } from "$lib/palette/keybinds";

/** One command row: the command plus its effective binding, pre-formatted. */
export interface ShortcutEntry {
  command: Command;
  /** Effective combo ("ctrl+k"), or `null` when unbound. */
  combo: string | null;
  /** Display form ("Ctrl+K"), or `null` when unbound. */
  display: string | null;
}

/** One group: a section heading and its entries, in registry order. */
export interface ShortcutSection {
  name: string;
  entries: ShortcutEntry[];
}

/**
 * Groups `commands` by section, attaching each command's effective combo
 * from `bindings` (defaults + overrides — pass `currentBindings()`).
 */
export function shortcutSections(
  bindings: Record<string, string>,
  commands: readonly Command[] = COMMANDS,
): ShortcutSection[] {
  const sections: ShortcutSection[] = [];
  const byName = new Map<string, ShortcutSection>();
  for (const command of commands) {
    let section = byName.get(command.section);
    if (section === undefined) {
      section = { name: command.section, entries: [] };
      byName.set(command.section, section);
      sections.push(section);
    }
    const combo = bindings[command.id] ?? null;
    section.entries.push({
      command,
      combo,
      display: combo === null ? null : formatCombo(combo),
    });
  }
  return sections;
}

/** Substring match over the fields a user searches by ("" matches all). */
export function matchesShortcutQuery(entry: ShortcutEntry, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle === "") return true;
  const haystack = [
    entry.command.section,
    entry.command.title,
    ...(entry.command.keywords ?? []),
  ]
    .join(" ")
    .toLowerCase();
  return haystack.includes(needle);
}

// ---------------------------------------------------------------------------
// Surface keys (§11): non-rebindable in-component shortcuts
// ---------------------------------------------------------------------------

/**
 * Shortcuts a surface component handles itself — NOT palette commands, so
 * they have no engine binding, are not rebindable in Settings, and never
 * appear in the editor's list. They still belong in the `?` cheatsheet (§11:
 * "add `b`/`h` row actions and Ctrl+Shift+Enter; document in the `?`
 * popover"), so the overlay appends them as a final static section via
 * {@link surfaceShortcutSection}. Display formatting goes through the same
 * {@link formatCombo} as engine bindings, so mac renders ⇧⌘↵ etc.
 */
export const SURFACE_SHORTCUTS: ReadonlyArray<{
  title: string;
  combo: string;
  keywords?: string[];
}> = [
  {
    title: "Commit and push (composer)",
    combo: "ctrl+shift+enter",
    keywords: ["commit", "push", "STATUS", "composer"],
  },
  {
    title: "Blame focused file row (commit detail)",
    combo: "b",
    keywords: ["blame", "row", "annotate"],
  },
  {
    title: "File history for focused row (commit detail)",
    combo: "h",
    keywords: ["history", "row", "log"],
  },
];

/** Section heading the static rows render under in the cheatsheet. */
export const SURFACE_SHORTCUTS_SECTION = "Row & composer keys";

/**
 * The static section for the cheatsheet: {@link SURFACE_SHORTCUTS} shaped as
 * {@link ShortcutEntry}s (synthetic read-only commands — `run` is a no-op;
 * these rows are display-only and the Settings editor never renders them).
 */
export function surfaceShortcutSection(): ShortcutSection {
  const entries = SURFACE_SHORTCUTS.map((row) => ({
    command: {
      id: `surface.${row.combo}`,
      title: row.title,
      section: SURFACE_SHORTCUTS_SECTION,
      keywords: row.keywords,
      run: () => {},
    } satisfies Command,
    combo: row.combo,
    display: formatCombo(row.combo),
  }));
  return { name: SURFACE_SHORTCUTS_SECTION, entries };
}
