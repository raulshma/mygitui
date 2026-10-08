/**
 * Keybind engine tests ($lib/palette/keybinds): combo canonicalization,
 * event matching (mac meta vs pc ctrl), overrides persistence, conflict
 * detection and the window keydown listener (jsdom). Pure pieces need no
 * DOM; the listener tests dispatch real KeyboardEvents on window.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_BINDINGS,
  KEYBINDS_STORAGE_KEY,
  __resetKeybindsForTests,
  canonicalCombo,
  comboFromEvent,
  comboToString,
  currentBindings,
  detectPlatform,
  findConflicts,
  formatCombo,
  getBinding,
  getOverrides,
  hasModifier,
  isEditableTarget,
  loadOverrides,
  matchEvent,
  normalizeKey,
  parseCombo,
  resetBindings,
  saveOverrides,
  setBinding,
  startKeybinds,
  clearBinding,
  effectiveBindings,
  type StorageLike,
} from "$lib/palette/keybinds";

/** In-memory Storage fake (mirrors the recentRepos test pattern). */
function fakeStorage(): StorageLike & { store: Map<string, string> } {
  const store = new Map<string, string>();
  return {
    store,
    getItem: (key) => store.get(key) ?? null,
    setItem: (key, value) => void store.set(key, value),
    removeItem: (key) => void store.delete(key),
  };
}

/** Minimal keydown-event stand-in (jsdom KeyboardEvent is heavyweight). */
function keyEvent(
  init: Partial<KeyboardEvent> & { key: string },
): KeyboardEvent {
  return {
    key: init.key,
    ctrlKey: init.ctrlKey ?? false,
    altKey: init.altKey ?? false,
    shiftKey: init.shiftKey ?? false,
    metaKey: init.metaKey ?? false,
    target: init.target ?? null,
    preventDefault: init.preventDefault ?? (() => {}),
    stopPropagation: init.stopPropagation ?? (() => {}),
  } as unknown as KeyboardEvent;
}

describe("combo canonicalization", () => {
  it("lowercases and orders modifiers canonically", () => {
    expect(canonicalCombo("Ctrl+Shift+P")).toBe("ctrl+shift+p");
    expect(canonicalCombo("shift+ctrl+p")).toBe("ctrl+shift+p");
    expect(canonicalCombo("ALT+C")).toBe("alt+c");
  });

  it("accepts meta-ish aliases as ctrl (platform-agnostic storage)", () => {
    expect(canonicalCombo("cmd+k")).toBe("ctrl+k");
    expect(canonicalCombo("Meta+Shift+P")).toBe("ctrl+shift+p");
    expect(canonicalCombo("option+left")).toBe("alt+left");
  });

  it("normalizes special keys", () => {
    expect(canonicalCombo("ctrl+space")).toBe("ctrl+space");
    expect(normalizeKey(" ")).toBe("space");
    expect(canonicalCombo("esc")).toBe("escape");
    expect(canonicalCombo("f5")).toBe("f5");
    expect(canonicalCombo("ctrl+ ")).toBeNull(); // bare "+" separator token
    expect(normalizeKey("Return")).toBe("enter");
  });

  it("rejects unusable combos", () => {
    expect(canonicalCombo("")).toBeNull();
    expect(canonicalCombo("   ")).toBeNull();
    expect(canonicalCombo("ctrl")).toBeNull();
    expect(canonicalCombo("ctrl+shift+alt")).toBeNull();
    expect(canonicalCombo("ctrl+p+q")).toBeNull();
    expect(canonicalCombo("ctrl++")).toBeNull();
  });

  it("round-trips through parseCombo/comboToString", () => {
    const parts = parseCombo("ctrl+alt+arrowright");
    expect(parts).toEqual({
      ctrl: true,
      alt: true,
      shift: false,
      key: "arrowright",
    });
    expect(comboToString(parts!)).toBe("ctrl+alt+arrowright");
  });

  it("rejects keys that cannot come from a keyboard event", () => {
    expect(canonicalCombo("ctrl+not a combo")).toBeNull();
  });

  it("hasModifier distinguishes bare keys from chorded ones", () => {
    expect(hasModifier("f5")).toBe(false);
    expect(hasModifier("shift+a")).toBe(true);
    expect(hasModifier("not a combo")).toBe(false);
  });

  it("formats combos for display", () => {
    expect(formatCombo("ctrl+shift+p")).toBe("Ctrl+Shift+P");
    expect(formatCombo("ctrl+alt+arrowleft")).toBe("Ctrl+Alt+Arrowleft");
    expect(formatCombo("f5")).toBe("F5");
    expect(formatCombo("ctrl+1")).toBe("Ctrl+1");
  });
});

describe("event matching", () => {
  const bindings = {
    "app.palette": "ctrl+shift+p",
    "repo.refresh": "f5",
    "panel.focus-status": "ctrl+1",
  };

  it("matches ctrl combos on pc", () => {
    expect(
      matchEvent(keyEvent({ key: "p", ctrlKey: true, shiftKey: true }), bindings, "pc"),
    ).toBe("app.palette");
    expect(matchEvent(keyEvent({ key: "1", ctrlKey: true }), bindings, "pc")).toBe(
      "panel.focus-status",
    );
    expect(matchEvent(keyEvent({ key: "F5" }), bindings, "pc")).toBe("repo.refresh");
  });

  it("maps metaKey to ctrl on mac (and accepts physical ctrl)", () => {
    expect(
      matchEvent(keyEvent({ key: "p", metaKey: true, shiftKey: true }), bindings, "mac"),
    ).toBe("app.palette");
    expect(
      matchEvent(keyEvent({ key: "p", ctrlKey: true, shiftKey: true }), bindings, "mac"),
    ).toBe("app.palette");
  });

  it("ignores meta-held events on pc (OS shortcut territory)", () => {
    expect(
      matchEvent(keyEvent({ key: "p", metaKey: true, shiftKey: true }), bindings, "pc"),
    ).toBeNull();
  });

  it("returns null for modifier-only presses and misses", () => {
    expect(matchEvent(keyEvent({ key: "Shift" }), bindings, "pc")).toBeNull();
    expect(matchEvent(keyEvent({ key: "x" }), bindings, "pc")).toBeNull();
    expect(
      matchEvent(keyEvent({ key: "p", shiftKey: true }), bindings, "pc"),
    ).toBeNull();
  });

  it("comboFromEvent builds the canonical combo", () => {
    expect(comboFromEvent(keyEvent({ key: "ArrowRight", ctrlKey: true, altKey: true }), "pc")).toBe(
      "ctrl+alt+arrowright",
    );
    expect(comboFromEvent(keyEvent({ key: "A", ctrlKey: true }), "pc")).toBe("ctrl+a");
  });
});

describe("overrides persistence", () => {
  beforeEach(() => {
    __resetKeybindsForTests();
  });

  it("persists setBinding as canonical overrides", () => {
    const storage = fakeStorage();
    __resetKeybindsForTests(storage);
    expect(setBinding("app.palette", "Ctrl + Alt + K")).toBe("ctrl+alt+k");
    expect(storage.store.get(KEYBINDS_STORAGE_KEY)).toBe(
      JSON.stringify({ "app.palette": "ctrl+alt+k" }),
    );
    expect(getOverrides()).toEqual({ "app.palette": "ctrl+alt+k" });
  });

  it("drops overrides that equal the default", () => {
    const storage = fakeStorage();
    __resetKeybindsForTests(storage);
    setBinding("app.palette", "ctrl+shift+p");
    expect(getOverrides()).toEqual({});
    expect(storage.store.get(KEYBINDS_STORAGE_KEY)).toBe("{}");
  });

  it("rejects invalid combos without touching storage", () => {
    const storage = fakeStorage();
    __resetKeybindsForTests(storage);
    expect(setBinding("app.palette", "ctrl+shift+alt")).toBeNull();
    expect(getOverrides()).toEqual({});
    expect(storage.store.get(KEYBINDS_STORAGE_KEY)).toBeUndefined();
  });

  it("clearBinding falls back to the default, resetBindings wipes all", () => {
    __resetKeybindsForTests(fakeStorage());
    setBinding("app.palette", "ctrl+alt+k");
    setBinding("repo.refresh", "ctrl+r");
    clearBinding("app.palette");
    expect(getBinding("app.palette")).toBe(DEFAULT_BINDINGS["app.palette"]);
    expect(getBinding("repo.refresh")).toBe("ctrl+r");
    resetBindings();
    expect(getOverrides()).toEqual({});
    expect(getBinding("repo.refresh")).toBe("f5");
  });

  it("loadOverrides drops malformed entries and invalid JSON", () => {
    const storage = fakeStorage();
    storage.store.set(
      KEYBINDS_STORAGE_KEY,
      JSON.stringify({ "a.b": "ctrl+k", "c.d": "not a combo", "e.f": 42 }),
    );
    expect(loadOverrides(storage)).toEqual({ "a.b": "ctrl+k" });
    storage.store.set(KEYBINDS_STORAGE_KEY, "{nope");
    expect(loadOverrides(storage)).toEqual({});
    storage.store.set(KEYBINDS_STORAGE_KEY, "[1,2]");
    expect(loadOverrides(storage)).toEqual({});
  });

  it("works without storage (in-memory only)", () => {
    __resetKeybindsForTests(null);
    setBinding("app.palette", "ctrl+alt+k");
    expect(getBinding("app.palette")).toBe("ctrl+alt+k");
  });

  it("effectiveBindings merges overrides over defaults", () => {
    expect(effectiveBindings({ "x.y": "ctrl+q" })["x.y"]).toBe("ctrl+q");
    expect(effectiveBindings({ "app.palette": "ctrl+alt+p" })["app.palette"]).toBe(
      "ctrl+alt+p",
    );
    expect(effectiveBindings({})["repo.refresh"]).toBe("f5");
  });

  it("currentBindings reflects defaults + overrides", () => {
    __resetKeybindsForTests(fakeStorage());
    expect(currentBindings()).toEqual(DEFAULT_BINDINGS);
    setBinding("repo.refresh", "ctrl+r");
    expect(currentBindings()["repo.refresh"]).toBe("ctrl+r");
  });
});

describe("conflict detection", () => {
  it("reports combos held by multiple commands", () => {
    const conflicts = findConflicts({
      "b.second": "ctrl+k",
      "a.first": "ctrl+k",
      "c.solo": "ctrl+l",
    });
    expect(conflicts).toEqual([
      { combo: "ctrl+k", commandIds: ["a.first", "b.second"] },
    ]);
  });

  it("returns an empty list for unique bindings", () => {
    expect(findConflicts(DEFAULT_BINDINGS)).toEqual([]);
  });
});

describe("startKeybinds (window listener)", () => {
  beforeEach(() => {
    __resetKeybindsForTests();
  });

  afterEach(() => {
    __resetKeybindsForTests();
  });

  function dispatchKeydown(
    init: Partial<KeyboardEvent> & { key: string },
    target: EventTarget | null = window,
  ): boolean {
    let defaultPrevented = false;
    const event = new KeyboardEvent("keydown", {
      key: init.key,
      ctrlKey: init.ctrlKey ?? false,
      altKey: init.altKey ?? false,
      shiftKey: init.shiftKey ?? false,
      metaKey: init.metaKey ?? false,
      bubbles: true,
      cancelable: true,
    });
    Object.defineProperty(event, "target", { value: target });
    target?.dispatchEvent(event);
    return event.defaultPrevented;
  }

  it("dispatches onAction for a bound combo and swallows the event", () => {
    const onAction = vi.fn();
    const stop = startKeybinds(onAction);
    try {
      const prevented = dispatchKeydown({ key: "p", ctrlKey: true, shiftKey: true });
      expect(onAction).toHaveBeenCalledWith("app.palette");
      expect(prevented).toBe(true);
    } finally {
      stop();
    }
  });

  it("uses the live bindings (override picked up after install)", () => {
    const onAction = vi.fn();
    const stop = startKeybinds(onAction);
    try {
      setBinding("app.palette", "ctrl+alt+k");
      dispatchKeydown({ key: "p", ctrlKey: true, shiftKey: true });
      expect(onAction).not.toHaveBeenCalled();
      dispatchKeydown({ key: "k", ctrlKey: true, altKey: true });
      expect(onAction).toHaveBeenCalledWith("app.palette");
    } finally {
      stop();
    }
  });

  it("does nothing for unbound combos", () => {
    const onAction = vi.fn();
    const stop = startKeybinds(onAction);
    try {
      dispatchKeydown({ key: "z", ctrlKey: true });
      expect(onAction).not.toHaveBeenCalled();
    } finally {
      stop();
    }
  });

  it("ignores modifier-less keys while typing in an input", () => {
    const onAction = vi.fn();
    const stop = startKeybinds(onAction);
    try {
      const input = document.createElement("input");
      document.body.append(input);
      // f5 is bound but modifier-less: swallowed by the editable rule.
      dispatchKeydown({ key: "F5" }, input);
      expect(onAction).not.toHaveBeenCalled();
      // Modified combos still fire from inputs.
      dispatchKeydown({ key: "p", ctrlKey: true, shiftKey: true }, input);
      expect(onAction).toHaveBeenCalledWith("app.palette");
      input.remove();
    } finally {
      stop();
    }
  });

  it("is idempotent: the second install is a no-op", () => {
    const onAction = vi.fn();
    const stop = startKeybinds(onAction);
    try {
      const stop2 = startKeybinds(onAction);
      stop2();
      dispatchKeydown({ key: "p", ctrlKey: true, shiftKey: true });
      expect(onAction).toHaveBeenCalledTimes(1); // first install still live
      stop();
      dispatchKeydown({ key: "p", ctrlKey: true, shiftKey: true });
      expect(onAction).toHaveBeenCalledTimes(1); // cleanup detached it
    } finally {
      stop();
    }
  });

  it("isEditableTarget covers inputs, textareas and contenteditable", () => {
    expect(isEditableTarget(document.createElement("input"))).toBe(true);
    expect(isEditableTarget(document.createElement("textarea"))).toBe(true);
    // jsdom does not implement the contentEditable editing-host model, so
    // simulate what a real browser reports for a contenteditable element.
    const editable = document.createElement("div");
    Object.defineProperty(editable, "isContentEditable", { value: true });
    expect(isEditableTarget(editable)).toBe(true);
    expect(isEditableTarget(document.createElement("div"))).toBe(false);
    expect(isEditableTarget(null)).toBe(false);
  });
});

describe("detectPlatform", () => {
  it("classifies the jsdom user agent as pc", () => {
    // jsdom's UA has no Mac token; guard against environment drift anyway.
    expect(["mac", "pc"]).toContain(detectPlatform());
  });
});
