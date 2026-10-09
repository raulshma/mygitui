/**
 * Tests for the persisted per-repo history UI state (filter + selection +
 * detail visibility): parse validation, load/save round-trips through a
 * stubbed localStorage, and the per-root debounce (last write wins).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EMPTY_FILTER } from "./history-logic";
import {
  flushHistoryPrefs,
  HISTORY_PREFS_DEBOUNCE_MS,
  HISTORY_PREFS_PREFIX,
  loadHistoryPrefs,
  parseHistoryPrefs,
  saveHistoryPrefs,
} from "./historyPrefs";

describe("parseHistoryPrefs", () => {
  it("returns null for empty or malformed JSON", () => {
    expect(parseHistoryPrefs(null)).toBeNull();
    expect(parseHistoryPrefs("")).toBeNull();
    expect(parseHistoryPrefs("not json")).toBeNull();
    expect(parseHistoryPrefs("42")).toBeNull();
    expect(parseHistoryPrefs('"str"')).toBeNull();
  });

  it("keeps well-formed fields and defaults the rest", () => {
    const prefs = parseHistoryPrefs(
      JSON.stringify({
        filter: {
          text: "fix",
          regex: true,
          author: "Ann",
          path: "src/",
          after: "2026-01-02",
          before: "not-a-date",
          pickaxe: "-S needle",
        },
        selectedSha: "abcdef1234567890",
        detailOpen: true,
      }),
    );
    expect(prefs).toEqual({
      filter: {
        text: "fix",
        regex: true,
        author: "Ann",
        path: "src/",
        after: "2026-01-02",
        before: "",
        pickaxe: "-S needle",
        pickaxeRegex: "",
      },
      selectedSha: "abcdef1234567890",
      detailOpen: true,
    });
  });

  it("drops a non-sha selection and detailOpen without a selection", () => {
    const prefs = parseHistoryPrefs(
      JSON.stringify({ selectedSha: "DROP TABLE", detailOpen: true }),
    );
    expect(prefs).not.toBeNull();
    expect(prefs!.selectedSha).toBeNull();
    expect(prefs!.detailOpen).toBe(false);
    expect(prefs!.filter).toEqual(EMPTY_FILTER);
  });

  it("ignores non-string filter fields", () => {
    const prefs = parseHistoryPrefs(
      JSON.stringify({ filter: { text: 5, author: null, regex: "yes" } }),
    );
    expect(prefs!.filter.text).toBe("");
    expect(prefs!.filter.author).toBe("");
    expect(prefs!.filter.regex).toBe(false);
  });
});

describe("load/save", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    flushHistoryPrefs();
    vi.useRealTimers();
  });

  it("round-trips prefs through localStorage", () => {
    expect(loadHistoryPrefs("/repo")).toBeNull();

    saveHistoryPrefs("/repo", {
      filter: { ...EMPTY_FILTER, text: "wip" },
      selectedSha: "abcd1234",
      detailOpen: true,
    });
    flushHistoryPrefs();

    expect(loadHistoryPrefs("/repo")).toEqual({
      filter: { ...EMPTY_FILTER, text: "wip" },
      selectedSha: "abcd1234",
      detailOpen: true,
    });
  });

  it("keys storage by root so repos stay independent", () => {
    saveHistoryPrefs("/a", {
      filter: { ...EMPTY_FILTER, text: "a" },
      selectedSha: null,
      detailOpen: false,
    });
    saveHistoryPrefs("/b", {
      filter: { ...EMPTY_FILTER, text: "b" },
      selectedSha: null,
      detailOpen: false,
    });
    flushHistoryPrefs();

    expect(loadHistoryPrefs("/a")!.filter.text).toBe("a");
    expect(loadHistoryPrefs("/b")!.filter.text).toBe("b");
    expect(localStorage.getItem(HISTORY_PREFS_PREFIX + "/a")).toContain('"a"');
  });

  it("debounces per root: only the last write lands", () => {
    saveHistoryPrefs("/repo", {
      filter: { ...EMPTY_FILTER, text: "first" },
      selectedSha: null,
      detailOpen: false,
    });
    saveHistoryPrefs("/repo", {
      filter: { ...EMPTY_FILTER, text: "second" },
      selectedSha: "beef",
      detailOpen: false,
    });
    vi.advanceTimersByTime(HISTORY_PREFS_DEBOUNCE_MS - 1);
    expect(loadHistoryPrefs("/repo")).toBeNull();
    vi.advanceTimersByTime(1);
    expect(loadHistoryPrefs("/repo")!.filter.text).toBe("second");
  });

  it("ignores the empty root (popouts) and storage failures", () => {
    expect(() =>
      saveHistoryPrefs("", {
        filter: EMPTY_FILTER,
        selectedSha: null,
        detailOpen: false,
      }),
    ).not.toThrow();
    expect(loadHistoryPrefs("")).toBeNull();
  });
});
