/**
 * Tests for the theme composer (src/lib/theme/preset/composer).
 *
 * Merge order must mirror upstream `buildRegistryTheme`: base color full
 * set → theme override → chart color → menu accent → radius (light only).
 */
import { describe, expect, it } from "vitest";

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
