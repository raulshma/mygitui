/**
 * Theme foundation for mygitui (shadcn-svelte).
 *
 * `initTheme()` is called once by App.svelte at startup. It:
 *   1. reads the persisted preference from localStorage "mygitui.theme"
 *      ("system" | "light" | "dark", default "system"),
 *   2. resolves the scheme (override wins, otherwise `prefers-color-scheme`),
 *   3. toggles the `.dark` class on <html> — shadcn/Tailwind dark variants,
 *      the oklch token sets in src/app.css and the `--app-*` dark hues all
 *      key off it — also stamps `data-theme="light|dark"` (legacy consumers,
 *      e2e) and `color-scheme`, and announces the resolved scheme on
 *      {@link THEME_SCHEME_EVENT},
 *   4. follows `prefers-color-scheme` reactively via matchMedia unless a
 *      manual override is active, in which case the OS switch is ignored
 *      until the preference returns to "system".
 *
 * The manual theme preference persists to localStorage "mygitui.theme".
 */
export type ColorScheme = "light" | "dark";

/** What the user asked the theme to do (persisted choice). */
export type ThemePreference = "system" | "light" | "dark";

/** localStorage key holding the persisted {@link ThemePreference}. */
export const THEME_STORAGE_KEY = "mygitui.theme";

/** Window event dispatched whenever the preference changes (detail = value). */
export const THEME_PREFERENCE_EVENT = "mygitui:theme-preference";

/**
 * Window event dispatched whenever the applied (resolved) scheme changes,
 * including OS-driven switches. Detail is the {@link ColorScheme}. Consumers
 * that bake colors at runtime (Shiki, the graph palette) follow this instead
 * of polling {@link currentScheme}.
 */
export const THEME_SCHEME_EVENT = "mygitui:theme-scheme";

const DARK_QUERY = "(prefers-color-scheme: dark)";

// ---------------------------------------------------------------------------
// Preference persistence (never throws; corrupt values fall back to system)
// ---------------------------------------------------------------------------

/** In-memory mirror of the persisted preference ("system" until loaded/set). */
let preference: ThemePreference = "system";

/** Reads the persisted preference, silently defaulting to "system". */
function loadPreference(): ThemePreference {
  try {
    if (typeof localStorage === "undefined") return "system";
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    return raw === "light" || raw === "dark" ? raw : "system";
  } catch {
    return "system";
  }
}

/** Persists the preference (storage failures are swallowed). */
function savePreference(value: ThemePreference): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(THEME_STORAGE_KEY, value);
  } catch {
    // Storage may be full or unavailable; the choice stays in memory.
  }
}

/**
 * Announces the current preference on `window` so theme controls elsewhere
 * (tab strip, home header, palette) stay in sync. Best-effort.
 */
function notifyPreference(): void {
  try {
    if (typeof window === "undefined") return;
    window.dispatchEvent(
      new CustomEvent<ThemePreference>(THEME_PREFERENCE_EVENT, {
        detail: preference,
      }),
    );
  } catch {
    // CustomEvent/window may be unavailable in tests; the state is set.
  }
}

/**
 * Sets the theme preference and applies it immediately (when the theme is
 * initialized). "system" hands control back to `prefers-color-scheme`
 * (followed live); "light" / "dark" pin the scheme until reset. The choice
 * persists to localStorage. Never throws.
 */
export function setThemePreference(value: ThemePreference): void {
  try {
    if (value !== "system" && value !== "light" && value !== "dark") return;
    preference = value;
    savePreference(value);
    if (watching) applyScheme(resolveScheme());
    notifyPreference();
  } catch {
    // A theming failure must never propagate to the caller.
  }
}

/** The current preference ("system" until a manual override is set). */
export function getThemePreference(): ThemePreference {
  return preference;
}

/** The scheme to apply right now: override wins, system follows the OS. */
function resolveScheme(): ColorScheme {
  return preference === "system" ? preferredScheme() : preference;
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

/** Module state; initTheme re-runs cleanly by disposing the previous one. */
let watching = false;
let stopWatching = () => {};

/** Applies a scheme to the document root: `.dark` class + legacy stamps. */
function applyScheme(scheme: ColorScheme): void {
  current = scheme;
  try {
    if (typeof document === "undefined") return;
    const root = document.documentElement;
    root.classList.toggle("dark", scheme === "dark");
    root.dataset.theme = scheme;
    root.style.setProperty("color-scheme", scheme);
  } catch {
    // No DOM (tests/workers): the module state is still updated.
  }
  announceScheme(scheme);
}

/** Announces the applied scheme so runtime-colored surfaces can re-theme. */
function announceScheme(scheme: ColorScheme): void {
  try {
    if (typeof window === "undefined") return;
    window.dispatchEvent(
      new CustomEvent<ColorScheme>(THEME_SCHEME_EVENT, { detail: scheme }),
    );
  } catch {
    // CustomEvent/window may be unavailable in tests; the state is set.
  }
}

/**
 * Initialize the theme. Safe to call more than once (re-initializes); must
 * never throw or reject — theming failures fall back silently to light.
 */
export async function initTheme(): Promise<void> {
  try {
    disposeTheme();

    if (typeof document === "undefined") return;

    preference = loadPreference();
    notifyPreference();
    applyScheme(resolveScheme());

    const mm = getMatchMedia();
    if (mm) {
      try {
        const query = mm(DARK_QUERY);
        const onChange = (event: MediaQueryListEvent) => {
          // The OS only drives the scheme while no override is pinned.
          if (preference !== "system") return;
          applyScheme(event.matches ? "dark" : "light");
        };
        if (typeof query.addEventListener === "function") {
          query.addEventListener("change", onChange);
          stopWatching = () => query.removeEventListener("change", onChange);
        } else if (typeof (query as MediaQueryList).addListener === "function") {
          // Legacy Safari < 14.
          query.addListener(onChange as (event: MediaQueryListEvent) => void);
          stopWatching = () =>
            query.removeListener(onChange as (event: MediaQueryListEvent) => void);
        }
      } catch {
        // Watching is best-effort; the applied scheme stays correct.
      }
    }

    watching = true;
  } catch {
    // Swallow everything: a theme failure must never break app startup.
  }
}

/** Stop reacting to OS scheme changes and drop module state. Test helper. */
export function disposeTheme(): void {
  try {
    stopWatching();
  } catch {
    // Ignore.
  }
  watching = false;
  stopWatching = () => {};
}

/** The scheme currently applied ("light" before init). */
let current: ColorScheme = "light";

/** The scheme whose tokens are currently applied ("light" before init). */
export function currentScheme(): ColorScheme {
  return current;
}
