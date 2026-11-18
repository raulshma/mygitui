/**
 * compare model tests (phase 5 item 50): persisted compare-bar prefs
 * (parse/load/save with the swallow-all storage conventions) and the
 * target-kind → DiffSide resolution the bar diffs with.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  COMPARE_PREFS_PREFIX,
  DEFAULT_COMPARE_PREFS,
  loadComparePrefs,
  parseComparePrefs,
  saveComparePrefs,
  targetKindToSide,
} from "./compare";

afterEach(() => {
  vi.unstubAllGlobals();
  globalThis.localStorage?.clear();
});

describe("parseComparePrefs", () => {
  it("falls back to defaults for null, garbage and malformed payloads", () => {
    expect(parseComparePrefs(null)).toEqual(DEFAULT_COMPARE_PREFS);
    expect(parseComparePrefs("")).toEqual(DEFAULT_COMPARE_PREFS);
    expect(parseComparePrefs("not json {")).toEqual(DEFAULT_COMPARE_PREFS);
    expect(parseComparePrefs("42")).toEqual(DEFAULT_COMPARE_PREFS);
    expect(parseComparePrefs('{"mode":"bogus","targetKind":5}')).toEqual(
      DEFAULT_COMPARE_PREFS,
    );
  });

  it("keeps the known enum values and defaults the unknown ones", () => {
    expect(parseComparePrefs('{"mode":"threedot","targetKind":"worktree"}')).toEqual({
      mode: "threedot",
      targetKind: "worktree",
    });
    // an unknown field defaults while a valid sibling survives
    expect(
      parseComparePrefs('{"mode":"threedot","targetKind":"blob"}'),
    ).toEqual({ mode: "threedot", targetKind: "commit" });
    expect(parseComparePrefs('{"mode":"twodot"}')).toEqual({
      mode: "twodot",
      targetKind: "commit",
    });
  });
});

describe("loadComparePrefs / saveComparePrefs", () => {
  it("round-trips through localStorage under the per-root key", () => {
    saveComparePrefs("/repos/demo", { mode: "threedot", targetKind: "index" });
    expect(globalThis.localStorage.getItem(COMPARE_PREFS_PREFIX + "/repos/demo")).toBe(
      '{"mode":"threedot","targetKind":"index"}',
    );
    expect(loadComparePrefs("/repos/demo")).toEqual({
      mode: "threedot",
      targetKind: "index",
    });
  });

  it("keys per repo root and never writes for popouts (empty root)", () => {
    saveComparePrefs("/repos/a", { mode: "threedot", targetKind: "commit" });
    saveComparePrefs("", { mode: "threedot", targetKind: "head" });

    expect(loadComparePrefs("/repos/a").mode).toBe("threedot");
    // empty root: defaults, nothing persisted
    expect(loadComparePrefs("")).toEqual(DEFAULT_COMPARE_PREFS);
    expect(
      globalThis.localStorage.getItem(COMPARE_PREFS_PREFIX + ""),
    ).toBeNull();
  });

  it("defaults when storage is unavailable and swallows write failures", () => {
    vi.stubGlobal(
      "localStorage",
      {
        getItem: vi.fn(() => {
          throw new Error("privacy mode");
        }),
        setItem: vi.fn(() => {
          throw new Error("privacy mode");
        }),
        clear: vi.fn(),
      },
    );
    expect(loadComparePrefs("/repos/demo")).toEqual(DEFAULT_COMPARE_PREFS);
    expect(() =>
      saveComparePrefs("/repos/demo", { mode: "threedot", targetKind: "commit" }),
    ).not.toThrow();
  });
});

describe("targetKindToSide", () => {
  it("maps commit kinds to the typed ref and symbolic kinds to the side name", () => {
    expect(targetKindToSide("commit", "main~2")).toEqual({ commit: "main~2" });
    expect(targetKindToSide("worktree", "")).toBe("worktree");
    expect(targetKindToSide("index", "")).toBe("index");
    expect(targetKindToSide("head", "")).toBe("head");
  });
});
