/**
 * Shared "commands + bindings + sections" listing (shortcutSections.ts) —
 * the derivation both the Settings keybind editor and the `?` cheatsheet
 * render. Covers grouping/order, binding lookup + display formatting, and
 * the substring filter.
 */
import { describe, expect, it } from "vitest";
import { COMMANDS } from "./commands";
import { DEFAULT_BINDINGS } from "./keybinds";
import {
  matchesShortcutQuery,
  shortcutSections,
  surfaceShortcutSection,
  SURFACE_SHORTCUTS,
  SURFACE_SHORTCUTS_SECTION,
} from "./shortcutSections";

/** Tiny command + binding fixtures (registry shapes, no run bodies needed). */
const commands = [
  { id: "a.one", title: "First command", section: "Alpha", run: () => {} },
  { id: "a.two", title: "Second command", section: "Alpha", keywords: ["second kw"], run: () => {} },
  { id: "b.one", title: "Beta command", section: "Beta", run: () => {} },
] as unknown as typeof COMMANDS;

describe("shortcutSections", () => {
  it("groups by section in first-seen order, entries in registry order", () => {
    const sections = shortcutSections({}, commands);
    expect(sections.map((section) => section.name)).toEqual(["Alpha", "Beta"]);
    expect(sections[0]!.entries.map((entry) => entry.command.id)).toEqual(["a.one", "a.two"]);
    expect(sections[1]!.entries.map((entry) => entry.command.id)).toEqual(["b.one"]);
  });

  it("attaches the effective combo and its display form", () => {
    const sections = shortcutSections(
      { "a.one": "ctrl+shift+p", "b.one": "f5" },
      commands,
    );
    expect(sections[0]!.entries[0]).toMatchObject({ combo: "ctrl+shift+p", display: "Ctrl+Shift+P" });
    expect(sections[1]!.entries[0]).toMatchObject({ combo: "f5", display: "F5" });
  });

  it("leaves unbound commands in the list with null combo/display", () => {
    const sections = shortcutSections({}, commands);
    expect(sections[0]!.entries[0]).toMatchObject({ combo: null, display: null });
  });

  it("covers the whole live registry against the real defaults", () => {
    const sections = shortcutSections(DEFAULT_BINDINGS);
    const total = sections.reduce((sum, section) => sum + section.entries.length, 0);
    expect(total).toBe(COMMANDS.length);
    const names = sections.map((section) => section.name);
    expect(names).toContain("Help");
    // help.shortcuts rides the shift+? default and formats as Shift+?.
    const help = sections
      .find((section) => section.name === "Help")!
      .entries.find((entry) => entry.command.id === "help.shortcuts");
    expect(help).toMatchObject({ combo: "shift+?", display: "Shift+?" });
  });
});

describe("matchesShortcutQuery", () => {
  const sections = shortcutSections({}, commands);
  const first = sections[0]!.entries[0]!;
  const second = sections[0]!.entries[1]!;

  it("matches everything on empty/blank queries", () => {
    expect(matchesShortcutQuery(first, "")).toBe(true);
    expect(matchesShortcutQuery(first, "   ")).toBe(true);
  });

  it("matches title, section and keywords case-insensitively", () => {
    expect(matchesShortcutQuery(first, "first")).toBe(true);
    expect(matchesShortcutQuery(first, "alpha")).toBe(true);
    expect(matchesShortcutQuery(second, "second kw")).toBe(true);
    expect(matchesShortcutQuery(second, "ALPHA SECOND")).toBe(true);
    // No cross-entry match: "beta" is only in the other section/title.
    expect(matchesShortcutQuery(first, "beta")).toBe(false);
    expect(matchesShortcutQuery(second, "nomatch")).toBe(false);
  });
});

describe("surfaceShortcutSection (§11 static rows)", () => {
  it("documents the non-rebindable surface keys under one section", () => {
    const section = surfaceShortcutSection();
    expect(section.name).toBe(SURFACE_SHORTCUTS_SECTION);
    expect(section.entries.map((entry) => entry.combo)).toEqual([
      "ctrl+shift+enter",
      "b",
      "h",
    ]);
  });

  it("formats display combos through the shared formatter (mac-safe)", () => {
    const section = surfaceShortcutSection();
    // `enter` renders as the ↵ glyph, like every other Kbd hint.
    expect(section.entries[0]).toMatchObject({
      display: "Ctrl+Shift+↵",
      command: expect.objectContaining({ title: "Commit and push (composer)" }),
    });
    expect(section.entries[1]).toMatchObject({ display: "B" });
    expect(section.entries[2]).toMatchObject({ display: "H" });
  });

  it("stays in sync with the SURFACE_SHORTCUTS data (one row each)", () => {
    const section = surfaceShortcutSection();
    expect(section.entries.length).toBe(SURFACE_SHORTCUTS.length);
    // Synthetic commands are display-only: no run side effects, own section.
    for (const entry of section.entries) {
      expect(entry.command.section).toBe(SURFACE_SHORTCUTS_SECTION);
      expect(() => entry.command.run({ repoId: null, root: null })).not.toThrow();
    }
  });

  it("is filterable through the shared query matcher", () => {
    const [commitPush, blame, history] = surfaceShortcutSection().entries;
    expect(matchesShortcutQuery(commitPush!, "composer")).toBe(true);
    expect(matchesShortcutQuery(blame!, "blame")).toBe(true);
    expect(matchesShortcutQuery(history!, "log")).toBe(true);
    expect(matchesShortcutQuery(commitPush!, "nomatch")).toBe(false);
  });
});
