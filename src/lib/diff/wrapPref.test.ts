/**
 * Unit tests for the persisted wrap preference (diff/wrapPref.ts): default
 * on, validated read, guarded access (missing/no-op localStorage).
 */

import { afterEach, describe, expect, it } from "vitest";
import { readWrapPref, writeWrapPref } from "./wrapPref";

afterEach(() => {
  localStorage.clear();
});

describe("wrapPref", () => {
  it("defaults to wrap on when nothing is stored", () => {
    expect(readWrapPref()).toBe(true);
  });

  it("round-trips a false value", () => {
    writeWrapPref(false);
    expect(readWrapPref()).toBe(false);
    writeWrapPref(true);
    expect(readWrapPref()).toBe(true);
  });

  it("treats corrupt storage as the default", () => {
    localStorage.setItem("mygitui.diff.wrap", "{not json");
    expect(readWrapPref()).toBe(true);
  });

  it("treats unknown stored values as wrap on", () => {
    localStorage.setItem("mygitui.diff.wrap", JSON.stringify("yes"));
    expect(readWrapPref()).toBe(true);
  });
});
