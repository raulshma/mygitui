/**
 * Keybind-editor capture-flow logic (keybindsModel.ts): conflict detection
 * over a candidate binding map (via the engine's findConflicts) and the
 * inline warning line formatting.
 */
import { describe, expect, it } from "vitest";
import { DEFAULT_BINDINGS, currentBindings } from "$lib/palette/keybinds";
import { COMMANDS } from "$lib/palette/commands";
import {
  bindingConflict,
  conflictWarning,
} from "./keybindsModel";

describe("bindingConflict", () => {
  it("returns null when the combo is free", () => {
    expect(
      bindingConflict(currentBindings(), COMMANDS, "app.about", "ctrl+alt+f13"),
    ).toBeNull();
  });

  it("returns null when the command itself holds the combo (no-op rebind)", () => {
    // Re-binding a command to its own current combo is not a conflict.
    expect(
      bindingConflict(currentBindings(), COMMANDS, "app.palette", DEFAULT_BINDINGS["app.palette"]!),
    ).toBeNull();
  });

  it("reports the other commands holding the combo, with titles", () => {
    const candidate = { "x.one": "ctrl+alt+k", "x.two": "ctrl+alt+k", "x.solo": "ctrl+alt+j" };
    const commands = [
      { id: "x.one", title: "First", section: "T", run: () => {} },
      { id: "x.two", title: "Second", section: "T", run: () => {} },
      { id: "x.solo", title: "Solo", section: "T", run: () => {} },
    ];
    const warning = bindingConflict(candidate, commands, "x.three", "ctrl+alt+k");
    expect(warning).not.toBeNull();
    expect(warning!.combo).toBe("ctrl+alt+k");
    expect(warning!.others).toEqual(["First", "Second"]);
    // Unknown ids degrade to the id itself.
    const raw = bindingConflict(candidate, [], "x.three", "ctrl+alt+k");
    expect(raw!.others).toEqual(["x.one", "x.two"]);
  });

  it("ignores pre-existing collisions elsewhere in the map", () => {
    const candidate = { "x.a": "ctrl+k", "x.b": "ctrl+k" };
    // Rebinding x.c to a free combo does not surface the x.a/x.b collision.
    expect(bindingConflict(candidate, [], "x.c", "ctrl+alt+m")).toBeNull();
  });
});

describe("conflictWarning", () => {
  it("formats one other command", () => {
    expect(
      conflictWarning({ combo: "ctrl+alt+k", others: ["Refresh repository status"] }),
    ).toBe("Ctrl+Alt+K is already bound to Refresh repository status");
  });

  it("joins several other commands with 'and'", () => {
    expect(
      conflictWarning({ combo: "ctrl+k", others: ["B", "A"] }),
    ).toBe("Ctrl+K is already bound to B and A");
  });

  it("renders the empty case as an empty string", () => {
    expect(conflictWarning({ combo: "ctrl+k", others: [] })).toBe("");
  });
});
