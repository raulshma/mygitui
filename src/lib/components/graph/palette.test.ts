/**
 * Unit tests for the M3-derived lane palette (palette.ts).
 */

import { describe, expect, it } from "vitest";
import {
  FALLBACK_THEME,
  laneColor,
  laneColorIndex,
  mixHex,
  parseHex,
  readTheme,
  withAlpha,
  type StyleLike,
} from "$lib/components/graph/palette";

describe("parseHex", () => {
  it("parses 3- and 6-digit hex (8-digit alpha ignored)", () => {
    expect(parseHex("#fff")).toEqual([255, 255, 255]);
    expect(parseHex("#65558F")).toEqual([0x65, 0x55, 0x8f]);
    expect(parseHex("#65558f80")).toEqual([0x65, 0x55, 0x8f]);
    expect(parseHex(" #65558f ")).toEqual([0x65, 0x55, 0x8f]);
  });

  it("rejects non-hex input", () => {
    expect(parseHex("rgb(1, 2, 3)")).toBeNull();
    expect(parseHex("")).toBeNull();
    expect(parseHex("#12")).toBeNull();
    expect(parseHex("#12345")).toBeNull();
  });
});

describe("withAlpha / mixHex", () => {
  it("withAlpha renders rgba() from hex", () => {
    expect(withAlpha("#65558f", 0.5)).toBe("rgba(101, 85, 143, 0.5)");
    expect(withAlpha("#fff", 0)).toBe("rgba(255, 255, 255, 0)");
  });

  it("withAlpha passes non-hex colors through untouched", () => {
    expect(withAlpha("var(--m3-primary)", 0.5)).toBe("var(--m3-primary)");
  });

  it("mixHex interpolates and clamps t", () => {
    expect(mixHex("#000000", "#ffffff", 0)).toBe("#000000");
    expect(mixHex("#000000", "#ffffff", 1)).toBe("#ffffff");
    expect(mixHex("#000000", "#ffffff", 0.5)).toBe("#808080");
    expect(mixHex("#000000", "#ffffff", -3)).toBe("#000000");
    expect(mixHex("#000000", "#ffffff", 9)).toBe("#ffffff");
  });
});

describe("readTheme", () => {
  it("falls back to the baseline palette without styles", () => {
    expect(readTheme(null)).toEqual(FALLBACK_THEME);
  });

  it("falls back when tokens are missing or non-hex", () => {
    const empty: StyleLike = { getPropertyValue: () => "" };
    expect(readTheme(empty)).toEqual(FALLBACK_THEME);
    const junk: StyleLike = { getPropertyValue: () => "color-mix(in srgb, red 50%, blue)" };
    expect(readTheme(junk)).toEqual(FALLBACK_THEME);
  });

  it("maps lanes as [primary, tertiary, secondary]", () => {
    const styles: StyleLike = {
      getPropertyValue: (name: string) =>
        ({
          "--m3-primary": "#111111",
          "--m3-secondary": "#333333",
          "--m3-tertiary": "#222222",
          "--m3-surface": "#f0f0f0",
          "--m3-outline": "#999999",
        })[name] ?? "",
    };
    expect(readTheme(styles)).toEqual({
      lanes: ["#111111", "#222222", "#333333"],
      surface: "#f0f0f0",
      outline: "#999999",
    });
  });
});

describe("laneColorIndex / laneColor", () => {
  it("assigns lanes to palette slots by lane % 3, stably", () => {
    expect(laneColorIndex(0)).toBe(0);
    expect(laneColorIndex(1)).toBe(1);
    expect(laneColorIndex(2)).toBe(2);
    expect(laneColorIndex(3)).toBe(0);
    expect(laneColorIndex(4)).toBe(1);
    expect(laneColorIndex(1000)).toBe(1);
    // Same lane → same slot regardless of page/batch: pure function.
    expect(laneColorIndex(7)).toBe(laneColorIndex(100));
  });

  it("wraps negative lanes instead of producing a bad index", () => {
    expect(laneColorIndex(-1)).toBe(2);
    expect(laneColorIndex(-3)).toBe(0);
    expect(laneColorIndex(-4, 3)).toBe(2);
  });

  it("laneColor resolves a concrete color and tolerates short palettes", () => {
    const palette = ["#aaaaaa", "#bbbbbb", "#cccccc"];
    expect(laneColor(0, palette)).toBe("#aaaaaa");
    expect(laneColor(4, palette)).toBe("#bbbbbb");
    expect(laneColor(5, ["#eeeeee"])).toBe("#eeeeee");
    expect(laneColor(9, [])).toBe("#888888");
  });
});
