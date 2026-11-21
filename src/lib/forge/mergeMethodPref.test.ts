/**
 * Unit tests for the per-repo merge-method pref (forge/mergeMethodPref.ts):
 * parse-validated read, per-root round-trip, corrupt-storage tolerance, and
 * the "squash-if-allowed, else merge" initial choice (E2 §9.4).
 */

import { afterEach, describe, expect, it } from "vitest";
import {
  initialMergeMethod,
  loadMergeMethod,
  parseMergeMethod,
  saveMergeMethod,
} from "./mergeMethodPref";

afterEach(() => {
  localStorage.clear();
});

describe("mergeMethodPref", () => {
  it("reads null when nothing is stored", () => {
    expect(loadMergeMethod("/repos/a")).toBeNull();
  });

  it("round-trips a remembered method per repo root", () => {
    saveMergeMethod("/repos/a", "rebase");
    saveMergeMethod("/repos/b", "merge");
    expect(loadMergeMethod("/repos/a")).toBe("rebase");
    expect(loadMergeMethod("/repos/b")).toBe("merge");
  });

  it("clears a stored method with null", () => {
    saveMergeMethod("/repos/a", "squash");
    saveMergeMethod("/repos/a", null);
    expect(loadMergeMethod("/repos/a")).toBeNull();
  });

  it("ignores an empty root (per-session tab without a root)", () => {
    saveMergeMethod("", "squash");
    expect(loadMergeMethod("")).toBeNull();
  });

  it("treats corrupt storage as no memory", () => {
    localStorage.setItem("mygitui.forge.merge-method./repos/a", "{not json");
    expect(loadMergeMethod("/repos/a")).toBeNull();
  });

  it("drops stored values that are not merge methods", () => {
    localStorage.setItem(
      "mygitui.forge.merge-method./repos/a",
      JSON.stringify("fast-forward"),
    );
    expect(loadMergeMethod("/repos/a")).toBeNull();
  });

  it("parseMergeMethod accepts exactly the three gh methods", () => {
    expect(parseMergeMethod(JSON.stringify("squash"))).toBe("squash");
    expect(parseMergeMethod(JSON.stringify("merge"))).toBe("merge");
    expect(parseMergeMethod(JSON.stringify("rebase"))).toBe("rebase");
    expect(parseMergeMethod(JSON.stringify("SQUASH"))).toBeNull();
    expect(parseMergeMethod("42")).toBeNull();
    expect(parseMergeMethod(null)).toBeNull();
  });
});

describe("initialMergeMethod (squash-if-allowed, else merge)", () => {
  it("squash for a mergeable PR", () => {
    expect(initialMergeMethod({ mergeable: "MERGEABLE" })).toBe("squash");
  });

  it("merge for conflicting / unknown / absent mergeable states", () => {
    expect(initialMergeMethod({ mergeable: "CONFLICTING" })).toBe("merge");
    expect(initialMergeMethod({ mergeable: "UNKNOWN" })).toBe("merge");
    expect(initialMergeMethod({ mergeable: "" })).toBe("merge");
  });
});
