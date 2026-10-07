/**
 * Theme foundation for mygitui.
 *
 * `initTheme()` is called once by App.svelte at startup. It:
 *   1. resolves a seed color (baseline palette today; dynamic color is a
 *      best-effort probe that silently no-ops, see dynamic-color.ts),
 *   2. derives light + dark M3 schemes from the seed,
 *   3. writes every token as `--m3-<name>` (plus `--m3c-<name>` aliases so
 *      wrapped m3-svelte components pick the exact same palette) onto
 *      document.documentElement, and
 *   4. follows `prefers-color-scheme` reactively via matchMedia — the token
 *      set is swapped in place when the OS switches, no reload.
 *
 * Published CSS variables (light/dark pairs, hex values):
 *   --m3-primary, --m3-on-primary, --m3-primary-container,
 *   --m3-on-primary-container, --m3-inverse-primary, --m3-primary-fixed,
 *   --m3-primary-fixed-dim, --m3-on-primary-fixed,
 *   --m3-on-primary-fixed-variant, --m3-secondary, --m3-on-secondary,
 *   --m3-secondary-container, --m3-on-secondary-container,
 *   --m3-secondary-fixed, --m3-secondary-fixed-dim, --m3-on-secondary-fixed,
 *   --m3-on-secondary-fixed-variant, --m3-tertiary, --m3-on-tertiary,
 *   --m3-tertiary-container, --m3-on-tertiary-container, --m3-tertiary-fixed,
 *   --m3-tertiary-fixed-dim, --m3-on-tertiary-fixed,
 *   --m3-on-tertiary-fixed-variant, --m3-error, --m3-on-error,
 *   --m3-error-container, --m3-on-error-container, --m3-background,
 *   --m3-on-background, --m3-surface, --m3-on-surface, --m3-surface-variant,
 *   --m3-on-surface-variant, --m3-surface-dim, --m3-surface-bright,
 *   --m3-surface-container-lowest, --m3-surface-container-low,
 *   --m3-surface-container, --m3-surface-container-high,
 *   --m3-surface-container-highest, --m3-surface-tint, --m3-inverse-surface,
 *   --m3-inverse-on-surface, --m3-outline, --m3-outline-variant, --m3-shadow,
 *   --m3-scrim  (each also as --m3c-<name> for m3-svelte compatibility)
 *
 * Static, non-color M3 tokens (--m3-shape-*, --m3-elevation-*) live in
 * src/app.css. Persistence is out of scope for M0: OS-follow only.
 */
export { TOKEN_NAMES } from "./dynamic-color";

import { BASELINE_SEED, resolveSeedColor, schemeFromSeed, tokensForScheme } from "./dynamic-color";

export type ColorScheme = "light" | "dark";

const DARK_QUERY = "(prefers-color-scheme: dark)";

/** Inline styles of <html> — the single write target for runtime tokens. */
function rootStyle(): CSSStyleDeclaration | null {
  try {
    return typeof document !== "undefined" ? document.documentElement.style : null;
  } catch {
    return null;
  }
}

/** Resolve matchMedia without assuming it exists (jsdom, workers, ...). */
function getMatchMedia(): ((query: string) => MediaQueryList) | null {
  try {
    if (typeof matchMedia === "function") return matchMedia;
    if (typeof window !== "undefined" && typeof window.matchMedia === "function") {
      return (query) => window.matchMedia(query);
    }
  } catch {
    // Fall through.
  }
  return null;
}

function preferredScheme(): ColorScheme {
  const mm = getMatchMedia();
  if (!mm) return "light";
  try {
    return mm(DARK_QUERY).matches ? "dark" : "light";
  } catch {
    return "light";
  }
}

interface AppliedTheme {
  light: Readonly<Record<string, string>>;
  dark: Readonly<Record<string, string>>;
  scheme: ColorScheme;
  stopWatching: () => void;
}

/** Module state; initTheme re-runs cleanly by disposing the previous one. */
let applied: AppliedTheme | null = null;

function applyTokens(theme: AppliedTheme, scheme: ColorScheme): void {
  const style = rootStyle();
  if (!style) return;
  const tokens = scheme === "dark" ? theme.dark : theme.light;
  for (const [name, value] of Object.entries(tokens)) {
    style.setProperty(`--m3-${name}`, value);
    style.setProperty(`--m3c-${name}`, value);
  }
  style.setProperty("color-scheme", scheme);
  theme.scheme = scheme;
}

/**
 * Initialize the M3 theme. Safe to call more than once (re-initializes);
 * must never throw or reject — theming failures fall back to the baseline
 * palette silently.
 */
export async function initTheme(): Promise<void> {
  try {
    disposeTheme();

    if (typeof document === "undefined") return;

    const seed = (await resolveSeedColor()) ?? BASELINE_SEED;
    const theme: AppliedTheme = {
      light: tokensForScheme(schemeFromSeed(seed, false)),
      dark: tokensForScheme(schemeFromSeed(seed, true)),
      scheme: preferredScheme(),
      stopWatching: () => {},
    };

    applyTokens(theme, theme.scheme);

    const mm = getMatchMedia();
    if (mm) {
      try {
        const query = mm(DARK_QUERY);
        const onChange = (event: MediaQueryListEvent) => {
          applyTokens(theme, event.matches ? "dark" : "light");
        };
        if (typeof query.addEventListener === "function") {
          query.addEventListener("change", onChange);
          theme.stopWatching = () => query.removeEventListener("change", onChange);
        } else if (typeof (query as MediaQueryList).addListener === "function") {
          // Legacy Safari < 14.
          query.addListener(onChange as (event: MediaQueryListEvent) => void);
          theme.stopWatching = () => query.removeListener(onChange as (event: MediaQueryListEvent) => void);
        }
      } catch {
        // Watching is best-effort; the applied scheme stays correct.
      }
    }

    applied = theme;
  } catch {
    // Swallow everything: a theme failure must never break app startup.
  }
}

/** Stop reacting to OS scheme changes and drop module state. Test helper. */
export function disposeTheme(): void {
  try {
    applied?.stopWatching();
  } catch {
    // Ignore.
  }
  applied = null;
}

/** The scheme whose tokens are currently applied ("light" before init). */
export function currentScheme(): ColorScheme {
  return applied?.scheme ?? "light";
}
