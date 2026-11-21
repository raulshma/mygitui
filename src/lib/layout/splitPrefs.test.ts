/**
 * Unit tests for split prefs (`$lib/layout/splitPrefs`): the v1 → v2
 * envelope migration (old bare-ratio payloads must survive intact), the
 * per-width-mode open/closed dimension, and the debounced-write/flush
 * behavior. Storage is jsdom `localStorage`, seeded per test with unique
 * keys (the module's cache/pending maps are process-wide).
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  flushSplits,
  readSplitModeOpen,
  readSplitRatio,
  SPLIT_PREFS_VERSION,
  writeSplitModeOpen,
  writeSplitRatio,
} from "./splitPrefs";
import { WIDTH_MODES } from "./widthMode.svelte";

const DEBOUNCE_MS = 150;

beforeEach(() => {
  vi.useFakeTimers();
  window.localStorage.clear();
});

afterEach(() => {
  flushSplits(); // drain pending writes so no timer leaks across tests
  vi.useRealTimers();
});

/** A key unique per test (module-level cache must not leak between them). */
let seq = 0;
function freshKey(): string {
  seq += 1;
  return `test-${seq}`;
}

function stored(key: string): string | null {
  return window.localStorage.getItem(`mygitui.split.${key}`);
}

describe("splitPrefs migration (v1 → v2)", () => {
  it("migrates a legacy bare-ratio payload and preserves the ratio", () => {
    const key = freshKey();
    // v1 writes were the raw string of the ratio (no envelope).
    window.localStorage.setItem(`mygitui.split.${key}`, "0.55");
    expect(readSplitRatio(key, 0.62)).toBe(0.55);
  });

  it("migrates a v1 envelope-wrapped ratio payload", () => {
    const key = freshKey();
    window.localStorage.setItem(
      `mygitui.split.${key}`,
      JSON.stringify({ v: 1, data: 0.42 }),
    );
    expect(readSplitRatio(key, 0.62)).toBe(0.42);
  });

  it("the next write re-saves the migrated ratio under a v2 envelope", () => {
    const key = freshKey();
    window.localStorage.setItem(`mygitui.split.${key}`, "0.55");
    expect(readSplitRatio(key, 0.62)).toBe(0.55);
    writeSplitRatio(key, 0.7);
    vi.advanceTimersByTime(DEBOUNCE_MS);
    expect(stored(key)).toBe(
      JSON.stringify({ v: SPLIT_PREFS_VERSION, data: { ratio: 0.7 } }),
    );
  });

  it("clamps out-of-range legacy ratios through the migration", () => {
    const key = freshKey();
    window.localStorage.setItem(`mygitui.split.${key}`, "0.01");
    expect(readSplitRatio(key, 0.5)).toBe(0.15);
    const high = freshKey();
    window.localStorage.setItem(`mygitui.split.${high}`, "0.99");
    expect(readSplitRatio(high, 0.5)).toBe(0.85);
  });

  it("drops malformed legacy payloads (fallback applies)", () => {
    const key = freshKey();
    window.localStorage.setItem(`mygitui.split.${key}`, "not json");
    expect(readSplitRatio(key, 0.62)).toBe(0.62);
  });
});

describe("splitPrefs ratios", () => {
  it("round-trips a ratio through the v2 envelope", () => {
    const key = freshKey();
    writeSplitRatio(key, 0.34);
    vi.advanceTimersByTime(DEBOUNCE_MS);
    expect(readSplitRatio(key, 0.62)).toBe(0.34);
    expect(stored(key)).toBe(
      JSON.stringify({ v: SPLIT_PREFS_VERSION, data: { ratio: 0.34 } }),
    );
  });

  it("returns the fallback when the key is missing", () => {
    expect(readSplitRatio(freshKey(), 0.62)).toBe(0.62);
  });

  it("debounces a drag stream to the last ratio", () => {
    const key = freshKey();
    writeSplitRatio(key, 0.3);
    writeSplitRatio(key, 0.4);
    vi.advanceTimersByTime(DEBOUNCE_MS - 1);
    expect(stored(key)).toBeNull();
    writeSplitRatio(key, 0.5);
    vi.advanceTimersByTime(DEBOUNCE_MS);
    expect(readSplitRatio(key, 0.62)).toBe(0.5);
  });

  it("flushSplits lands the pending write synchronously", () => {
    const key = freshKey();
    writeSplitRatio(key, 0.44);
    flushSplits();
    expect(readSplitRatio(key, 0.62)).toBe(0.44);
  });
});

describe("splitPrefs per-mode open/closed state", () => {
  it("returns the fallback when no mode state is persisted", () => {
    const key = freshKey();
    for (const mode of WIDTH_MODES) {
      expect(readSplitModeOpen(key, mode, true)).toBe(true);
    }
  });

  it("round-trips a mode's open state alongside the stored ratio", () => {
    const key = freshKey();
    writeSplitRatio(key, 0.62);
    writeSplitModeOpen(key, "narrow", false);
    vi.advanceTimersByTime(DEBOUNCE_MS);
    expect(readSplitModeOpen(key, "narrow", true)).toBe(false);
    // The ratio survived the mode write (the "mode dimension alongside
    // existing ratios" contract).
    expect(readSplitRatio(key, 0.34)).toBe(0.62);
    expect(stored(key)).toBe(
      JSON.stringify({
        v: SPLIT_PREFS_VERSION,
        data: { ratio: 0.62, modes: { narrow: false } },
      }),
    );
  });

  it("merges mode writes without clobbering other modes", () => {
    const key = freshKey();
    writeSplitModeOpen(key, "medium", false);
    writeSplitModeOpen(key, "stacked", false);
    writeSplitModeOpen(key, "medium", true);
    vi.advanceTimersByTime(DEBOUNCE_MS);
    expect(readSplitModeOpen(key, "medium", false)).toBe(true);
    expect(readSplitModeOpen(key, "stacked", true)).toBe(false);
    expect(readSplitModeOpen(key, "wide", true)).toBe(true);
  });

  it("a ratio write after a mode write preserves the mode state", () => {
    const key = freshKey();
    writeSplitModeOpen(key, "narrow", false);
    writeSplitRatio(key, 0.5);
    vi.advanceTimersByTime(DEBOUNCE_MS);
    expect(readSplitModeOpen(key, "narrow", true)).toBe(false);
    expect(readSplitRatio(key, 0.34)).toBe(0.5);
    expect(Object.keys(JSON.parse(stored(key)!).data.modes)).toEqual([
      "narrow",
    ]);
  });

  it("reads mode state from a migrated legacy payload (modes absent → fallback)", () => {
    const key = freshKey();
    window.localStorage.setItem(`mygitui.split.${key}`, "0.55");
    expect(readSplitModeOpen(key, "narrow", true)).toBe(true);
    writeSplitModeOpen(key, "narrow", false);
    vi.advanceTimersByTime(DEBOUNCE_MS);
    // The migrated ratio rides along into the v2 envelope.
    expect(readSplitRatio(key, 0.34)).toBe(0.55);
    expect(readSplitModeOpen(key, "narrow", true)).toBe(false);
  });
});
