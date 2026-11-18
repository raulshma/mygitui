/**
 * ShortcutsOverlay (`?` cheatsheet) tests: the grouped read-only list with
 * current bindings, the filter, dismiss paths (Escape, click-outside) and
 * focus return. Rendered directly; the store is toggled around each test.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { tick } from "svelte";
import ShortcutsOverlay from "./ShortcutsOverlay.svelte";
import {
  __resetKeybindsForTests,
  setBinding,
} from "$lib/palette/keybinds";
import {
  closeShortcuts,
  isShortcutsOpen,
  openShortcuts,
} from "./shortcuts.svelte";

beforeEach(() => {
  __resetKeybindsForTests(localStorage);
});

afterEach(() => {
  closeShortcuts();
  cleanup();
  __resetKeybindsForTests();
});

describe("ShortcutsOverlay", () => {
  it("renders nothing while closed", () => {
    render(ShortcutsOverlay);
    expect(screen.queryByRole("dialog", { name: "Keyboard shortcuts" })).toBeNull();
  });

  it("lists commands grouped by section with their bindings and unbound dashes", async () => {
    openShortcuts();
    render(ShortcutsOverlay);
    await tick();

    expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeTruthy();
    expect(screen.getByRole("region", { name: "Repo commands" })).toBeTruthy();
    // A bound command shows its formatted combo (F5 default for refresh).
    expect(screen.getByText("Refresh repository status")).toBeTruthy();
    expect(screen.getByText("F5")).toBeTruthy();
    // An unbound command shows the em dash placeholder.
    const unbound = screen.getByText("Find in file…");
    expect(unbound.parentElement!.textContent).toContain("—");
  });

  it("reflects overrides (setBinding) at open time", async () => {
    setBinding("repo.refresh", "ctrl+alt+r");
    openShortcuts();
    render(ShortcutsOverlay);
    await tick();
    expect(screen.queryByText("F5")).toBeNull();
    expect(screen.getByText("Ctrl+Alt+R")).toBeTruthy();
  });

  it("filters per keystroke and shows an empty state", async () => {
    openShortcuts();
    render(ShortcutsOverlay);
    await tick();

    await fireEvent.input(screen.getByLabelText("Filter shortcuts"), {
      target: { value: "palette" },
    });
    expect(screen.getByText("Toggle command palette")).toBeTruthy();
    expect(screen.queryByText("Refresh repository status")).toBeNull();

    await fireEvent.input(screen.getByLabelText("Filter shortcuts"), {
      target: { value: "zzz-none" },
    });
    expect(screen.getByText(/No shortcuts match/)).toBeTruthy();
  });

  it("closes on Escape and returns focus to the trigger element", async () => {
    render(ShortcutsOverlay);
    const trigger = document.createElement("button");
    trigger.textContent = "opener";
    document.body.append(trigger);
    trigger.focus();
    expect(document.activeElement).toBe(trigger);

    openShortcuts();
    await tick();
    // Opening moved focus into the filter input.
    expect(document.activeElement).toBe(screen.getByLabelText("Filter shortcuts"));

    await fireEvent.keyDown(window, { key: "Escape" });
    await tick();
    expect(isShortcutsOpen()).toBe(false);
    expect(screen.queryByRole("dialog")).toBeNull();
    // Focus is handed back to the pre-open element.
    expect(document.activeElement).toBe(trigger);
    trigger.remove();
  });

  it("Escape first clears the filter, then a second Escape closes", async () => {
    openShortcuts();
    render(ShortcutsOverlay);
    await tick();
    const input = screen.getByLabelText("Filter shortcuts") as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "palette" } });

    await fireEvent.keyDown(input, { key: "Escape" });
    await tick();
    expect(isShortcutsOpen()).toBe(true); // filter cleared, overlay stays
    expect(input.value).toBe("");

    await fireEvent.keyDown(window, { key: "Escape" });
    await tick();
    expect(isShortcutsOpen()).toBe(false);
  });

  it("closes on an outside (scrim) click", async () => {
    openShortcuts();
    render(ShortcutsOverlay);
    await tick();

    await fireEvent.click(screen.getByRole("presentation"));
    await tick();
    expect(isShortcutsOpen()).toBe(false);
  });
});
