/**
 * Unit tests for the M12 custom-action shortcut model: the event → combo
 * formatter, the display formatter, duplicate detection and the global
 * keydown match.
 */

import { describe, expect, it } from "vitest";
import {
  comboFromKeyboardEvent,
  defForEvent,
  formatShortcut,
  isBoundShortcut,
  shortcutHolders,
  type ShortcutDef,
} from "./shortcutModel";

/** Synthetic KeyboardEvent-like input (jsdom KeyboardEvent + target). */
function keyEvent(
  init: {
    key: string;
    ctrlKey?: boolean;
    altKey?: boolean;
    shiftKey?: boolean;
    metaKey?: boolean;
    editable?: boolean;
    capture?: boolean;
  },
): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key: init.key,
    ctrlKey: init.ctrlKey ?? false,
    altKey: init.altKey ?? false,
    shiftKey: init.shiftKey ?? false,
    metaKey: init.metaKey ?? false,
    bubbles: true,
  });
  if (init.editable || init.capture) {
    const holder = document.createElement(init.capture ? "span" : "input");
    if (init.capture) holder.setAttribute("data-shortcut-capture", "");
    const inner = document.createElement("input");
    holder.appendChild(inner);
    document.body.appendChild(holder);
    Object.defineProperty(event, "target", { value: inner });
  }
  return event;
}

const DEFS: ShortcutDef[] = [
  { id: "1", name: "Run tests", shortcut: "ctrl+shift+r" },
  { id: "2", name: "Deploy" },
  { id: "3", name: "Lint", shortcut: "f9" },
];

describe("comboFromKeyboardEvent (key → string formatter)", () => {
  it("records Ctrl+Shift+R as the canonical ctrl+shift+r", () => {
    const combo = comboFromKeyboardEvent(
      keyEvent({ key: "R", ctrlKey: true, shiftKey: true }),
      "pc",
    );
    expect(combo).toBe("ctrl+shift+r");
  });

  it("orders modifiers ctrl < alt < shift regardless of press state", () => {
    expect(
      comboFromKeyboardEvent(keyEvent({ key: "k", shiftKey: true, ctrlKey: true }), "pc"),
    ).toBe("ctrl+shift+k");
    expect(
      comboFromKeyboardEvent(keyEvent({ key: "F5", ctrlKey: true, altKey: true }), "pc"),
    ).toBe("ctrl+alt+f5");
  });

  it("rejects bare modifier presses and stray meta on pc", () => {
    expect(comboFromKeyboardEvent(keyEvent({ key: "Shift", shiftKey: true }), "pc")).toBeNull();
    expect(comboFromKeyboardEvent(keyEvent({ key: "Control", ctrlKey: true }), "pc")).toBeNull();
    expect(
      comboFromKeyboardEvent(keyEvent({ key: "r", metaKey: true, ctrlKey: true }), "pc"),
    ).toBeNull();
  });

  it("on mac, Cmd plays Ctrl's role", () => {
    expect(
      comboFromKeyboardEvent(keyEvent({ key: "r", metaKey: true, shiftKey: true }), "mac"),
    ).toBe("ctrl+shift+r");
  });
});

describe("formatShortcut", () => {
  it("renders the display form", () => {
    expect(formatShortcut("ctrl+shift+r")).toBe("Ctrl+Shift+R");
    expect(formatShortcut("f9")).toBe("F9");
  });
});

describe("isBoundShortcut", () => {
  it("treats undefined/empty as unbound", () => {
    expect(isBoundShortcut(undefined)).toBe(false);
    expect(isBoundShortcut("")).toBe(false);
    expect(isBoundShortcut("ctrl+r")).toBe(true);
  });
});

describe("shortcutHolders (duplicate reject)", () => {
  it("finds other defs holding the combo, excluding self", () => {
    expect(shortcutHolders("ctrl+shift+r", DEFS)).toEqual([DEFS[0]]);
    expect(shortcutHolders("ctrl+shift+r", DEFS, "1")).toEqual([]);
    expect(shortcutHolders("ctrl+alt+t", DEFS)).toEqual([]);
  });
});

describe("defForEvent (match logic)", () => {
  it("matches a stored combo to its def", () => {
    expect(defForEvent(keyEvent({ key: "R", ctrlKey: true, shiftKey: true }), DEFS, "pc"))?.toBe(
      DEFS[0],
    );
    expect(defForEvent(keyEvent({ key: "F9" }), DEFS, "pc")).toBe(DEFS[2]);
  });

  it("returns null for unbound combos", () => {
    expect(defForEvent(keyEvent({ key: "x", ctrlKey: true }), DEFS, "pc")).toBeNull();
  });

  it("ignores modifier-less combos while typing in an editable field", () => {
    expect(defForEvent(keyEvent({ key: "f", editable: true }), DEFS, "pc")).toBeNull();
    // …but still honors modifier combos there.
    expect(
      defForEvent(keyEvent({ key: "R", ctrlKey: true, shiftKey: true, editable: true }), DEFS, "pc"),
    ).toBe(DEFS[0]);
  });

  it("skips events aimed at a shortcut-capture field", () => {
    expect(
      defForEvent(keyEvent({ key: "R", ctrlKey: true, shiftKey: true, capture: true }), DEFS, "pc"),
    ).toBeNull();
  });
});
