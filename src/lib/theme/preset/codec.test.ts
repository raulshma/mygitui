/**
 * Tests for the appearance preset codec (src/lib/theme/preset/codec).
 *
 * Codes are bit-packed, base62-encoded config indexes with a version
 * prefix, shared with shadcn-svelte.com/create — so decoding must match the
 * upstream value-array order exactly, and normalization must coerce
 * anything arbitrary into a valid config.
 */
import { describe, expect, it } from "vitest";

import {
  DEFAULT_APPEARANCE,
  decodeAppearance,
  encodeAppearance,
  isPresetCode,
  normalizeAppearance,
  randomAppearance,
  type AppearanceConfig,
} from "./codec";
import { BASE_COLORS, FONT_HEADINGS, FONTS, getThemesForBaseColor } from "./values";

describe("encodeAppearance / decodeAppearance", () => {
  it("round-trips the app defaults", () => {
    expect(decodeAppearance(encodeAppearance(DEFAULT_APPEARANCE))).toEqual(DEFAULT_APPEARANCE);
  });

  it("decodes the upstream all-zeros code to the website defaults", () => {
    // "b0" = every field at index 0 — the /create defaults (vega style).
    expect(decodeAppearance("b0")).toEqual({ ...DEFAULT_APPEARANCE, style: "vega" });
  });

  it("round-trips every field through a code", () => {
    const config: AppearanceConfig = {
      style: "vega",
      baseColor: "taupe",
      theme: "violet",
      chartColor: "amber",
      iconLibrary: "phosphor",
      font: "jetbrains-mono",
      fontHeading: "playfair-display",
      radius: "large",
      menuAccent: "bold",
      menuColor: "inverted-translucent",
    };
    expect(decodeAppearance(encodeAppearance(config))).toEqual(config);
  });

  it("decodes a v1 ('a') code: first 8 fields, fontHeading inherit", () => {
    // The v1 field layout is the v2 layout minus the two trailing fields
    // (chartColor, fontHeading), so "a" + the same payload decodes the
    // shared fields identically.
    const config: AppearanceConfig = {
      ...DEFAULT_APPEARANCE,
      style: "mira",
      baseColor: "zinc",
      theme: "cyan",
      font: "figtree",
    };
    const code = encodeAppearance(config);

    const decoded = decodeAppearance("a" + code.slice(1));
    expect(decoded).toMatchObject({
      style: "mira",
      baseColor: "zinc",
      theme: "cyan",
      font: "figtree",
      // v1 codes have no chartColor → default ("neutral") coerced to the
      // base color's available set ("zinc").
      chartColor: "zinc",
      fontHeading: "inherit", // v1 codes force inherit
    });
  });

  it("returns null for malformed codes", () => {
    expect(decodeAppearance("")).toBeNull();
    expect(decodeAppearance("b")).toBeNull();
    expect(decodeAppearance("z0")).toBeNull(); // unknown version
    expect(decodeAppearance("b!!!")).toBeNull(); // non-base62 payload
  });

  it("isPresetCode accepts plausible codes and rejects junk", () => {
    expect(isPresetCode("b0")).toBe(true);
    expect(isPresetCode("a1Z")).toBe(true);
    expect(isPresetCode("")).toBe(false);
    expect(isPresetCode("b")).toBe(false);
    expect(isPresetCode("z0")).toBe(false);
    expect(isPresetCode("b-._")).toBe(false);
    expect(isPresetCode("b00000000000")).toBe(false); // > 10 chars
  });
});

describe("normalizeAppearance", () => {
  it("fills unknown values with defaults", () => {
    const normalized = normalizeAppearance({
      style: "nonsense" as never,
      baseColor: "nope" as never,
      font: "comic-sans" as never,
    });
    expect(normalized).toEqual({ ...DEFAULT_APPEARANCE, style: "nova", baseColor: "neutral", font: "inter" });
  });

  it("constrains theme and chartColor to the base color's set", () => {
    const normalized = normalizeAppearance({
      baseColor: "taupe",
      // "taupe" is a base color, so it is available; "neutral" is not.
      theme: "neutral",
      chartColor: "neutral",
    });
    const available = getThemesForBaseColor("taupe").map((t) => t.name);
    expect(available).toContain("taupe");
    expect(available).not.toContain("neutral");
    expect(normalized.theme).toBe("taupe");
    expect(normalized.chartColor).toBe("taupe");
  });

  it("keeps colored themes when the base color changes", () => {
    const normalized = normalizeAppearance({ baseColor: "mist", theme: "violet" });
    expect(normalized.theme).toBe("violet");
  });

  it("forces square corners for lyra and sera", () => {
    expect(normalizeAppearance({ style: "lyra", radius: "large" }).radius).toBe("none");
    expect(normalizeAppearance({ style: "sera", radius: "medium" }).radius).toBe("none");
  });

  it("rejects the large radius for rhea", () => {
    expect(normalizeAppearance({ style: "rhea", radius: "large" }).radius).toBe("default");
    expect(normalizeAppearance({ style: "rhea", radius: "small" }).radius).toBe("small");
  });

  it("keeps valid fontHeading values and rejects invalid ones", () => {
    expect(normalizeAppearance({ fontHeading: "lora" }).fontHeading).toBe("lora");
    expect(normalizeAppearance({ fontHeading: "inherit" }).fontHeading).toBe("inherit");
    expect(normalizeAppearance({ fontHeading: "nope" as never }).fontHeading).toBe(
      DEFAULT_APPEARANCE.fontHeading,
    );
    expect(FONT_HEADINGS.length).toBe(FONTS.length + 1);
  });
});

describe("randomAppearance", () => {
  it("always produces a normalized config", () => {
    for (let i = 0; i < 50; i++) {
      const random = randomAppearance();
      expect(decodeAppearance(encodeAppearance(random))).toEqual(normalizeAppearance(random));
      expect(BASE_COLORS.map((b) => b.name)).toContain(random.baseColor);
    }
  });
});
