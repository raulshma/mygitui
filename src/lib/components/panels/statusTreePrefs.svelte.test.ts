/**
 * Tests for the persisted per-repo status-tree collapse state
 * (`statusTreePrefs`): parse validation, load/save round-trips through
 * the versioned localStorage envelope, per-root independence, and the
 * debounced write (last write wins).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  STATUS_TREE_PREFS_DEBOUNCE_MS,
  parseCollapsedMap,
  StatusTreePrefs,
} from "./statusTreePrefs.svelte";

const KEY = "mygitui.status.collapsed./repo";

describe("parseCollapsedMap", () => {
  it("returns an empty map for empty or malformed payloads", () => {
    expect(parseCollapsedMap(null)).toEqual({});
    expect(parseCollapsedMap("not json")).toEqual({});
    expect(parseCollapsedMap(42)).toEqual({});
    expect(parseCollapsedMap(["staged"])).toEqual({});
  });

  it("keeps boolean values and drops everything else", () => {
    expect(
      parseCollapsedMap({
        "section:staged": true,
        "section:changes": false,
        "mod:vendor/foo": 1,
        "sub:vendor/foo:changes": "yes",
      }),
    ).toEqual({ "section:staged": true, "section:changes": false });
  });
});

describe("StatusTreePrefs", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("snapshots default to all-expanded when nothing is stored", () => {
    const prefs = new StatusTreePrefs();
    expect(prefs.snapshot("/repo")).toEqual({});
  });

  it("round-trips collapsed keys through the versioned envelope", () => {
    const prefs = new StatusTreePrefs();
    prefs.set("/repo", "section:staged", true);
    prefs.set("/repo", "section:changes", false);
    vi.advanceTimersByTime(STATUS_TREE_PREFS_DEBOUNCE_MS);

    const raw = localStorage.getItem(KEY);
    expect(JSON.parse(raw!)).toMatchObject({ v: 1 });
    expect(prefs.snapshot("/repo")).toEqual({
      "section:staged": true,
      "section:changes": false,
    });

    // A fresh instance (app relaunch) reads the persisted state back.
    expect(new StatusTreePrefs().snapshot("/repo")).toEqual({
      "section:staged": true,
      "section:changes": false,
    });
  });

  it("keys storage by root so repos stay independent", () => {
    const prefs = new StatusTreePrefs();
    prefs.set("/a", "section:changes", true);
    prefs.set("/b", "section:staged", true);
    vi.advanceTimersByTime(STATUS_TREE_PREFS_DEBOUNCE_MS);

    expect(new StatusTreePrefs().snapshot("/a")).toEqual({
      "section:changes": true,
    });
    expect(new StatusTreePrefs().snapshot("/b")).toEqual({
      "section:staged": true,
    });
  });

  it("debounces writes per root: only the last state lands", () => {
    const prefs = new StatusTreePrefs();
    prefs.set("/repo", "section:changes", true);
    prefs.set("/repo", "section:changes", false);
    prefs.set("/repo", "section:staged", true);
    vi.advanceTimersByTime(STATUS_TREE_PREFS_DEBOUNCE_MS - 1);
    expect(localStorage.getItem(KEY)).toBeNull();
    vi.advanceTimersByTime(1);
    const envelope = JSON.parse(localStorage.getItem(KEY)!);
    expect(envelope.data).toEqual({
      "section:changes": false,
      "section:staged": true,
    });
  });

  it("drops a malformed stored payload instead of throwing", () => {
    localStorage.setItem(KEY, "{not json");
    const prefs = new StatusTreePrefs();
    expect(prefs.snapshot("/repo")).toEqual({});
  });
});
