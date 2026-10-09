/**
 * Pure model for M12 custom-action keyboard shortcuts.
 *
 * A shortcut is stored on an `ActionDef` in the canonical combo format the
 * palette keybind engine uses (`"ctrl+shift+r"` — lowercase modifiers in
 * ctrl < alt < shift order, then the key), so both systems stay consistent.
 * This module reuses the keybinds combo logic and adds the two action-
 * specific pieces:
 *   - {@link shortcutHolders} — duplicate detection for the capture field
 *     (the same combo must not be bound to two actions);
 *   - {@link defForEvent} — the global keydown match: the first def whose
 *     stored shortcut equals the event's combo. Typing is never hijacked:
 *     modifier-less combos are ignored while the focus is in an editable
 *     field, and any keydown aimed at a shortcut-capture field is skipped
 *     (the capture field itself owns those events).
 */

import {
  comboFromEvent,
  hasModifier,
  isEditableTarget,
  type Platform,
} from "$lib/palette/keybinds";

/** Minimal def surface this module needs (structural subset of ActionDef). */
export interface ShortcutDef {
  id: string;
  name: string;
  /** Canonical combo (`"ctrl+shift+r"`) or undefined/"" when unbound. */
  shortcut?: string;
}

/**
 * Canonical combo string for a keydown event, or `null` when the event
 * cannot be a binding (modifier-only key press, or meta held on non-mac).
 * The key→string formatter — `"Ctrl+Shift+R"` keystrokes persist as
 * `"ctrl+shift+r"`.
 */
export function comboFromKeyboardEvent(
  event: KeyboardEvent,
  platform: Platform,
): string | null {
  return comboFromEvent(event, platform);
}

/** Display form for the capture field: `"ctrl+shift+r"` → `"Ctrl+Shift+R"`. */
export function formatShortcut(combo: string): string {
  return combo
    .split("+")
    .map((token) =>
      token.length === 1
        ? token.toUpperCase()
        : token.charAt(0).toUpperCase() + token.slice(1),
    )
    .join("+");
}

/** True when the combo is a usable stored shortcut. */
export function isBoundShortcut(shortcut: string | undefined | null): boolean {
  return typeof shortcut === "string" && shortcut.trim() !== "";
}

/**
 * Defs (other than `exceptId`) already holding `combo` — the reject list for
 * the capture field. Empty means the combo is free.
 */
export function shortcutHolders(
  combo: string,
  defs: readonly ShortcutDef[],
  exceptId?: string,
): ShortcutDef[] {
  return defs.filter(
    (def) => def.id !== exceptId && def.shortcut === combo,
  );
}

/**
 * The def whose shortcut matches this keydown, or `null`. Generic over the
 * concrete def type so callers keep full `ActionDef`s back. Skips:
 *   - events that cannot form a combo (bare modifier presses);
 *   - modifier-less combos while the focus is in an editable field
 *     (typing protection — same rule as the palette keybinds);
 *   - events targeting a shortcut-capture field (those belong to the
 *     capture handler, not the global runner).
 */
export function defForEvent<T extends ShortcutDef>(
  event: KeyboardEvent,
  defs: readonly T[],
  platform: Platform,
): T | null {
  const target = event.target;
  if (
    target instanceof HTMLElement &&
    target.closest("[data-shortcut-capture]") !== null
  ) {
    return null;
  }
  const combo = comboFromEvent(event, platform);
  if (combo === null) return null;
  if (isEditableTarget(target) && !hasModifier(combo)) return null;
  return defs.find((def) => def.shortcut === combo) ?? null;
}
