/**
 * Composes the final CSS custom property sets for an appearance config —
 * a port of upstream's `buildRegistryTheme` (docs/src/lib/registry/config.ts).
 *
 * Merge order (later wins):
 *   1. base color full token set (light + dark)
 *   2. theme override set (primary/secondary/sidebar-primary/chart ramps)
 *   3. chart color: chart-1..5 from the chart theme, both schemes
 *   4. menu accent "bold": accent := primary, accent-foreground := primary-foreground
 *   5. radius: `--radius` override in the light set (dark never overrides)
 *
 * Pure functions — the applier turns the result into CSS.
 */
import type { AppearanceConfig } from "./codec.js";
import {
	getBaseColor,
	getFont,
	getTheme,
	type ChartColorName,
	type FontHeadingName,
	type FontName,
	type ThemeName,
} from "./values.js";

export type SchemeVars = Record<string, string>;

/** Per-scheme CSS custom properties, ready to be written into :root/.dark. */
export type ComposedTheme = { light: SchemeVars; dark: SchemeVars };

/** Which chart ramp index a var belongs to (chart-1..chart-5). */
const CHART_KEYS = ["chart-1", "chart-2", "chart-3", "chart-4", "chart-5"] as const;

export function buildThemeVars(config: AppearanceConfig): ComposedTheme {
	const baseColor = getBaseColor(config.baseColor);
	const theme = getTheme(config.theme);
	if (!baseColor || !theme) {
		// Callers pass normalized configs; fall back to the neutral base.
		return {
			light: { ...getBaseColor("neutral")!.cssVars.light },
			dark: { ...getBaseColor("neutral")!.cssVars.dark },
		};
	}

	const light: SchemeVars = { ...baseColor.cssVars.light, ...theme.cssVars.light };
	const dark: SchemeVars = { ...baseColor.cssVars.dark, ...theme.cssVars.dark };

	applyChartColor(light, dark, config.chartColor);
	applyMenuAccent(light, dark, config.menuAccent);
	applyRadius(light, config.radius);

	return { light, dark };
}

/** Copies chart-1..5 from the chart theme into both schemes. */
function applyChartColor(
	light: SchemeVars,
	dark: SchemeVars,
	chartColor: ChartColorName,
): void {
	const chartTheme = getTheme(chartColor);
	if (!chartTheme) return;
	for (const key of CHART_KEYS) {
		const lightValue = chartTheme.cssVars.light[key];
		const darkValue = chartTheme.cssVars.dark[key];
		if (lightValue) light[key] = lightValue;
		if (darkValue) dark[key] = darkValue;
	}
}

/** "bold" menus render hover/active surfaces in the theme's primary. */
function applyMenuAccent(
	light: SchemeVars,
	dark: SchemeVars,
	accent: AppearanceConfig["menuAccent"],
): void {
	if (accent !== "bold") return;
	light.accent = light.primary;
	light["accent-foreground"] = light["primary-foreground"];
	dark.accent = dark.primary;
	dark["accent-foreground"] = dark["primary-foreground"];
}

/** Radius override only when non-default; dark scheme never overrides. */
function applyRadius(light: SchemeVars, radius: AppearanceConfig["radius"]): void {
	if (radius === "default") return;
	const entry = RADIUS_VALUES[radius];
	if (entry) light.radius = entry;
}

const RADIUS_VALUES: Record<AppearanceConfig["radius"], string> = {
	default: "",
	none: "0rem",
	small: "0.45rem",
	medium: "0.625rem",
	large: "0.875rem",
};

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/**
 * Font-family custom properties for the config. The body font drives the
 * whole UI face — mono/serif choices set `--font-sans` too (upstream applies
 * the chosen font to `html` whichever its type). `--font-heading` falls back
 * to the body font when the heading choice is "inherit". The app's explicit
 * `--font-mono` (terminal, diffs) is untouched, like upstream's default.
 */
export function buildFontVars(config: {
	font: FontName;
	fontHeading: FontHeadingName;
}): { sans: string; heading: string } {
	const body = getFont(config.font);
	const heading = getFont(config.fontHeading);
	return {
		sans: body?.family ?? FALLBACK_SANS,
		heading: heading?.family ?? body?.family ?? FALLBACK_SANS,
	};
}

const FALLBACK_SANS = "'Inter Variable', sans-serif";
