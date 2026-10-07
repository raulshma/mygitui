/**
 * Dynamic color engine for mygitui.
 *
 * Wraps @material/material-color-utilities: a seed color is turned into a
 * light and a dark {@link DynamicScheme} (TonalSpot, the default Material You
 * variant), and each scheme is flattened into a flat token map of hex strings
 * (`primary` -> `#6750a4`, `surfaceContainerLow` -> `#f7f2fa`, ...).
 *
 * Token names are kebab-cased downstream (`--m3-primary-container`), see
 * src/lib/theme/index.ts.
 */
import {
  Hct,
  SchemeTonalSpot,
  argbFromHex,
  hexFromArgb,
} from "@material/material-color-utilities";
import type { DynamicScheme } from "@material/material-color-utilities";

/** Baseline M3 purple seed, used whenever no source color is available. */
export const BASELINE_SEED = "#6750a4";

/** Contrast level: 0 = standard, -1..1 = reduced..increased. */
const CONTRAST_LEVEL = 0.0;

/**
 * The full set of color tokens this theme publishes, in a stable order.
 * Each entry maps a token name (kebab-cased later) to a scheme accessor.
 */
const TOKEN_SPEC: ReadonlyArray<readonly [string, (scheme: DynamicScheme) => number]> = [
  // Primary
  ["primary", (s) => s.primary],
  ["on-primary", (s) => s.onPrimary],
  ["primary-container", (s) => s.primaryContainer],
  ["on-primary-container", (s) => s.onPrimaryContainer],
  ["inverse-primary", (s) => s.inversePrimary],
  ["primary-fixed", (s) => s.primaryFixed],
  ["primary-fixed-dim", (s) => s.primaryFixedDim],
  ["on-primary-fixed", (s) => s.onPrimaryFixed],
  ["on-primary-fixed-variant", (s) => s.onPrimaryFixedVariant],
  // Secondary
  ["secondary", (s) => s.secondary],
  ["on-secondary", (s) => s.onSecondary],
  ["secondary-container", (s) => s.secondaryContainer],
  ["on-secondary-container", (s) => s.onSecondaryContainer],
  ["secondary-fixed", (s) => s.secondaryFixed],
  ["secondary-fixed-dim", (s) => s.secondaryFixedDim],
  ["on-secondary-fixed", (s) => s.onSecondaryFixed],
  ["on-secondary-fixed-variant", (s) => s.onSecondaryFixedVariant],
  // Tertiary
  ["tertiary", (s) => s.tertiary],
  ["on-tertiary", (s) => s.onTertiary],
  ["tertiary-container", (s) => s.tertiaryContainer],
  ["on-tertiary-container", (s) => s.onTertiaryContainer],
  ["tertiary-fixed", (s) => s.tertiaryFixed],
  ["tertiary-fixed-dim", (s) => s.tertiaryFixedDim],
  ["on-tertiary-fixed", (s) => s.onTertiaryFixed],
  ["on-tertiary-fixed-variant", (s) => s.onTertiaryFixedVariant],
  // Error
  ["error", (s) => s.error],
  ["on-error", (s) => s.onError],
  ["error-container", (s) => s.errorContainer],
  ["on-error-container", (s) => s.onErrorContainer],
  // Neutral / surfaces
  ["background", (s) => s.background],
  ["on-background", (s) => s.onBackground],
  ["surface", (s) => s.surface],
  ["on-surface", (s) => s.onSurface],
  ["surface-variant", (s) => s.surfaceVariant],
  ["on-surface-variant", (s) => s.onSurfaceVariant],
  ["surface-dim", (s) => s.surfaceDim],
  ["surface-bright", (s) => s.surfaceBright],
  ["surface-container-lowest", (s) => s.surfaceContainerLowest],
  ["surface-container-low", (s) => s.surfaceContainerLow],
  ["surface-container", (s) => s.surfaceContainer],
  ["surface-container-high", (s) => s.surfaceContainerHigh],
  ["surface-container-highest", (s) => s.surfaceContainerHighest],
  ["surface-tint", (s) => s.surfaceTint],
  ["inverse-surface", (s) => s.inverseSurface],
  ["inverse-on-surface", (s) => s.inverseOnSurface],
  ["outline", (s) => s.outline],
  ["outline-variant", (s) => s.outlineVariant],
  ["shadow", (s) => s.shadow],
  ["scrim", (s) => s.scrim],
];

/** All published token names (without the `--m3-` prefix), in spec order. */
export const TOKEN_NAMES: readonly string[] = TOKEN_SPEC.map(([name]) => name);

/** Flatten a scheme into `{ "token-name": "#rrggbb" }`. */
export function tokensForScheme(scheme: DynamicScheme): Readonly<Record<string, string>> {
  const tokens: Record<string, string> = {};
  for (const [name, get] of TOKEN_SPEC) {
    tokens[name] = hexFromArgb(get(scheme));
  }
  return tokens;
}

/** Build a light or dark TonalSpot scheme from a hex seed color. */
export function schemeFromSeed(seed: string, dark: boolean): DynamicScheme {
  return new SchemeTonalSpot(Hct.fromInt(argbFromHex(seed)), dark, CONTRAST_LEVEL);
}

/**
 * Best-effort dynamic seed color resolution.
 *
 * Verification (m3-svelte 7.2.1): its public surface — the root module
 * (components only) and the `etc/*` exports (CSS generation helpers) — does
 * NOT expose any wallpaper/source-color extraction API. Probing it at runtime
 * is pointless today: importing the root module would additionally drag every
 * component through the bundler (~10s measured in vitest) for a guaranteed
 * null, and `m3-svelte/etc/colors` needs the uninstalled optional peer
 * `@ktibow/material-color-utilities-nightly` (an unresolvable import breaks
 * `vite build` even inside try/catch).
 *
 * This function is the single seam for real dynamic color later (Tauri
 * wallpaper plugin, OS accent color, ...): return a hex string from here and
 * the whole token pipeline picks it up. Until then callers keep the baseline
 * palette. Never throws, never rejects.
 */
export async function resolveSeedColor(): Promise<string | null> {
  return null;
}
