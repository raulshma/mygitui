/**
 * Tests for the persisted activity-bar rail pins (`$lib/stores/railPins.svelte`):
 * parse validation, pin/unpin/toggle semantics, and the localStorage
 * round-trip (a fresh store restores the persisted pin order).
 */
import { beforeEach, describe, expect, it } from "vitest";
import { RAIL_PANELS, type PanelId } from "$lib/layout/layoutModel";
import {
  RailPinsStore,
  RAIL_PINS_KEY,
  loadRailPins,
  parseRailPins,
  railPins,
} from "./railPins.svelte";

beforeEach(() => {
  localStorage.clear();
  railPins.clear();
});

describe("parseRailPins", () => {
  it("returns empty for empty or malformed JSON", () => {
    expect(parseRailPins(null)).toEqual([]);
    expect(parseRailPins("")).toEqual([]);
    expect(parseRailPins("not json")).toEqual([]);
    expect(parseRailPins("{}")).toEqual([]);
    expect(parseRailPins("42")).toEqual([]);
  });

  it("keeps known non-default pinnable panels in order", () => {
    expect(parseRailPins(JSON.stringify(["stashes", "reflog"]))).toEqual([
      "stashes",
      "reflog",
    ]);
  });

  it("drops unknown ids, defaults, history/diff, and duplicates", () => {
    const raw = JSON.stringify([
      "stashes",
      "nope",
      "status", // default RAIL_PANEL — always shown, never pinned
      "history", // own bottom button
      "diff", // center editor
      "stashes", // duplicate
    ]);
    expect(parseRailPins(raw)).toEqual(["stashes"]);
  });
});

describe("RailPinsStore", () => {
  it("pins non-default panels in order and persists them", () => {
    const store = new RailPinsStore();
    store.pin("stashes");
    store.pin("reflog");
    expect(store.pins).toEqual(["stashes", "reflog"]);
    expect(loadRailPins()).toEqual(["stashes", "reflog"]);
  });

  it("refuses to pin defaults, history/diff, or duplicates", () => {
    const store = new RailPinsStore();
    for (const panel of [...RAIL_PANELS, "history", "diff"] as PanelId[]) {
      store.pin(panel);
    }
    store.pin("worktrees");
    store.pin("worktrees");
    expect(store.pins).toEqual(["worktrees"]);
    expect(loadRailPins()).toEqual(["worktrees"]);
  });

  it("unpins only pinned panels", () => {
    const store = new RailPinsStore();
    store.pin("worktrees");
    store.unpin("status"); // default: not in the pin list
    expect(store.pins).toEqual(["worktrees"]);
    store.unpin("worktrees");
    expect(store.pins).toEqual([]);
    expect(loadRailPins()).toEqual([]);
  });

  it("toggle flips membership", () => {
    const store = new RailPinsStore();
    store.toggle("stats");
    expect(store.has("stats")).toBe(true);
    store.toggle("stats");
    expect(store.has("stats")).toBe(false);
  });

  it("has() tracks only the pin list (defaults are not pinned)", () => {
    const store = new RailPinsStore();
    expect(store.has("stashes")).toBe(false);
    expect(store.has("status")).toBe(false); // default: shown, not pinned
    store.pin("status"); // refused — defaults are never pinned
    expect(store.has("status")).toBe(false);
  });

  it("clear resets state and storage", () => {
    const store = new RailPinsStore();
    store.pin("stashes");
    store.clear();
    expect(store.pins).toEqual([]);
    expect(localStorage.getItem(RAIL_PINS_KEY)).toBeNull();
  });

  it("a fresh store restores the persisted pin order", () => {
    railPins.pin("reflog");
    railPins.pin("stats");
    const restored = new RailPinsStore();
    expect(restored.pins).toEqual(["reflog", "stats"]);
  });

  it("tolerates corrupt storage on load", () => {
    localStorage.setItem(RAIL_PINS_KEY, "{oops");
    expect(loadRailPins()).toEqual([]);
    expect(new RailPinsStore().pins).toEqual([]);
  });
});
