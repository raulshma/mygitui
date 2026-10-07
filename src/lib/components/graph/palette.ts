/**
 * Lane palette for the commit graph, derived from M3 design tokens.
 *
 * Lane color scheme (B2 contract): lane `n` picks from
 * `[--m3-primary, --m3-tertiary, --m3-secondary]` by `n % 3`. Tokens are read
 * ONCE per theme change via `getComputedStyle(documentElement)` (see
 * GraphCanvas' theme watcher) — never per frame. Canvas cannot resolve CSS
 * `var()` or `color-mix()`, so everything here works on plain hex/rgba strings.
 *
 * Static fallbacks mirror the baseline palette in src/app.css so the graph is
 * still readable before `initTheme()` runs (or under jsdom).
 */

export type RGB = readonly [number, number, number];

export interface ThemeColors {
  /** Lane colors indexed by `laneColorIndex(lane)`: [primary, tertiary, secondary]. */
  lanes: readonly [string, string, string];
  surface: string;
  outline: string;
}

/** Baseline M3 tokens (light scheme, seed #6750a4) — see src/app.css. */
export const FALLBACK_THEME: ThemeColors = {
  lanes: ["#65558f", "#7e5260", "#625b71"],
  surface: "#fdf7ff",
  outline: "#7a757f",
};

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

/** Anything with `getPropertyValue` — i.e. `getComputedStyle()` output. */
export interface StyleLike {
  getPropertyValue(name: string): string;
}

/**
 * Read the lane palette + accent tokens from computed styles. Missing or
 * non-hex tokens fall back to the baseline palette. The order is part of the
 * contract: lanes = [primary, tertiary, secondary].
 */
export function readTheme(styles: StyleLike | null): ThemeColors {
  const pick = (name: string, fallback: string): string => {
    const value = styles?.getPropertyValue(name)?.trim();
    return value && parseHex(value) ? value : fallback;
  };
  const f = FALLBACK_THEME;
  return {
    lanes: [pick("--m3-primary", f.lanes[0]), pick("--m3-tertiary", f.lanes[1]), pick("--m3-secondary", f.lanes[2])],
    surface: pick("--m3-surface", f.surface),
    outline: pick("--m3-outline", f.outline),
  };
}

/**
 * Stable lane → palette-slot mapping: `lane % paletteSize`, non-negative for
 * any integer input. Pure, so the mapping is identical across pages/frames.
 */
export function laneColorIndex(lane: number, paletteSize = 3): number {
  const n = Math.max(1, Math.floor(paletteSize));
  const laneInt = Math.floor(lane);
  return ((laneInt % n) + n) % n;
}

/** Resolve the concrete color for a lane index. */
export function laneColor(lane: number, palette: readonly string[]): string {
  const color = palette[laneColorIndex(lane, palette.length)];
  return color ?? palette[0] ?? "#888888";
}
