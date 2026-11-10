/**
 * Unit tests for the M3-derived lane palette (palette.ts).
 */

import { describe, expect, it } from "vitest";
import {
  FALLBACK_THEME,
  LANE_COUNT,
  deriveLanes,
  laneColor,
  laneColorIndex,
  mixHex,
  oklchToRgb,
  parseHex,
  readTheme,
  resolveColor,
  rgbToOklch,
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
    expect(withAlpha("var(--primary)", 0.5)).toBe("var(--primary)");
  });

  it("mixHex interpolates and clamps t", () => {
    expect(mixHex("#000000", "#ffffff", 0)).toBe("#000000");
    expect(mixHex("#000000", "#ffffff", 1)).toBe("#ffffff");
    expect(mixHex("#000000", "#ffffff", 0.5)).toBe("#808080");
    expect(mixHex("#000000", "#ffffff", -3)).toBe("#000000");
    expect(mixHex("#000000", "#ffffff", 9)).toBe("#ffffff");
  });
});

describe("resolveColor", () => {
  it("resolves the oklch extremes the token sets use", () => {
    expect(resolveColor("oklch(1 0 0)")).toEqual({ rgb: [255, 255, 255], alpha: 1 });
    expect(resolveColor("oklch(0 0 0)")).toEqual({ rgb: [0, 0, 0], alpha: 1 });
  });

  it("resolves app.css neutral tokens to their sRGB equivalents", () => {
    expect(resolveColor("oklch(0.205 0 0)")?.rgb).toEqual([23, 23, 23]);
    expect(resolveColor("oklch(0.922 0 0)")?.rgb).toEqual([229, 229, 229]);
  });

  it("keeps alpha from the slash syntax (translucent dark borders)", () => {
    expect(resolveColor("oklch(1 0 0 / 10%)")).toEqual({ rgb: [255, 255, 255], alpha: 0.1 });
    expect(resolveColor("oklch(1 0 0 / 0.25)")?.alpha).toBe(0.25);
  });

  it("accepts percent lightness/chroma and deg hues", () => {
    expect(resolveColor("oklch(100% 0% 0deg)")?.rgb).toEqual([255, 255, 255]);
  });

  it("still resolves hex and rgb() forms", () => {
    expect(resolveColor("#65558f")).toEqual({ rgb: [101, 85, 143], alpha: 1 });
    expect(resolveColor("#65558f80")?.alpha).toBeCloseTo(0.502, 2);
    expect(resolveColor("rgb(1, 2, 3)")).toEqual({ rgb: [1, 2, 3], alpha: 1 });
    expect(resolveColor("rgba(1, 2, 3, 0.5)")).toEqual({ rgb: [1, 2, 3], alpha: 0.5 });
  });

  it("returns null for forms canvas cannot be handed blindly", () => {
    expect(resolveColor("color-mix(in srgb, red 50%, blue)")).toBeNull();
    expect(resolveColor("var(--primary)")).toBeNull();
    expect(resolveColor("")).toBeNull();
    expect(resolveColor("oklch(nope 0 0)")).toBeNull();
  });

  it("round-trips sRGB through OKLCH within a rounding step", () => {
    for (const rgb of [
      [101, 85, 143],
      [26, 127, 55],
      [207, 34, 46],
      [0, 0, 0],
      [255, 255, 255],
    ] as const) {
      const { l, c, h } = rgbToOklch(rgb);
      const back = oklchToRgb(l, c, h);
      for (let i = 0; i < 3; i += 1) {
        expect(Math.abs(back[i]! - rgb[i]!)).toBeLessThanOrEqual(1);
      }
    }
  });
});

describe("deriveLanes", () => {
  it("produces six distinct, canvas-safe hex colors", () => {
    const lanes = deriveLanes({ l: 0.55, c: 0.15, h: 280 }, false);
    expect(lanes).toHaveLength(LANE_COUNT);
    expect(new Set(lanes).size).toBe(LANE_COUNT);
    for (const lane of lanes) expect(parseHex(lane)).not.toBeNull();
  });

  it("keeps a neutral accent's lanes separable by injecting chroma", () => {
    const lanes = deriveLanes({ l: 0.5, c: 0, h: 0 }, false);
    expect(new Set(lanes).size).toBe(LANE_COUNT);
  });

  it("spreads lightness across the band even for a near-black accent", () => {
    // The default light theme's --primary is oklch(0.205 0 0). Anchoring the
    // offsets to it clamped every lane onto the band floor — six muddy
    // colors distinguishable only by hue.
    const lanes = deriveLanes({ l: 0.205, c: 0, h: 0 }, false);
    const lightness = lanes.slice(1).map((lane) => rgbToOklch(parseHex(lane)!).l);
    expect(Math.max(...lightness) - Math.min(...lightness)).toBeGreaterThan(0.2);
    for (const l of lightness) {
      expect(l).toBeGreaterThanOrEqual(0.3);
      expect(l).toBeLessThanOrEqual(0.66);
    }
  });

  it("lifts generated lanes into a readable band on a dark surface", () => {
    const lanes = deriveLanes({ l: 0.2, c: 0.12, h: 280 }, true);
    // Lane 0 stays the accent verbatim; the rest must clear the dark surface.
    for (const lane of lanes.slice(1)) {
      expect(rgbToOklch(parseHex(lane)!).l).toBeGreaterThanOrEqual(0.55);
    }
  });
});

describe("readTheme", () => {
  const tokens = (vars: Record<string, string>): StyleLike => ({
    getPropertyValue: (name: string) => vars[name] ?? "",
  });

  it("falls back to the baseline palette without styles", () => {
    expect(readTheme(null)).toEqual(FALLBACK_THEME);
  });

  it("falls back when tokens are missing or unresolvable", () => {
    expect(readTheme(tokens({}))).toEqual(FALLBACK_THEME);
    expect(
      readTheme(tokens({ "--primary": "color-mix(in srgb, red 50%, blue)" })),
    ).toEqual(FALLBACK_THEME);
  });

  it("resolves the real oklch token set instead of falling back", () => {
    // The neutral light base from src/app.css. Before oklch support these
    // all failed parseHex and the graph silently used FALLBACK_THEME.
    const theme = readTheme(
      tokens({
        "--primary": "oklch(0.205 0 0)",
        "--card": "oklch(1 0 0)",
        "--border": "oklch(0.922 0 0)",
      }),
    );
    expect(theme).not.toEqual(FALLBACK_THEME);
    expect(theme.surface).toBe("#ffffff");
    expect(theme.outline).toBe("#e5e5e5");
    expect(theme.lanes[0]).toBe("#171717");
    expect(theme.lanes).toHaveLength(LANE_COUNT);
    expect(new Set(theme.lanes).size).toBe(LANE_COUNT);
  });

  it("composites a translucent dark border over the surface", () => {
    const theme = readTheme(
      tokens({
        "--primary": "oklch(0.922 0 0)",
        "--card": "oklch(0.205 0 0)",
        "--border": "oklch(1 0 0 / 10%)",
      }),
    );
    expect(theme.surface).toBe("#171717");
    // 10% white over #171717, not a solid white rule.
    expect(theme.outline).toBe("#2e2e2e");
    expect(theme.lanes[0]).toBe("#e5e5e5");
  });

  it("follows the accent when the user changes it", () => {
    const base = { "--card": "oklch(1 0 0)", "--border": "oklch(0.922 0 0)" };
    const purple = readTheme(tokens({ ...base, "--primary": "oklch(0.55 0.2 300)" }));
    const green = readTheme(tokens({ ...base, "--primary": "oklch(0.55 0.2 145)" }));
    expect(purple.lanes).not.toEqual(green.lanes);
    expect(purple.surface).toBe(green.surface);
  });

  it("falls back to --background when --card is absent", () => {
    const theme = readTheme(
      tokens({ "--primary": "#65558f", "--background": "#fdf7ff" }),
    );
    expect(theme.surface).toBe("#fdf7ff");
    expect(theme.lanes[0]).toBe("#65558f");
  });
});

describe("laneColorIndex / laneColor", () => {
  it("assigns lanes to palette slots by lane % paletteSize, stably", () => {
    expect(laneColorIndex(0)).toBe(0);
    expect(laneColorIndex(1)).toBe(1);
    expect(laneColorIndex(5)).toBe(5);
    expect(laneColorIndex(LANE_COUNT)).toBe(0);
    expect(laneColorIndex(LANE_COUNT + 1)).toBe(1);
    // Same lane → same slot regardless of page/batch: pure function.
    expect(laneColorIndex(7)).toBe(laneColorIndex(7 + LANE_COUNT * 10));
  });

  it("wraps negative lanes instead of producing a bad index", () => {
    expect(laneColorIndex(-1)).toBe(LANE_COUNT - 1);
    expect(laneColorIndex(-LANE_COUNT)).toBe(0);
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
