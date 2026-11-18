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
  /** Effective combo ("ctrl+shift+p"), or `null` when unbound. */
  combo: string | null;
  /** Display form ("Ctrl+Shift+P"), or `null` when unbound. */
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
