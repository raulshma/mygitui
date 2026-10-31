/**
 * Preset codec for appearance configs — a browser-safe port of
 * `shadcn-svelte/preset` (packages/cli/src/preset/preset.ts, "BROWSER SAFE").
 *
 * Codes are bit-packed field indexes, base62-encoded with a version prefix
 * ("a" v1: no chartColor/fontHeading, "b" v2: all fields). Codes are
 * compatible with shadcn-svelte.com/create share links both ways.
 *
 * Backward-compat rules (upstream): never reorder value arrays — only append;
 * new fields get their default at index 0; appended after existing fields;
 * stay under 53 bits.
 */
import {
	BASE_COLORS,
	FONTS,
	FONT_HEADINGS,
	ICON_LIBRARIES,
	MENU_ACCENTS,
	MENU_COLORS,
	RADII,
	STYLES,
	type BaseColorName,
	type ChartColorName,
	type FontHeadingName,
	type FontName,
	type IconLibraryName,
	type MenuAccentName,
	type MenuColorName,
	type RadiusName,
	type StyleName,
	type ThemeName,
	getThemesForBaseColor,
} from "./values.js";

/** The full customizer state (one /create column). */
export type AppearanceConfig = {
	style: StyleName;
	baseColor: BaseColorName;
	theme: ThemeName;
	chartColor: ChartColorName;
	iconLibrary: IconLibraryName;
	font: FontName;
	fontHeading: FontHeadingName;
	radius: RadiusName;
	menuAccent: MenuAccentName;
	menuColor: MenuColorName;
};

// Upstream PRESET_THEMES order (base colors first, then colored themes) —
// NOT the same as themes.ts order, so spell it out.
const THEMES_FOR_CODEC = [
	"neutral",
	"stone",
	"zinc",
	"amber",
	"blue",
	"cyan",
	"emerald",
	"fuchsia",
	"green",
	"indigo",
	"lime",
	"orange",
	"pink",
	"purple",
	"red",
	"rose",
	"sky",
	"teal",
	"violet",
	"yellow",
	"mauve",
	"olive",
	"mist",
	"taupe",
] as const;

/**
 * Codec value arrays in upstream index order. Order is load-bearing — see
 * the file docstring.
 */
const CODEC_FIELDS = [
	{ key: "menuColor", values: MENU_COLORS.map((m) => m.value), bits: 3 },
	{ key: "menuAccent", values: MENU_ACCENTS.map((m) => m.value), bits: 3 },
	{ key: "radius", values: RADII.map((r) => r.name), bits: 4 },
	{ key: "font", values: FONTS.map((f) => f.name), bits: 6 },
	{
		key: "iconLibrary",
		values: ICON_LIBRARIES.map((i) => i.name),
		bits: 6,
	},
	{ key: "theme", values: THEMES_FOR_CODEC, bits: 6 },
	{ key: "baseColor", values: BASE_COLORS.map((b) => b.name), bits: 6 },
	{ key: "style", values: STYLES.map((s) => s.name), bits: 6 },
	{ key: "chartColor", values: THEMES_FOR_CODEC, bits: 6 },
	{ key: "fontHeading", values: FONT_HEADINGS.map((f) => f.name), bits: 5 },
] as const;

export const DEFAULT_APPEARANCE: AppearanceConfig = {
	// The vendored ui components were generated from the nova registry —
	// defaulting here keeps the pre-customizer look byte-identical.
	style: "nova",
	baseColor: "neutral",
	theme: "neutral",
	chartColor: "neutral",
	iconLibrary: "lucide",
	font: "inter",
	fontHeading: "inherit",
	radius: "default",
	menuAccent: "subtle",
	menuColor: "default",
};

// ---------------------------------------------------------------------------
// base62
// ---------------------------------------------------------------------------

const BASE62 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

function toBase62(num: number): string {
	if (num === 0) return "0";
	let result = "";
	let n = num;
	while (n > 0) {
		result = BASE62[n % 62] + result;
		n = Math.floor(n / 62);
	}
	return result;
}

function fromBase62(str: string): number {
	let result = 0;
	for (let i = 0; i < str.length; i++) {
		const idx = BASE62.indexOf(str[i]);
		if (idx === -1) return -1;
		result = result * 62 + idx;
	}
	return result;
}

// ---------------------------------------------------------------------------
// Encode / decode
// ---------------------------------------------------------------------------

const CURRENT_VERSION = "b";
const VALID_VERSIONS = new Set(["a", "b"]);

/** Encodes a config (missing fields fall back to defaults) as a preset code. */
export function encodeAppearance(config: Partial<AppearanceConfig>): string {
	const merged: Record<string, string> = { ...DEFAULT_APPEARANCE, ...config };

	// Multiplication, not bitwise ops: JS bitwise truncates to 32 bits.
	let bits = 0;
	let offset = 0;
	for (const field of CODEC_FIELDS) {
		const value = merged[field.key];
		const idx = value === undefined ? -1 : field.values.indexOf(value as never);
		bits += (idx === -1 ? 0 : idx) * 2 ** offset;
		offset += field.bits;
	}

	return CURRENT_VERSION + toBase62(bits);
}

/**
 * Decodes a preset code ("b" v2, or legacy "a" v1 from early /create
 * links) into a config. Returns null for malformed/unknown codes.
 */
export function decodeAppearance(code: string): AppearanceConfig | null {
	if (!code || code.length < 2) return null;

	const version = code[0];
	if (!VALID_VERSIONS.has(version)) return null;

	const bits = fromBase62(code.slice(1));
	if (bits < 0) return null;

	const result: Record<string, string> = {};
	let offset = 0;
	for (const field of CODEC_FIELDS) {
		// v1 codes predate chartColor/fontHeading; skip fields past their width.
		if (version === "a" && (field.key === "chartColor" || field.key === "fontHeading")) {
			continue;
		}
		const idx = Math.floor(bits / 2 ** offset) % 2 ** field.bits;
		result[field.key] = field.values[idx] ?? field.values[0];
		offset += field.bits;
	}

	if (version === "a") {
		result.fontHeading = "inherit";
	}

	return normalizeAppearance(result as Partial<AppearanceConfig>);
}

/** Cheap shape check for user-pasted values. */
export function isPresetCode(value: string): boolean {
	if (!value || value.length < 2 || value.length > 10) return false;
	if (!VALID_VERSIONS.has(value[0])) return false;
	for (let i = 1; i < value.length; i++) {
		if (!BASE62.includes(value[i])) return false;
	}
	return true;
}

// ---------------------------------------------------------------------------
// Normalization: keep a config self-consistent as pickers change it.
// ---------------------------------------------------------------------------

function firstAvailable<T>(value: unknown, available: readonly T[], fallback: T): T {
	return available.includes(value as never) ? (value as T) : fallback;
}

/**
 * Coerces an arbitrary partial config into a valid one: unknown values fall
 * back to defaults, theme/chartColor are constrained to the base color's
 * available set, and per-style radius quirks are enforced (lyra/sera are
 * square, rhea has no large radius) — matching upstream's normalizeConfig.
 */
export function normalizeAppearance(
	partial: Partial<AppearanceConfig>,
): AppearanceConfig {
	const merged: AppearanceConfig = { ...DEFAULT_APPEARANCE, ...partial };

	const style = firstAvailable(
		merged.style,
		STYLES.map((s) => s.name),
		DEFAULT_APPEARANCE.style,
	);
	const baseColor = firstAvailable(
		merged.baseColor,
		BASE_COLORS.map((b) => b.name),
		DEFAULT_APPEARANCE.baseColor,
	);
	const available = getThemesForBaseColor(baseColor).map((t) => t.name);

	const theme = firstAvailable(merged.theme, available, baseColor);
	const chartColor = firstAvailable(merged.chartColor, available, baseColor);

	const radiusRaw = firstAvailable(
		merged.radius,
		RADII.map((r) => r.name),
		DEFAULT_APPEARANCE.radius,
	);
	const radius = normalizeRadius(style, radiusRaw);

	const fontHeading = firstAvailable(
		merged.fontHeading,
		FONT_HEADINGS.map((f) => f.name),
		DEFAULT_APPEARANCE.fontHeading,
	);

	return {
		style,
		baseColor,
		theme,
		chartColor,
		iconLibrary: firstAvailable(
			merged.iconLibrary,
			ICON_LIBRARIES.map((i) => i.name),
			DEFAULT_APPEARANCE.iconLibrary,
		),
		font: firstAvailable(
			merged.font,
			FONTS.map((f) => f.name),
			DEFAULT_APPEARANCE.font,
		),
		fontHeading,
		radius,
		menuAccent: firstAvailable(
			merged.menuAccent,
			MENU_ACCENTS.map((m) => m.value),
			DEFAULT_APPEARANCE.menuAccent,
		),
		menuColor: firstAvailable(
			merged.menuColor,
			MENU_COLORS.map((m) => m.value),
			DEFAULT_APPEARANCE.menuColor,
		),
	};
}

/** Per-style radius constraints from the /create preview (lyra/sera/rhea). */
function normalizeRadius(style: StyleName, radius: RadiusName): RadiusName {
	if (style === "lyra" || style === "sera") return "none";
	if (style === "rhea" && radius === "large") return "default";
	return radius;
}

/** Picks a random valid config (the "Shuffle" button). */
export function randomAppearance(): AppearanceConfig {
	const pick = <T>(arr: readonly T[]): T => arr[Math.floor(Math.random() * arr.length)];
	const style = pick(STYLES).name;
	const baseColor = pick(BASE_COLORS).name;
	const available = getThemesForBaseColor(baseColor);
	return normalizeAppearance({
		style,
		baseColor,
		theme: pick(available).name,
		chartColor: pick(available).name,
		iconLibrary: pick(ICON_LIBRARIES).name,
		font: pick(FONTS).name,
		fontHeading: pick(FONT_HEADINGS.map((f) => f.name)),
		radius: pick(RADII).name,
		menuAccent: pick(MENU_ACCENTS).value,
		menuColor: pick(MENU_COLORS).value,
	});
}
