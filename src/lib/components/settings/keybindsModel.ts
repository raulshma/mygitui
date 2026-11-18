/**
 * Pure logic for the Settings keybind editor's capture flow (no runes, no
 * DOM): the conflict a candidate binding would create and the inline
 * warning line shown before it is applied.
 */

import type { Command } from "$lib/palette/commands";
import { findConflicts, formatCombo } from "$lib/palette/keybinds";

/** A candidate binding that another command already holds. */
export interface BindingConflict {
  /** The canonical combo that collided. */
  combo: string;
  /** Titles of the OTHER commands holding the combo (sorted, unique). */
  others: string[];
}

/**
 * The conflict `combo` would create for `commandId` when applied on top of
 * `bindings`, or `null` when none — computed with the engine's own
 * {@link findConflicts} over the candidate map, so the editor's notion of a
 * conflict can never drift from the matcher's.
 */
export function bindingConflict(
  bindings: Record<string, string>,
  commands: readonly Command[],
  commandId: string,
  combo: string,
): BindingConflict | null {
  const candidate = { ...bindings, [commandId]: combo };
  const hit = findConflicts(candidate).find((conflict) => conflict.combo === combo);
  if (hit === undefined) return null;
  const others = [
    ...new Set(
      hit.commandIds
        .filter((id) => id !== commandId)
        .map((id) => commands.find((command) => command.id === id)?.title ?? id),
    ),
  ].sort();
  return { combo, others };
}

/** The inline warning line: "Ctrl+K is also bound to X and Y". */
export function conflictWarning(conflict: BindingConflict): string {
  const [first, ...rest] = conflict.others;
  if (first === undefined) return "";
  const list = rest.length === 0 ? first : `${first} and ${rest.join(" and ")}`;
  return `${formatCombo(conflict.combo)} is already bound to ${list}`;
}
