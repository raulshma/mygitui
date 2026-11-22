/**
 * Tests for the persisted repo-action pins (`$lib/stores/repoActionPins`):
 * parse validation (envelope + legacy shapes), pin/unpin/toggle semantics,
 * the cap, and the localStorage round-trip (a fresh store restores the
 * persisted pin order).
 */
import { beforeEach, describe, expect, it } from "vitest";
import {
  REPO_ACTION_PINS_KEY,
  RepoActionPinsStore,
  loadRepoActionPins,
  parseRepoActionPinData,
  parseRepoActionPins,
  repoActionPins,
} from "./repoActionPins.svelte";

beforeEach(() => {
  localStorage.clear();
  repoActionPins.clear();
});

describe("parseRepoActionPinData", () => {
  it("keeps unique non-empty strings in order", () => {
    expect(parseRepoActionPinData(["clean", "bisect", "clean"])).toEqual([
      "clean",
      "bisect",
    ]);
    expect(parseRepoActionPinData(["", 42, null])).toEqual([]);
    expect(parseRepoActionPinData("nope")).toBeNull();
    expect(parseRepoActionPinData(null)).toBeNull();
  });

  it("caps the list at 12 pins", () => {
    const pins = Array.from({ length: 20 }, (_, i) => `action-${i}`);
    expect(parseRepoActionPinData(pins)).toHaveLength(12);
  });
});

describe("parseRepoActionPins", () => {
  it("returns empty for empty or malformed JSON", () => {
    expect(parseRepoActionPins(null)).toEqual([]);
    expect(parseRepoActionPins("")).toEqual([]);
    expect(parseRepoActionPins("not json")).toEqual([]);
    expect(parseRepoActionPins("42")).toEqual([]);
  });

  it("reads legacy raw lists and v1 envelopes alike", () => {
    expect(parseRepoActionPins(JSON.stringify(["clean"]))).toEqual(["clean"]);
    expect(
      parseRepoActionPins(JSON.stringify({ v: 1, data: ["bisect"] })),
    ).toEqual(["bisect"]);
    expect(parseRepoActionPins(JSON.stringify({ v: 99, data: ["x"] }))).toEqual(
      ["x"],
    );
  });
});

describe("RepoActionPinsStore", () => {
  it("pins ids in order and persists them (enveloped)", () => {
    const store = new RepoActionPinsStore();
    store.pin("clean");
    store.pin("bisect");
    expect(store.pins).toEqual(["clean", "bisect"]);
    expect(loadRepoActionPins()).toEqual(["clean", "bisect"]);
    const raw = localStorage.getItem(REPO_ACTION_PINS_KEY);
    expect(JSON.parse(raw!)).toEqual({ v: 1, data: ["clean", "bisect"] });
  });

  it("refuses to pin duplicates and empty ids", () => {
    const store = new RepoActionPinsStore();
    store.pin("clean");
    store.pin("clean");
    store.pin("");
    expect(store.pins).toEqual(["clean"]);
  });

  it("drops the oldest pin at the cap", () => {
    const store = new RepoActionPinsStore();
    for (let i = 0; i < 13; i++) store.pin(`action-${i}`);
    expect(store.pins).toHaveLength(12);
    expect(store.pins[0]).toBe("action-1");
  });

  it("unpins only pinned ids", () => {
    const store = new RepoActionPinsStore();
    store.pin("clean");
    store.unpin("bisect"); // not pinned
    expect(store.pins).toEqual(["clean"]);
    store.unpin("clean");
    expect(store.pins).toEqual([]);
    expect(loadRepoActionPins()).toEqual([]);
  });

  it("toggle flips membership", () => {
    const store = new RepoActionPinsStore();
    store.toggle("clean");
    expect(store.has("clean")).toBe(true);
    store.toggle("clean");
    expect(store.has("clean")).toBe(false);
  });

  it("clear resets state and storage", () => {
    const store = new RepoActionPinsStore();
    store.pin("clean");
    store.clear();
    expect(store.pins).toEqual([]);
    expect(localStorage.getItem(REPO_ACTION_PINS_KEY)).toBeNull();
  });

  it("a fresh store restores the persisted pin order", () => {
    repoActionPins.pin("bisect");
    repoActionPins.pin("clean");
    const restored = new RepoActionPinsStore();
    expect(restored.pins).toEqual(["bisect", "clean"]);
  });

  it("tolerates corrupt storage on load", () => {
    localStorage.setItem(REPO_ACTION_PINS_KEY, "{oops");
    expect(loadRepoActionPins()).toEqual([]);
    expect(new RepoActionPinsStore().pins).toEqual([]);
  });
});
