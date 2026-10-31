/**
 * Appearance option sets for the shadcn-svelte customizer (mirrors
 * shadcn-svelte.com/create).
 *
 * The value arrays are the codec's source of truth: preset codes index into
 * them positionally, so the order of every array here MUST match upstream
 * (`shadcn-svelte/preset` + `docs/src/lib/registry/config.ts`). Only append —
 * never reorder — or preset codes from the website decode wrong.
 */
import { BASE_THEMES, THEMES } from "./themes.js";

// ---------------------------------------------------------------------------
// Styles (component shapes; each has a vendored style-<name>.css sheet)
// ---------------------------------------------------------------------------

export const STYLES = [
	{
		name: "vega",
		title: "Vega",
		description: "The classic shadcn/ui look. Clean, neutral, and familiar.",
	},
	{
		name: "nova",
		title: "Nova",
		description: "Reduced padding and margins for compact layouts.",
	},
	{
		name: "maia",
		title: "Maia",
		description: "Soft and rounded, with generous spacing.",
	},
	{
		name: "lyra",
		title: "Lyra",
		description: "Boxy and sharp. Pairs well with mono fonts.",
	},
	{
		name: "mira",
		title: "Mira",
		description: "Compact. Made for dense interfaces.",
	},
	{
		name: "luma",
		title: "Luma",
		description: "Rounded geometry. Soft elevation. Breathable layouts.",
	},
	{
		name: "sera",
		title: "Sera",
		description: "Editorial and typographic.",
	},
	{
		name: "rhea",
		title: "Rhea",
		description: "Like Luma but compact.",
	},
] as const;

export type StyleName = (typeof STYLES)[number]["name"];

// ---------------------------------------------------------------------------
// Base colors: full token sets live in themes.ts (BASE_THEMES).
// ---------------------------------------------------------------------------

export const BASE_COLORS = [
	{ name: "neutral", title: "Neutral", color: "#737373" },
	{ name: "stone", title: "Stone", color: "#79716B" },
	{ name: "zinc", title: "Zinc", color: "#71717B" },
	{ name: "mauve", title: "Mauve", color: "#79697B" },
	{ name: "olive", title: "Olive", color: "#7C7C67" },
	{ name: "mist", title: "Mist", color: "#67787C" },
	{ name: "taupe", title: "Taupe", color: "#7C6D67" },
] as const;

export type BaseColorName = (typeof BASE_COLORS)[number]["name"];

/** All themes (base colors + colored override sets), keyed like upstream. */
export const ALL_THEMES = THEMES;

export type ThemeName = (typeof ALL_THEMES)[number]["name"];

export const THEMES_BY_NAME: Record<string, (typeof ALL_THEMES)[number]> = Object.fromEntries(
	ALL_THEMES.map((theme) => [theme.name, theme]),
);

// ---------------------------------------------------------------------------
// Chart colors: any theme name (upstream `PRESET_CHART_COLORS`), evaluated
// against the base color at composition time (getThemesForBaseColor).
// ---------------------------------------------------------------------------

export type ChartColorName = ThemeName;

// ---------------------------------------------------------------------------
// Radius
// ---------------------------------------------------------------------------

export const RADII = [
	{ name: "default", label: "Default", value: "0.5rem" },
	{ name: "none", label: "None", value: "0rem" },
	{ name: "small", label: "Small", value: "0.45rem" },
	{ name: "medium", label: "Medium", value: "0.625rem" },
	{ name: "large", label: "Large", value: "0.875rem" },
] as const;

export type RadiusName = (typeof RADII)[number]["name"];

// ---------------------------------------------------------------------------
// Menu (content styling of dropdown/select/context/command menus)
// ---------------------------------------------------------------------------

export const MENU_ACCENTS = [
	{ value: "subtle", label: "Subtle" },
	{ value: "bold", label: "Bold" },
] as const;

export type MenuAccentName = (typeof MENU_ACCENTS)[number]["value"];

export const MENU_COLORS = [
	{ value: "default", label: "Default / Solid" },
	{ value: "inverted", label: "Inverted" },
	{ value: "default-translucent", label: "Default Translucent" },
	{ value: "inverted-translucent", label: "Inverted Translucent" },
] as const;

export type MenuColorName = (typeof MENU_COLORS)[number]["value"];

// ---------------------------------------------------------------------------
// Fonts: id order MUST match upstream `PRESET_FONTS` (codec indexing).
// ---------------------------------------------------------------------------

export const FONTS = [
	{ name: "inter", title: "Inter", type: "sans", family: "'Inter Variable', sans-serif", dependency: "@fontsource-variable/inter" },
	{ name: "noto-sans", title: "Noto Sans", type: "sans", family: "'Noto Sans Variable', sans-serif", dependency: "@fontsource-variable/noto-sans" },
	{ name: "nunito-sans", title: "Nunito Sans", type: "sans", family: "'Nunito Sans Variable', sans-serif", dependency: "@fontsource-variable/nunito-sans" },
	{ name: "figtree", title: "Figtree", type: "sans", family: "'Figtree Variable', sans-serif", dependency: "@fontsource-variable/figtree" },
	{ name: "roboto", title: "Roboto", type: "sans", family: "'Roboto Variable', sans-serif", dependency: "@fontsource-variable/roboto" },
	{ name: "raleway", title: "Raleway", type: "sans", family: "'Raleway Variable', sans-serif", dependency: "@fontsource-variable/raleway" },
	{ name: "dm-sans", title: "DM Sans", type: "sans", family: "'DM Sans Variable', sans-serif", dependency: "@fontsource-variable/dm-sans" },
	{ name: "public-sans", title: "Public Sans", type: "sans", family: "'Public Sans Variable', sans-serif", dependency: "@fontsource-variable/public-sans" },
	{ name: "outfit", title: "Outfit", type: "sans", family: "'Outfit Variable', sans-serif", dependency: "@fontsource-variable/outfit" },
	{ name: "jetbrains-mono", title: "JetBrains Mono", type: "mono", family: "'JetBrains Mono Variable', monospace", dependency: "@fontsource-variable/jetbrains-mono" },
	{ name: "geist", title: "Geist", type: "sans", family: "'Geist Variable', sans-serif", dependency: "@fontsource-variable/geist" },
	{ name: "geist-mono", title: "Geist Mono", type: "mono", family: "'Geist Mono Variable', monospace", dependency: "@fontsource-variable/geist-mono" },
	{ name: "lora", title: "Lora", type: "serif", family: "'Lora Variable', serif", dependency: "@fontsource-variable/lora" },
	{ name: "merriweather", title: "Merriweather", type: "serif", family: "'Merriweather Variable', serif", dependency: "@fontsource-variable/merriweather" },
	{ name: "playfair-display", title: "Playfair Display", type: "serif", family: "'Playfair Display Variable', serif", dependency: "@fontsource-variable/playfair-display" },
	{ name: "noto-serif", title: "Noto Serif", type: "serif", family: "'Noto Serif Variable', serif", dependency: "@fontsource-variable/noto-serif" },
	{ name: "roboto-slab", title: "Roboto Slab", type: "serif", family: "'Roboto Slab Variable', serif", dependency: "@fontsource-variable/roboto-slab" },
	{ name: "oxanium", title: "Oxanium", type: "sans", family: "'Oxanium Variable', sans-serif", dependency: "@fontsource-variable/oxanium" },
	{ name: "manrope", title: "Manrope", type: "sans", family: "'Manrope Variable', sans-serif", dependency: "@fontsource-variable/manrope" },
	{ name: "space-grotesk", title: "Space Grotesk", type: "sans", family: "'Space Grotesk Variable', sans-serif", dependency: "@fontsource-variable/space-grotesk" },
	{ name: "montserrat", title: "Montserrat", type: "sans", family: "'Montserrat Variable', sans-serif", dependency: "@fontsource-variable/montserrat" },
	{ name: "ibm-plex-sans", title: "IBM Plex Sans", type: "sans", family: "'IBM Plex Sans Variable', sans-serif", dependency: "@fontsource-variable/ibm-plex-sans" },
	{ name: "source-sans-3", title: "Source Sans 3", type: "sans", family: "'Source Sans 3 Variable', sans-serif", dependency: "@fontsource-variable/source-sans-3" },
	{ name: "instrument-sans", title: "Instrument Sans", type: "sans", family: "'Instrument Sans Variable', sans-serif", dependency: "@fontsource-variable/instrument-sans" },
	{ name: "eb-garamond", title: "EB Garamond", type: "serif", family: "'EB Garamond Variable', serif", dependency: "@fontsource-variable/eb-garamond" },
	{ name: "instrument-serif", title: "Instrument Serif", type: "serif", family: "'Instrument Serif', serif", dependency: "@fontsource/instrument-serif" },
] as const;

export type FontName = (typeof FONTS)[number]["name"];

export type FontHeadingName = FontName | "inherit";

/** Heading options = "inherit" (use the body font) + every body font. */
export const FONT_HEADINGS = [
	{ name: "inherit", title: "Inherit body font", family: "", dependency: "" },
	...FONTS,
] as const;

const FONTS_BY_NAME: Record<string, (typeof FONTS)[number]> = Object.fromEntries(
	FONTS.map((font) => [font.name, font]),
);

/** Resolves a font id (or "inherit" → null) to its definition. */
export function getFont(name: FontHeadingName): (typeof FONTS)[number] | null {
	return name === "inherit" ? null : (FONTS_BY_NAME[name] ?? null);
}

// ---------------------------------------------------------------------------
// Icon libraries: ids MUST match upstream codec order
// (lucide, tabler, hugeicons, phosphor, remixicon). Each entry maps a
// semantic app icon to the per-set Iconify slug, resolved by
// `src/lib/components/icons/library.ts`.
// ---------------------------------------------------------------------------

export const ICON_LIBRARIES = [
	{ name: "lucide", title: "Lucide" },
	{ name: "tabler", title: "Tabler" },
	{ name: "hugeicons", title: "HugeIcons" },
	{ name: "phosphor", title: "Phosphor" },
	{ name: "remixicon", title: "Remix Icon" },
] as const;

export type IconLibraryName = (typeof ICON_LIBRARIES)[number]["name"];

// ---------------------------------------------------------------------------
// Theme/base-color lookups (mirror upstream config.ts helpers)
// ---------------------------------------------------------------------------

// Membership checks accept any theme name (ThemeName ⊇ BaseColorName).
const BASE_COLOR_NAMES = new Set<string>(BASE_COLORS.map((base) => base.name));

/** Themes selectable for a base color: every colored theme + the base itself. */
export function getThemesForBaseColor(baseColor: BaseColorName) {
	return ALL_THEMES.filter(
		(theme) => theme.name === baseColor || !BASE_COLOR_NAMES.has(theme.name),
	);
}

export function getBaseColor(name: BaseColorName) {
	return BASE_THEMES.find((base) => base.name === name);
}

export function getTheme(name: ThemeName) {
	return THEMES_BY_NAME[name];
}

/**
 * Swatch color for pickers, matching the /create pickers: base colors
 * preview `muted-foreground`, colored themes preview `primary`.
 */
export function getThemeSwatch(name: ThemeName, scheme: "light" | "dark"): string {
	const theme = getTheme(name);
	if (!theme) return "transparent";
	const key = BASE_COLOR_NAMES.has(name) ? "muted-foreground" : "primary";
	return theme.cssVars[scheme][key] ?? "transparent";
}
