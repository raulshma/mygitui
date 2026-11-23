/**
 * Contrast guardrails for the appearance engine: WCAG 2.x contrast ratios
 * computed straight from the oklch token strings the customizer emits, with
 * lightness-only repair passes that keep a token's hue (and its look) while
 * making it readable.
 *
 * The customizer's hand-tuned presets assume paired use — `--primary` fills
 * always sit under `--primary-foreground` text, etc. — but nothing enforced
 * it: several theme ramps pair a near-white yellow fill with a light
 * foreground (≈1.6:1), and colored primaries used as *ink* on the background
 * (chips, links, ref badges) fall far below 4.5:1 for light yellows and
 * dark reds alike. The composer runs these helpers over the composed token
 * sets so every fill/foreground pair and the derived `--primary-ink` /
 * `--primary-hover` variants are readable in both schemes, whatever theme
 * colors the user picks.
 *
 * Pure string-in/string-out: unparsable values (color-mix, named colors,
 * alpha-carrying overlays) pass through untouched — a missed repair must
 * never corrupt a working token.
 */

/** One parsed `oklch(L C H[/A])` color. Alpha `null` means "not specified". */
export interface Oklch {
	l: number;
	c: number;
	h: number;
	alpha: number | null;
}

const OKLCH_RE =
	/^oklch\(\s*([\d.]+)(%?)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+%?)\s*)?\)$/i;

/** Parses the oklch subset the theme tokens use; `null` for anything else. */
export function parseOklch(value: string): Oklch | null {
	const match = OKLCH_RE.exec(value.trim());
	if (!match) return null;
	let l = Number(match[1]);
	if (match[2] === "%") l /= 100;
	let alpha: number | null = null;
	if (match[5] !== undefined) {
		alpha = match[5].endsWith("%")
			? Number(match[5].slice(0, -1)) / 100
			: Number(match[5]);
	}
	return { l, c: Number(match[3]), h: Number(match[4]), alpha };
}

/** Renders back to the `oklch()` string form the token sets use. */
export function oklchToCss(color: Oklch): string {
	const { l, c, h, alpha } = color;
	const base = `oklch(${round(l, 4)} ${round(c, 4)} ${round(h, 2)})`;
	return alpha === null ? base : `${base} / ${round(alpha, 4)}`;
}

function round(value: number, digits: number): string {
	return String(Number(value.toFixed(digits)));
}

/** oklch → linear-light sRGB (Björn Ottosson's oklab matrices). */
function oklchToLinearSrgb({ l, c, h }: Oklch): [number, number, number] {
	const cosH = Math.cos((h * Math.PI) / 180);
	const sinH = Math.sin((h * Math.PI) / 180);
	const la = l + 0.3963377774 * c * cosH + 0.2158037573 * c * sinH;
	const mb = l - 0.1055613458 * c * cosH - 0.0638541728 * c * sinH;
	const sc = l - 0.0894841775 * c * cosH - 1.291485548 * c * sinH;
	const L = la ** 3;
	const M = mb ** 3;
	const S = sc ** 3;
	return [
		4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S,
		-1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S,
		-0.0041960863 * L - 0.7034186147 * M + 1.707614701 * S,
	];
}

function linearToSrgb(channel: number): number {
	const v = Math.min(Math.max(channel, 0), 1);
	return v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055;
}

/** WCAG 2.x relative luminance of an oklch color (gamma-encoded channels). */
export function relativeLuminance(color: Oklch): number {
	const [r, g, b] = oklchToLinearSrgb(color);
	return (
		0.2126 * linearToSrgb(r) + 0.7152 * linearToSrgb(g) + 0.0722 * linearToSrgb(b)
	);
}

/** WCAG 2.x contrast ratio; `null` when either side is not oklch. */
export function contrastRatio(a: string, b: string): number | null {
	const ca = parseOklch(a);
	const cb = parseOklch(b);
	if (!ca || !cb) return null;
	const la = relativeLuminance(ca);
	const lb = relativeLuminance(cb);
	const [hi, lo] = la >= lb ? [la, lb] : [lb, la];
	return (hi + 0.05) / (lo + 0.05);
}

/** Contrast of `color` (defaulting its alpha over black, like CSS does). */
function luminanceOf(color: Oklch): number {
	return color.alpha !== null && color.alpha < 1
		? relativeLuminance(color) * color.alpha
		: relativeLuminance(color);
}

/**
 * The best achievable contrast for a hue/chroma pair against `bg`: scan the
 * lightness axis finely and take the max (chroma can make luminance
 * non-monotonic in-gamut, so a plain endpoint check is not enough).
 */
function bestContrast(c: number, h: number, bg: Oklch): { l: number; ratio: number } {
	const bgLum = luminanceOf(bg);
	let best = { l: 0, ratio: 0 };
	for (let i = 0; i <= 100; i++) {
		const l = i / 100;
		const lum = relativeLuminance({ l, c, h, alpha: null });
		const ratio = (Math.max(lum, bgLum) + 0.05) / (Math.min(lum, bgLum) + 0.05);
		if (ratio > best.ratio) best = { l, ratio };
	}
	return best;
}

/**
 * The readable variant of `color` against `bg`: the same hue at the closest
 * lightness that reaches `target` contrast — a foreground pair that already
 * passes is returned untouched (hand-tuned presets keep their exact
 * strings), one that falls short is repaired; when chroma makes the target
 * unreachable the fallback desaturates toward gray. Used both to pair
 * `*-foreground` tokens with their fills and to derive `--primary-ink`
 * (chips, links and badges that paint `--primary` as TEXT stay legible for
 * any theme color). Unparsable inputs pass through.
 */
export function inkFor(color: string, bg: string, target = 4.5): string {
	const parsed = parseOklch(color);
	const bgParsed = parseOklch(bg);
	if (!parsed || !bgParsed) return color;
	const ratio = contrastRatio(color, bg);
	if (ratio !== null && ratio >= target) return color;
	const repaired = repair(parsed, bgParsed, target);
	return repaired === null ? color : oklchToCss(repaired);
}

function repair(color: Oklch, bg: Oklch, target: number): Oklch | null {
	const bgLum = luminanceOf(bg);
	// Direction: away from the background on the lightness axis (darker text
	// on a lighter surface, lighter text on a darker one) — that is the only
	// direction contrast grows in.
	const towardLight = luminanceOf(color) >= bgLum;
	const passes = (cand: Oklch): boolean => {
		const lum = relativeLuminance(cand);
		const ratio = (Math.max(lum, bgLum) + 0.05) / (Math.min(lum, bgLum) + 0.05);
		return ratio >= target;
	};
	let lo = towardLight ? color.l : 0;
	let hi = towardLight ? 1 : color.l;
	let best: Oklch | null = null;
	for (let i = 0; i < 24; i++) {
		const mid = (lo + hi) / 2;
		const cand: Oklch = { ...color, l: mid };
		if (passes(cand)) {
			best = cand;
			// Keep moving toward the ORIGINAL lightness (smaller change wins).
			if (towardLight) hi = mid;
			else lo = mid;
		} else if (towardLight) {
			lo = mid;
		} else {
			hi = mid;
		}
		if (hi - lo < 0.002) break;
	}
	if (best) return best;
	// Chroma fallback: gray-ward at the best-scan lightness until it passes.
	const { l } = bestContrast(color.c, color.h, bg);
	for (let k = 0; k <= 20; k++) {
		const cand: Oklch = { l, c: color.c * (1 - k / 20), h: color.h, alpha: color.alpha };
		if (passes(cand)) return cand;
	}
	return null;
}

/**
 * The hover variant of a fill: one lightness step away from its own
 * foreground, so hover grows text contrast instead of eating it — the old
 * `bg-primary/80` / `brightness-110` hovers composited light fills (yellow)
 * toward white and away from their dark text, dropping the pair below 4.5:1.
 * Light-mode buttons darken on hover, dark-mode buttons lighten — the
 * standard GitHub/Bootstrap press-in look, and hue and chroma are untouched.
 * Unparsable inputs pass through.
 */
export function hoverShift(color: string, fg: string, step = 0.04): string {
	const parsed = parseOklch(color);
	const fgParsed = parseOklch(fg);
	if (!parsed || !fgParsed) return color;
	const l = Math.min(1, Math.max(0, parsed.l + (fgParsed.l >= parsed.l ? -step : step)));
	return oklchToCss({ ...parsed, l });
}
