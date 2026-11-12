/**
 * Lane palette for the commit graph, resolved from the app's design tokens.
 *
 * Canvas cannot resolve CSS `var()` or `color-mix()`, and the token sets in
 * src/app.css and the appearance presets are written in `oklch()` — so every
 * token is converted to a concrete sRGB hex here. Tokens are read ONCE per
 * theme change (see GraphCanvas' theme watcher), never per frame.
 *
 * Lanes are derived from `--primary` by rotating hue in OKLCH, so the graph
 * belongs to the active accent instead of a fixed palette, and six lanes stay
 * separable by hue *and* luminance. Reading `--muted`/`--secondary` directly
 * would not work: in the neutral base they are the same near-white value, so
 * two of the three lanes would be indistinguishable (and invisible on a light
 * surface).
 *
 * Static fallbacks keep the graph readable before `initTheme()` runs, or
 * under jsdom where computed styles resolve nothing.
 */

export type RGB = readonly [number, number, number];

export interface ThemeColors {
  /** Lane colors indexed by `laneColorIndex(lane)`; lane 0 is the accent. */
  lanes: readonly string[];
  surface: string;
  outline: string;
}

/** Baseline palette (light scheme, seed #6750a4) — used before tokens land. */
export const FALLBACK_THEME: ThemeColors = {
  lanes: ["#65558f", "#3f6f8f", "#4f7a52", "#8f6a3f", "#8f4f6a", "#5a5a9e"],
  surface: "#fdf7ff",
  outline: "#7a757f",
};

/** Seed accent of the baseline palette above, for hex-only consumers. */
export const FALLBACK_ACCENT = "#6750a4";

/** How many distinct lanes the palette carries before it repeats. */
export const LANE_COUNT = 6;

const HEX_RE = /^#([0-9a-f]+)$/i;

/** Parse `#rgb`, `#rrggbb` (and `#rrggbbaa`, alpha ignored) → RGB. */
export function parseHex(color: string): RGB | null {
  if (typeof color !== "string") return null;
  const match = HEX_RE.exec(color.trim());
  if (!match) return null;
  let digits = match[1];
  if (digits.length === 3 || digits.length === 4) {
    digits = digits
      .slice(0, 3)
      .split("")
      .map((c) => c + c)
      .join("");
  }
  if (digits.length === 6) {
    return [
      parseInt(digits.slice(0, 2), 16),
      parseInt(digits.slice(2, 4), 16),
      parseInt(digits.slice(4, 6), 16),
    ];
  }
  if (digits.length === 8) {
    return [
      parseInt(digits.slice(0, 2), 16),
      parseInt(digits.slice(2, 4), 16),
      parseInt(digits.slice(4, 6), 16),
    ];
  }
  return null;
}

function toHex(rgb: RGB): string {
  const hex = rgb.map((c) => c.toString(16).padStart(2, "0")).join("");
  return `#${hex}`;
}

/** `rgba()` string from a hex token; non-hex input is passed through as-is. */
export function withAlpha(color: string, alpha: number): string {
  const rgb = parseHex(color);
  if (!rgb) return color;
  return `rgba(${rgb[0]}, ${rgb[1]}, ${rgb[2]}, ${alpha})`;
}

/** Linear sRGB mix of two hex colors; `t=0` → a, `t=1` → b. */
export function mixHex(a: string, b: string, t: number): string {
  const ca = parseHex(a);
  const cb = parseHex(b);
  if (!ca || !cb) return a;
  const k = Math.min(1, Math.max(0, t));
  return toHex([
    Math.round(ca[0] + (cb[0] - ca[0]) * k),
    Math.round(ca[1] + (cb[1] - ca[1]) * k),
    Math.round(ca[2] + (cb[2] - ca[2]) * k),
  ]);
}

// ---------------------------------------------------------------------------
// CSS color resolution (oklch is what the token sets actually ship)
// ---------------------------------------------------------------------------

/** A CSS color decomposed into sRGB plus its alpha (0–1). */
export interface ResolvedColor {
  rgb: RGB;
  alpha: number;
}

const OKLCH_RE =
  /^oklch\(\s*([0-9.]+%?)\s+([0-9.]+%?)\s+([0-9.]+)(?:deg)?\s*(?:\/\s*([0-9.]+%?))?\s*\)$/i;
const RGB_RE =
  /^rgba?\(\s*([0-9.]+)\s*,\s*([0-9.]+)\s*,\s*([0-9.]+)\s*(?:,\s*([0-9.]+%?))?\s*\)$/i;

function clamp01(n: number): number {
  return n < 0 ? 0 : n > 1 ? 1 : n;
}

/** `"50%"` → 0.5, `"0.5"` → 0.5; `percentScale` maps 100% → that value. */
function numeric(token: string, percentScale = 1): number {
  if (token.endsWith("%")) return (parseFloat(token) / 100) * percentScale;
  return parseFloat(token);
}

/** OKLCH → OKLab → linear sRGB → sRGB (Ottosson's matrices). */
export function oklchToRgb(l: number, c: number, h: number): RGB {
  const rad = (h * Math.PI) / 180;
  const a = c * Math.cos(rad);
  const b = c * Math.sin(rad);

  const l_ = l + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = l - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = l - 0.0894841775 * a - 1.291485548 * b;

  const lc = l_ * l_ * l_;
  const mc = m_ * m_ * m_;
  const sc = s_ * s_ * s_;

  const linear = [
    4.0767416621 * lc - 3.3077115913 * mc + 0.2309699292 * sc,
    -1.2684380046 * lc + 2.6097574011 * mc - 0.3413193965 * sc,
    -0.0041960863 * lc - 0.7034186147 * mc + 1.707614701 * sc,
  ];

  const channels = linear.map((channel) => {
    const v = clamp01(channel);
    const srgb = v <= 0.0031308 ? 12.92 * v : 1.055 * Math.pow(v, 1 / 2.4) - 0.055;
    return Math.round(clamp01(srgb) * 255);
  });
  return [channels[0]!, channels[1]!, channels[2]!];
}

/** sRGB → OKLCH (inverse of {@link oklchToRgb}); lets hex tokens seed lanes. */
export function rgbToOklch(rgb: RGB): { l: number; c: number; h: number } {
  const linear = rgb.map((channel) => {
    const v = channel / 255;
    return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
  });
  const r = linear[0]!;
  const g = linear[1]!;
  const b = linear[2]!;

  const l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
  const m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
  const s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

  const l_ = Math.cbrt(l);
  const m_ = Math.cbrt(m);
  const s_ = Math.cbrt(s);

  const L = 0.2104542553 * l_ + 0.793617785 * m_ - 0.0040720468 * s_;
  const A = 1.9779984951 * l_ - 2.428592205 * m_ + 0.4505937099 * s_;
  const B = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.808675766 * s_;

  const C = Math.sqrt(A * A + B * B);
  let H = (Math.atan2(B, A) * 180) / Math.PI;
  if (H < 0) H += 360;
  return { l: L, c: C, h: H };
}

/**
 * Resolves a CSS color token to concrete sRGB + alpha. Handles the three
 * forms the app's tokens use (`oklch()`, `#hex`, `rgb()/rgba()`); anything
 * else (`color-mix()`, `var()`, named colors) returns null so the caller
 * falls back rather than painting a wrong color.
 */
export function resolveColor(value: string): ResolvedColor | null {
  if (typeof value !== "string") return null;
  const token = value.trim();
  if (token === "") return null;

  const hex = parseHex(token);
  if (hex) {
    const alpha = /^#[0-9a-f]{8}$/i.test(token)
      ? parseInt(token.slice(7, 9), 16) / 255
      : 1;
    return { rgb: hex, alpha };
  }

  const oklch = OKLCH_RE.exec(token);
  if (oklch) {
    const l = numeric(oklch[1]!);
    const c = numeric(oklch[2]!, 0.4);
    const h = parseFloat(oklch[3]!);
    if (!Number.isFinite(l) || !Number.isFinite(c) || !Number.isFinite(h)) return null;
    const alpha = oklch[4] === undefined ? 1 : clamp01(numeric(oklch[4]));
    return { rgb: oklchToRgb(clamp01(l), Math.max(0, c), h), alpha };
  }

  const rgb = RGB_RE.exec(token);
  if (rgb) {
    const channels = [rgb[1]!, rgb[2]!, rgb[3]!].map((n) =>
      Math.round(clamp01(parseFloat(n) / 255) * 255),
    );
    if (channels.some((n) => !Number.isFinite(n))) return null;
    const alpha = rgb[4] === undefined ? 1 : clamp01(numeric(rgb[4]));
    return { rgb: [channels[0]!, channels[1]!, channels[2]!], alpha };
  }

  return null;
}

/** Composites a semi-transparent color over an opaque one (source-over). */
function composite(fg: ResolvedColor, bg: RGB): RGB {
  if (fg.alpha >= 1) return fg.rgb;
  const a = fg.alpha;
  return [
    Math.round(fg.rgb[0] * a + bg[0] * (1 - a)),
    Math.round(fg.rgb[1] * a + bg[1] * (1 - a)),
    Math.round(fg.rgb[2] * a + bg[2] * (1 - a)),
  ];
}

// ---------------------------------------------------------------------------
// Lane derivation
// ---------------------------------------------------------------------------

/**
 * Lightness offset per lane, so adjacent lanes differ in luminance as well
 * as hue — they stay apart for color-blind users and on monochrome displays.
 */
/**
 * Where each generated lane sits inside the surface's lightness band, as a
 * fraction. Spread across the band rather than offset from the accent: the
 * default theme's accent is near-black (light) or near-white (dark), so an
 * accent-relative offset clamps every lane onto the same band edge and the
 * graph loses its lightness separation.
 */
const LANE_L_POSITIONS = [0.5, 0.78, 0.28, 0.92, 0.12, 0.62];
/** Neutral accents carry no chroma; lanes still need hue to be separable. */
const MIN_LANE_CHROMA = 0.12;

/**
 * Builds `LANE_COUNT` separable lane colors from the accent. Hue rotates a
 * full turn over the palette; lightness spreads across a band that contrasts
 * with the surface. Lane 0 stays the accent verbatim.
 */
export function deriveLanes(
  accent: { l: number; c: number; h: number },
  surfaceIsDark: boolean,
): string[] {
  const lo = surfaceIsDark ? 0.56 : 0.3;
  const hi = surfaceIsDark ? 0.86 : 0.66;
  const chroma = Math.max(accent.c, MIN_LANE_CHROMA);
  const lanes: string[] = [];
  for (let i = 0; i < LANE_COUNT; i += 1) {
    const hue = (accent.h + (360 / LANE_COUNT) * i) % 360;
    const lightness = lo + (hi - lo) * LANE_L_POSITIONS[i]!;
    lanes.push(toHex(oklchToRgb(lightness, chroma, hue)));
  }
  lanes[0] = toHex(oklchToRgb(accent.l, accent.c, accent.h));
  return lanes;
}

/** Anything with `getPropertyValue` — i.e. `getComputedStyle()` output. */
export interface StyleLike {
  getPropertyValue(name: string): string;
}

/**
 * Reads the lane palette + surface/outline tokens from computed styles and
 * converts them to canvas-safe hex. Missing or unresolvable tokens fall back
 * to the baseline palette, so the graph never paints an invalid color.
 */
export function readTheme(styles: StyleLike | null): ThemeColors {
  const read = (name: string): ResolvedColor | null =>
    resolveColor(styles?.getPropertyValue(name) ?? "");

  const accent = read("--primary");
  const surfaceColor = read("--card") ?? read("--background");
  if (!accent || !surfaceColor) return FALLBACK_THEME;

  const surface = composite(surfaceColor, [255, 255, 255]);
  const surfaceIsDark = rgbToOklch(surface).l < 0.5;
  // A translucent border (`oklch(1 0 0 / 10%)` in dark) must composite over
  // the surface, not become a solid white rule across the graph.
  const outlineColor = read("--border");
  const outline = outlineColor
    ? composite(outlineColor, surface)
    : surfaceIsDark
      ? ([255, 255, 255] as RGB)
      : ([0, 0, 0] as RGB);

  return {
    lanes: deriveLanes(rgbToOklch(accent.rgb), surfaceIsDark),
    surface: toHex(surface),
    outline: toHex(outline),
  };
}

/**
 * The live `--primary` accent as a concrete sRGB hex, for consumers that
 * need a hex string (HTML `<input type="color">` can't take `oklch()`).
 * Falls back to {@link FALLBACK_ACCENT} when the token is missing or
 * unresolvable.
 */
export function primaryHex(styles?: StyleLike): string {
  const raw = (styles ?? getComputedStyle(document.documentElement)).getPropertyValue(
    "--primary",
  );
  const resolved = resolveColor(raw);
  return resolved ? toHex(resolved.rgb) : FALLBACK_ACCENT;
}

/**
 * Stable lane → palette-slot mapping: `lane % paletteSize`, non-negative for
 * any integer input. Pure, so the mapping is identical across pages/frames.
 */
export function laneColorIndex(lane: number, paletteSize = LANE_COUNT): number {
  const n = Math.max(1, Math.floor(paletteSize));
  const laneInt = Math.floor(lane);
  return ((laneInt % n) + n) % n;
}

/** Resolve the concrete color for a lane index. */
export function laneColor(lane: number, palette: readonly string[]): string {
  const color = palette[laneColorIndex(lane, palette.length)];
  return color ?? palette[0] ?? "#888888";
}
