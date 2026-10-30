/**
 * Tests for the persisted header sync pins (`$lib/stores/headerPins.svelte`):
 * parse validation, pin/unpin/toggle semantics, and the localStorage
 * round-trip (a fresh store restores the persisted pin order).
 */
import { beforeEach, describe, expect, it } from "vitest";
import {
  HeaderPinsStore,
  HEADER_PINS_KEY,
  headerPins,
  loadHeaderPins,
  parseHeaderPins,
} from "./headerPins.svelte";

beforeEach(() => {
  localStorage.clear();
  headerPins.clear();
});

describe("parseHeaderPins", () => {
  it("returns empty for empty or malformed JSON", () => {
    expect(parseHeaderPins(null)).toEqual([]);
    expect(parseHeaderPins("")).toEqual([]);
    expect(parseHeaderPins("not json")).toEqual([]);
    expect(parseHeaderPins("{}")).toEqual([]);
    expect(parseHeaderPins("42")).toEqual([]);
  });

  it("keeps known sync verbs in order", () => {
    expect(parseHeaderPins(JSON.stringify(["pull", "fetch"]))).toEqual([
      "pull",
      "fetch",
    ]);
  });

  it("drops unknown ids and duplicates", () => {
    const raw = JSON.stringify([
      "pull",
      "commit", // not a sync verb
      "", // empty string
      "pull", // duplicate
    ]);
    expect(parseHeaderPins(raw)).toEqual(["pull"]);
  });
});

describe("HeaderPinsStore", () => {
  it("pins verbs in order and persists them", () => {
    const store = new HeaderPinsStore();
    store.pin("pull");
    store.pin("fetch");
    expect(store.pins).toEqual(["pull", "fetch"]);
    expect(loadHeaderPins()).toEqual(["pull", "fetch"]);
  });

  it("refuses to pin duplicates", () => {
    const store = new HeaderPinsStore();
    store.pin("push");
    store.pin("push");
    expect(store.pins).toEqual(["push"]);
    expect(loadHeaderPins()).toEqual(["push"]);
  });

  it("unpins only pinned verbs", () => {
    const store = new HeaderPinsStore();
    store.pin("pull");
    store.unpin("push"); // not pinned
    expect(store.pins).toEqual(["pull"]);
    store.unpin("pull");
    expect(store.pins).toEqual([]);
    expect(loadHeaderPins()).toEqual([]);
  });

  it("toggle flips membership", () => {
    const store = new HeaderPinsStore();
    store.toggle("fetch");
    expect(store.has("fetch")).toBe(true);
    store.toggle("fetch");
    expect(store.has("fetch")).toBe(false);
  });

  it("has() tracks only the pin list", () => {
    const store = new HeaderPinsStore();
    expect(store.has("push")).toBe(false);
    store.pin("push");
    expect(store.has("push")).toBe(true);
    expect(store.has("pull")).toBe(false);
  });

  it("clear resets state and storage", () => {
    const store = new HeaderPinsStore();
    store.pin("pull");
    store.clear();
    expect(store.pins).toEqual([]);
    expect(localStorage.getItem(HEADER_PINS_KEY)).toBeNull();
  });

  it("a fresh store restores the persisted pin order", () => {
    headerPins.pin("push");
    headerPins.pin("pull");
    const restored = new HeaderPinsStore();
    expect(restored.pins).toEqual(["push", "pull"]);
  });

  it("tolerates corrupt storage on load", () => {
    localStorage.setItem(HEADER_PINS_KEY, "{oops");
    expect(loadHeaderPins()).toEqual([]);
    expect(new HeaderPinsStore().pins).toEqual([]);
  });
});
