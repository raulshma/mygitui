/**
 * Tests for the theme composer (src/lib/theme/preset/composer).
 *
 * Merge order must mirror upstream `buildRegistryTheme`: base color full
 * set → theme override → chart color → menu accent → radius (light only) →
 * contrast guardrails (fill/foreground repairs + primary-ink/-hover).
 */
import { describe, expect, it } from "vitest";

import { contrastRatio, parseOklch } from "../contrast";
import { DEFAULT_APPEARANCE, type AppearanceConfig } from "./codec";
import { buildFontVars, buildThemeVars } from "./composer";
import { getBaseColor, getTheme } from "./values";

function config(overrides: Partial<AppearanceConfig>): AppearanceConfig {
  return { ...DEFAULT_APPEARANCE, ...overrides };
}

describe("buildThemeVars", () => {
  it("returns the base color's full sets for a plain config", () => {
    const base = getBaseColor("neutral")!;
    const { light, dark } = buildThemeVars(config({}));
    expect(light.background).toBe(base.cssVars.light.background);
    expect(light["muted-foreground"]).toBe(base.cssVars.light["muted-foreground"]);
    expect(dark.background).toBe(base.cssVars.dark.background);
    // Dark scheme keeps its own radius absence (never overridden there).
    expect(dark.radius).toBeUndefined();
  });

  it("lets the theme override the base color key-by-key", () => {
    const theme = getTheme("red")!;
    const { light, dark } = buildThemeVars(config({ theme: "red" }));
    expect(light.primary).toBe(theme.cssVars.light.primary);
    expect(dark.primary).toBe(theme.cssVars.dark.primary);
    // Non-overridden keys stay from the base color.
    expect(light.background).toBe(getBaseColor("neutral")!.cssVars.light.background);
  });

  it("applies the chart color ramp from the chart theme", () => {
    const chart = getTheme("blue")!;
    const { light, dark } = buildThemeVars(
      config({ theme: "red", chartColor: "blue" }),
    );
    expect(light["chart-1"]).toBe(chart.cssVars.light["chart-1"]);
    expect(dark["chart-5"]).toBe(chart.cssVars.dark["chart-5"]);
    // Primary still comes from the theme, not the chart theme.
    expect(light.primary).toBe(getTheme("red")!.cssVars.light.primary);
  });

  it("bold menu accent derives accent from primary in both schemes", () => {
    const subtle = buildThemeVars(config({ theme: "green" }));
    const bold = buildThemeVars(config({ theme: "green", menuAccent: "bold" }));
    expect(bold.light.accent).toBe(bold.light.primary);
    expect(bold.light["accent-foreground"]).toBe(bold.light["primary-foreground"]);
    expect(bold.dark.accent).toBe(bold.dark.primary);
    // Subtle leaves the base color's accent untouched.
    expect(subtle.light.accent).toBe(getBaseColor("neutral")!.cssVars.light.accent);
  });

  it("overrides --radius in the light set only, and only when non-default", () => {
    expect(buildThemeVars(config({ radius: "default" })).light.radius).toBe(
      getBaseColor("neutral")!.cssVars.light.radius,
    );
    expect(buildThemeVars(config({ radius: "large" })).light.radius).toBe("0.875rem");
    expect(buildThemeVars(config({ radius: "none" })).light.radius).toBe("0rem");
  });

  it("falls back to neutral when given an unknown base color", () => {
    const { light } = buildThemeVars(
      config({ baseColor: "nope" as never, theme: "nope" as never }),
    );
    expect(light.background).toBe(getBaseColor("neutral")!.cssVars.light.background);
  });
});

describe("contrast guardrails", () => {
  it("repair below-threshold fill/foreground pairs (yellow, both schemes)", () => {
    for (const scheme of ["light", "dark"] as const) {
      const vars = buildThemeVars(config({ theme: "yellow" }))[scheme];
      const raw = getTheme("yellow")!.cssVars[scheme];
      // The shipped pair is under 4.5:1 (the reported wash-out)…
      expect(
        contrastRatio(raw["primary-foreground"], raw.primary),
      ).toBeLessThan(4.5);
      // …and the guardrail darkens the foreground past it, hue intact.
      const fg = parseOklch(vars["primary-foreground"])!;
      expect(contrastRatio(vars["primary-foreground"], vars.primary)).toBeGreaterThanOrEqual(
        4.5,
      );
      expect(fg.h).toBeCloseTo(parseOklch(raw["primary-foreground"])!.h, 0);
      // The fill itself keeps the theme's identity (only the fg moves).
      expect(vars.primary).toBe(raw.primary);
    }
  });

  it("leave passing pairs byte-identical (red dark, neutral base)", () => {
    const red = buildThemeVars(config({ theme: "red" }));
    expect(red.dark["primary-foreground"]).toBe(
      getTheme("red")!.cssVars.dark["primary-foreground"],
    );
    const neutral = buildThemeVars(config({}));
    const base = getBaseColor("neutral")!;
    expect(neutral.light["accent-foreground"]).toBe(base.cssVars.light["accent-foreground"]);
    expect(neutral.dark["primary-foreground"]).toBe(base.cssVars.dark["primary-foreground"]);
  });

  it("derive a readable --primary-ink against the background in both schemes", () => {
    for (const scheme of ["light", "dark"] as const) {
      const vars = buildThemeVars(config({ theme: "yellow" }))[scheme];
      const ink = parseOklch(vars["primary-ink"])!;
      expect(contrastRatio(vars["primary-ink"], vars.background)).toBeGreaterThanOrEqual(4.5);
      // Hue preserved: the ink stays the theme color, not a gray.
      expect(ink.h).toBeCloseTo(parseOklch(vars.primary)!.h, 0);
    }
  });

  it("derive --primary-hover stepping away from the foreground", () => {
    const { light, dark } = buildThemeVars(config({ theme: "yellow" }));
    // Direction follows the pair, not the scheme: yellow's foreground is
    // darker than its fill in both schemes, and so is the neutral dark
    // scheme's — hover lightens all three fills (away from the fg).
    const lightHover = parseOklch(light["primary-hover"])!;
    expect(lightHover.l).toBeGreaterThan(parseOklch(light.primary)!.l);
    const darkHover = parseOklch(dark["primary-hover"])!;
    expect(darkHover.l).toBeGreaterThan(parseOklch(dark.primary)!.l);
    const neutralDark = buildThemeVars(config({})).dark;
    expect(parseOklch(neutralDark["primary-hover"])!.l).toBeGreaterThan(
      parseOklch(neutralDark.primary)!.l,
    );
  });

  it("guard the bold accent pair after accent := primary", () => {
    const bold = buildThemeVars(config({ theme: "yellow", menuAccent: "bold" }));
    expect(bold.light.accent).toBe(bold.light.primary);
    expect(
      contrastRatio(bold.light["accent-foreground"], bold.light.accent),
    ).toBeGreaterThanOrEqual(4.5);
    expect(
      contrastRatio(bold.dark["accent-foreground"], bold.dark.accent),
    ).toBeGreaterThanOrEqual(4.5);
  });
});

describe("buildFontVars", () => {
  it("uses the body font for --font-sans and inherit falls back to it", () => {
    const fonts = buildFontVars({ font: "figtree", fontHeading: "inherit" });
    expect(fonts.sans).toBe("'Figtree Variable', sans-serif");
    expect(fonts.heading).toBe(fonts.sans);
  });

  it("resolves an explicit heading font independently", () => {
    const fonts = buildFontVars({ font: "inter", fontHeading: "playfair-display" });
    expect(fonts.sans).toBe("'Inter Variable', sans-serif");
    expect(fonts.heading).toBe("'Playfair Display Variable', serif");
  });

  it("treats mono body fonts as the UI face (upstream applies them to html)", () => {
    const fonts = buildFontVars({ font: "jetbrains-mono", fontHeading: "inherit" });
    expect(fonts.sans).toBe("'JetBrains Mono Variable', monospace");
  });
});
