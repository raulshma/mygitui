/**
 * Terminal theme preference + fixed light/dark palettes.
 *
 * The terminal can follow the app theme ("system", the default — the live
 * CSS-var derivation in `terminalStore.svelte.ts`'s `terminalTheme()`) or
 * pin an independent light/dark scheme. The preference persists to
 * localStorage `mygitui.terminal-theme` like the app theme does
 * (`$lib/theme`, key `mygitui.theme`); it is deliberately independent of
 * that preference.
 *
 * The fixed palettes freeze the same color roles the live derivation maps
 * (background/foreground/cursor from the scheme tokens, selection =
 * precomputed `mixColors` blends, ANSI-16 per the `ANSI16` role table), but
 * hardcode per-mode values so xterm never depends on live CSS state:
 * - scheme-token slots use the app's default neutral token values converted
 *   oklch → sRGB hex (background `#171717`/`#ffffff`, foreground
 *   `#fafafa`/`#0a0a0a`, primary cursor, …);
 * - chromatic slots reuse the repo's static per-scheme git colors
 *   (`--app-added`/`--app-modified`/`--app-renamed`/`--destructive`) and,
 *   for the hues the repo doesn't define (blue, cyan, grays), the GitHub
 *   palette the `--app-*` values are derived from — so both palettes stay
 *   legible on their background (the live role table maps some slots to
 *   near-background neutrals, which would be invisible when frozen).
 *
 * Re-theming: `applyTerminalThemeToAllSessions` re-applies the active theme
 * to every live session (sessions outlive panels). It runs when the
 * preference changes (the store) and when the app's resolved scheme changes
 * (`THEME_SCHEME_EVENT`, subscribed once via `startTerminalThemeSync` —
 * this also fixes the old gap where a manual app-theme toggle re-themed
 * nothing because `matchMedia` never fired).
 */

import { terminals, terminalTheme, type TerminalTheme } from "./terminalStore.svelte";
import { THEME_SCHEME_EVENT } from "$lib/theme";

/** What the terminal should do about its colors (persisted choice). */
export type TerminalThemePreference = "system" | "light" | "dark";

/** localStorage key holding the persisted {@link TerminalThemePreference}. */
export const TERMINAL_THEME_STORAGE_KEY = "mygitui.terminal-theme";

/** The cycle order of the panel's theme toggle (system → light → dark). */
const ORDER: readonly TerminalThemePreference[] = ["system", "light", "dark"];

/** Header title per preference ("click for X" names the next state). */
export const TERMINAL_THEME_TITLES: Record<TerminalThemePreference, string> = {
  system: "Terminal theme: match app (click for light)",
  light: "Terminal theme: light (click for dark)",
  dark: "Terminal theme: dark (click for match app)",
};

/**
 * Fixed dark palette. Background/foreground/cursor/cursorAccent are the
 * app's dark neutral tokens (oklch → hex); the selection blends are
 * `mixColors` results computed once; ANSI-16 mixes repo git colors
 * (`--app-*`, `--destructive`) with GitHub dark for the undefined hues.
 */
export const TERMINAL_THEME_DARK: TerminalTheme = {
  background: "#171717", // dark --card (oklch(0.205 0 0))
  foreground: "#fafafa", // dark --foreground (oklch(0.985 0 0))
  cursor: "#e5e5e5", // dark --primary (oklch(0.922 0 0))
  cursorAccent: "#171717", // dark --primary-foreground
  // Precomputed color-mix(in srgb, primary N%, background).
  selectionBackground: "#5f5f5f",
  selectionInactiveBackground: "#363636",
  // ANSI-16: repo git colors + GitHub dark (see module doc).
  black: "#484f58",
  red: "#ff6467", // dark --destructive (oklch(0.704 0.191 22.216))
  green: "#3fb950", // dark --app-added
  yellow: "#d29922", // dark --app-modified
  blue: "#58a6ff",
  magenta: "#a371f7", // dark --app-renamed
  cyan: "#39c5cf",
  white: "#b1bac4",
  brightBlack: "#6e7681",
  brightRed: "#ffa198",
  brightGreen: "#7ee787",
  brightYellow: "#f2cc60",
  brightBlue: "#79c0ff",
  brightMagenta: "#d2a8ff",
  brightCyan: "#a5d6ff",
  brightWhite: "#fafafa", // dark --foreground
};

/**
 * Fixed light palette — same role mapping against the app's light neutral
 * tokens and the GitHub light counterparts of the `--app-*` values.
 */
export const TERMINAL_THEME_LIGHT: TerminalTheme = {
  background: "#ffffff", // light --card (oklch(1 0 0))
  foreground: "#0a0a0a", // light --foreground (oklch(0.145 0 0))
  cursor: "#171717", // light --primary (oklch(0.205 0 0))
  cursorAccent: "#fafafa", // light --primary-foreground (oklch(0.985 0 0))
  // Precomputed color-mix(in srgb, primary N%, background).
  selectionBackground: "#aeaeae",
  selectionInactiveBackground: "#dcdcdc",
  // ANSI-16: repo git colors + GitHub light (see module doc).
  black: "#24292f",
  red: "#e7000b", // light --destructive (oklch(0.577 0.245 27.325))
  green: "#116329",
  yellow: "#bf8700",
  blue: "#0969da",
  magenta: "#8250df", // light --app-renamed
  cyan: "#1b7c83",
  white: "#6e7781",
  brightBlack: "#57606a",
  brightRed: "#a40e26",
  brightGreen: "#1a7f37", // light --app-added
  brightYellow: "#9a6700", // light --app-modified
  brightBlue: "#218bff",
  brightMagenta: "#a475f9",
  brightCyan: "#3192aa",
  brightWhite: "#0a0a0a", // light --foreground
};

/** Reads the persisted preference, silently defaulting to "system". */
function loadPreference(): TerminalThemePreference {
  try {
    if (typeof localStorage === "undefined") return "system";
    const raw = localStorage.getItem(TERMINAL_THEME_STORAGE_KEY);
    return raw === "light" || raw === "dark" ? raw : "system";
  } catch {
    return "system";
  }
}

/** Persists the preference (storage failures are swallowed). */
function savePreference(value: TerminalThemePreference): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(TERMINAL_THEME_STORAGE_KEY, value);
  } catch {
    // Storage may be full or unavailable; the choice stays in memory.
  }
}

/** Singleton rune store (same pattern as the terminal store). */
class TerminalThemePrefStore {
  /** The user's terminal theme preference ("system" = follow the app). */
  preference: TerminalThemePreference = $state(loadPreference());

  /**
   * Sets the preference, persists it and re-themes every live session
   * (immediately — sessions outlive panels, so the store owns this
   * trigger). Unknown values are ignored. Never throws.
   */
  set(value: TerminalThemePreference): void {
    if (!ORDER.includes(value)) return;
    if (value === this.preference) return;
    this.preference = value;
    savePreference(value);
    applyTerminalThemeToAllSessions();
  }

  /** system → light → dark → system (the panel header's toggle). */
  cycle(): TerminalThemePreference {
    const index = ORDER.indexOf(this.preference);
    const next = ORDER[(index + 1) % ORDER.length] ?? "system";
    this.set(next);
    return next;
  }
}

/** The application-wide terminal theme preference store. */
export const terminalThemePref = new TerminalThemePrefStore();

/**
 * The theme the terminals should show right now: the fixed palette for a
 * pinned preference, the live CSS-var derivation while "system" (that
 * behavior is unchanged).
 */
export function activeTerminalTheme(): TerminalTheme {
  if (terminalThemePref.preference === "light") return TERMINAL_THEME_LIGHT;
  if (terminalThemePref.preference === "dark") return TERMINAL_THEME_DARK;
  return terminalTheme();
}

/**
 * Re-applies the active theme to EVERY live session (the store's records
 * may outlive their panels). No-op where a session has no theme handle
 * (exited / failed / unavailable). Never throws.
 */
export function applyTerminalThemeToAllSessions(): void {
  try {
    const theme = activeTerminalTheme();
    for (const session of Object.values(terminals.sessions)) {
      session.setTheme?.(theme);
    }
  } catch {
    // Re-theming must never propagate to the trigger.
  }
}

let synced = false;

/**
 * Subscribes (once) to the app theme's resolved-scheme event so "system"
 * terminals follow MANUAL app-theme switches too, not just OS scheme
 * changes (the event fires on every resolved-scheme change; the old
 * matchMedia-only listener missed the manual ones). Idempotent; returns a
 * disposer for tests. Never throws.
 */
export function startTerminalThemeSync(): () => void {
  if (synced) return () => {};
  synced = true;
  try {
    if (typeof window === "undefined") return () => {};
    window.addEventListener(THEME_SCHEME_EVENT, applyTerminalThemeToAllSessions);
    return () => {
      try {
        window.removeEventListener(THEME_SCHEME_EVENT, applyTerminalThemeToAllSessions);
      } catch {
        // Nothing to unwind.
      }
      synced = false;
    };
  } catch {
    return () => {};
  }
}
