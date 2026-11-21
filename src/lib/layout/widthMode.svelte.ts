/**
 * Reactive window width modes (plan §4.2) — the shell's responsive ladder.
 *
 * The window width resolves to one of four modes, mirrored onto
 * `<html data-width-mode="…">` so CSS can key off it:
 *
 * - **Wide** ≥1280 — full docked layout,
 * - **Medium** 1024–1279 — first collapse tier (§10 matrix),
 * - **Narrow** 800–1023 — drawer tier (800 = the Tauri min window width),
 * - **Stacked** <800 — best-effort fallback below the supported floor.
 *
 * Components style against the mode with the `wm-wide` / `wm-medium` /
 * `wm-narrow` / `wm-stacked` custom variants (see `src/app.css`) instead of
 * arbitrary `max-[…px]` breakpoints; panel-INTERNAL adaptation uses CSS
 * container queries instead (`@container` + `@[…px]:` — the convention is
 * documented in `app.css`). Open/closed overlay state keyed by mode
 * persists through `splitPrefs.ts`.
 *
 * The browser surface is injectable (`WidthModeEnv`) so tests run without
 * a real window: nothing touches `window`/`document` at module import —
 * `App.svelte` calls {@link startWidthMode} from its `onMount` (popouts
 * included — each webview reflects its own width), and {@link stopWidthMode}
 * unsubscribes on teardown. Resize handling is change-guarded rather than
 * debounced: a resize tick costs a few comparisons and (only when the mode
 * actually flips) one attribute write + one `$state` update.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 */

/** The mode ladder, roomiest first (also the persistence iteration order). */
export const WIDTH_MODES = ["wide", "medium", "narrow", "stacked"] as const;

/** One rung of the responsive ladder (see the module doc). */
export type WidthMode = (typeof WIDTH_MODES)[number];

/** Width floors (px): a mode applies from its floor up to the next one. */
export const WIDTH_WIDE_MIN = 1280;
export const WIDTH_MEDIUM_MIN = 1024;
/** The responsive floor — matches the Tauri min window width (§4.2). */
export const WIDTH_NARROW_MIN = 800;

/** `<html>` attribute the resolved mode is mirrored to (`wm-*` variants). */
export const WIDTH_MODE_ATTR = "data-width-mode";

/**
 * Pure threshold resolution. Non-finite widths fail open to "wide" (a
 * broken read must not hide panels); the store path passes `null` through
 * `readWidth` instead and keeps the current mode.
 */
export function resolveWidthMode(width: number): WidthMode {
  if (!Number.isFinite(width)) return "wide";
  if (width >= WIDTH_WIDE_MIN) return "wide";
  if (width >= WIDTH_MEDIUM_MIN) return "medium";
  if (width >= WIDTH_NARROW_MIN) return "narrow";
  return "stacked";
}

/** Injectable browser surface (tests inject fakes; production defaults). */
export interface WidthModeEnv {
  /** Current window width (`null` = unknown — keep the current mode). */
  readWidth(): number | null;
  /** Subscribes to resizes; the returned fn unsubscribes. */
  onResize(handler: () => void): () => void;
  /** Mirrors the mode onto the document (see {@link WIDTH_MODE_ATTR}). */
  reflect(mode: WidthMode): void;
}

/** Production env: `window.innerWidth` + the `resize` event + `<html>`. */
function defaultEnv(): WidthModeEnv {
  return {
    readWidth: () => (typeof window === "undefined" ? null : window.innerWidth),
    onResize: (handler) => {
      window.addEventListener("resize", handler);
      return () => window.removeEventListener("resize", handler);
    },
    reflect: (mode) => {
      try {
        document.documentElement.dataset.widthMode = mode;
      } catch {
        // No document to reflect onto (defensive; tests inject `reflect`).
      }
    },
  };
}

/** Reactive mirror of the resolved mode (reads are runes-reactive). */
const modeState = $state<{ current: WidthMode }>({ current: "wide" });

/** The current width mode. "wide" until {@link startWidthMode} resolves. */
export const widthMode = {
  get current(): WidthMode {
    return modeState.current;
  },
};

let unsubscribe: (() => void) | null = null;

/**
 * Resolves + reflects the width once, then keeps it updated on resize.
 * Idempotent (a running instance is stopped first); returns the stop fn.
 */
export function startWidthMode(inject?: Partial<WidthModeEnv>): () => void {
  stopWidthMode();
  const env: WidthModeEnv = { ...defaultEnv(), ...inject };
  // null until the first successful resolve, so a restart re-reflects.
  let applied: WidthMode | null = null;
  const apply = (): void => {
    const width = env.readWidth();
    if (width === null) return;
    const mode = resolveWidthMode(width);
    if (mode === applied) return;
    applied = mode;
    modeState.current = mode;
    env.reflect(mode);
  };
  apply();
  unsubscribe = env.onResize(apply);
  return stopWidthMode;
}

/** Unsubscribes from resizes (the mode and its attribute stay as-is). */
export function stopWidthMode(): void {
  if (unsubscribe) {
    unsubscribe();
    unsubscribe = null;
  }
}
