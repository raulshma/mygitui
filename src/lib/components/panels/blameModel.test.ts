/**
 * Unit tests for the blame age-heat model (panels/blameModel.ts): ratio
 * scaling, dedupe, the all-equal degenerate case and the tint renderer.
 */

import { describe, expect, it } from "vitest";
import {
  ageBackground,
  ageRatios,
  AGE_HEAT_MAX_PCT,
  buildBlameQuery,
} from "$lib/components/panels/blameModel";
import type { BlameLine } from "$lib/ipc/types";

function blameLine(sha: string, time: number): BlameLine {
  return {
    line_no: 1,
    sha,
    signature: { name: "Ada", email: "ada@example.com", time, offset_minutes: 0 },
    final_sha: sha,
    final_signature: {
      name: "Ada",
      email: "ada@example.com",
      time,
      offset_minutes: 0,
    },
    text: "x",
    ignored: false,
  };
}

describe("ageRatios", () => {
  it("scales newest → 1 and oldest → 0, linearly between", () => {
    const ratios = ageRatios([
      blameLine("a", 1000),
      blameLine("b", 2000),
      blameLine("c", 3000),
    ]);
    expect(ratios.get("a")).toBe(0);
    expect(ratios.get("b")).toBeCloseTo(0.5);
    expect(ratios.get("c")).toBe(1);
  });

  it("dedupes shas (first wins)", () => {
    const ratios = ageRatios([
      blameLine("a", 1000),
      blameLine("a", 9999),
    ]);
    expect(ratios.get("a")).toBe(1); // only 1000 kept → single value = newest
    expect(ratios.size).toBe(1);
  });

  it("treats an all-equal timestamp as newest (no divide by zero)", () => {
    const ratios = ageRatios([
      blameLine("a", 1000),
      blameLine("b", 1000),
    ]);
    expect(ratios.get("a")).toBe(1);
    expect(ratios.get("b")).toBe(1);
  });

  it("returns an empty map for empty input", () => {
    expect(ageRatios([]).size).toBe(0);
  });
});

describe("ageBackground", () => {
  it("fades the oldest to transparent", () => {
    expect(ageBackground(0)).toBe("transparent");
    expect(ageBackground(-1)).toBe("transparent");
  });

  it("gives the newest the strongest accent", () => {
    expect(ageBackground(1)).toContain(`${AGE_HEAT_MAX_PCT}%`);
    expect(ageBackground(5)).toContain(`${AGE_HEAT_MAX_PCT}%`); // clamped
  });

  it("is monotonic in the ratio", () => {
    const low = ageBackground(0.25);
    const high = ageBackground(0.75);
    const pct = (s: string): number => Number(s.match(/(\d+)%/)?.[1] ?? -1);
    expect(pct(low)).toBeLessThan(pct(high));
  });

  it("derives from the theme primary (color-mix)", () => {
    expect(ageBackground(1)).toMatch(/^color-mix\(in srgb, var\(--primary\)/);
  });
});

describe("buildBlameQuery", () => {
  it("returns null when every option is at its default", () => {
    expect(buildBlameQuery({})).toBeNull();
    expect(buildBlameQuery({ from: "abc123" })).toBeNull();
    expect(buildBlameQuery({ oldest: "   " })).toBeNull();
    expect(
      buildBlameQuery({
        ignoreWhitespace: false,
        firstParent: false,
        ignoreRevs: false,
        oldest: "",
      }),
    ).toBeNull();
  });

  it("carries the from revision and trimmed oldest bound", () => {
    expect(
      buildBlameQuery({ from: "abc123", oldest: " def456 " }),
    ).toEqual({
      from: "abc123",
      oldest: "def456",
      ignore_whitespace: false,
      first_parent: false,
      ignore_revs: false,
    });
  });

  it("forwards the toggles verbatim", () => {
    expect(
      buildBlameQuery({
        ignoreWhitespace: true,
        firstParent: true,
        ignoreRevs: true,
        oldest: "main",
      }),
    ).toEqual({
      from: null,
      oldest: "main",
      ignore_whitespace: true,
      first_parent: true,
      ignore_revs: true,
    });
  });

  it("accepts branch-ish oldest values verbatim (plain pass-through)", () => {
    expect(buildBlameQuery({ oldest: "release-1.2"})?.oldest).toBe("release-1.2");
  });
});
