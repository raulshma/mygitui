/**
 * Terminal session store (Svelte 5 runes) — one PTY session per repository.
 *
 * Lifecycle:
 * - `ensure(repoId)` lazily creates (via the injected factory) the terminal
 *   UI instance + the backend pty on the first panel mount; later calls
 *   return the same live session (hide/show of the panel does NOT restart
 *   the shell — the xterm element is simply re-parented by the panel).
 * - Input: `term.onData` → `ptyWrite`. Output: `pty-output` events are
 *   dispatched by session id → `term.write`. Exit: `pty-exit` → toast +
 *   cleanup; no auto-recreate (the user restarts from the panel).
 * - `dispose(repoId)` (RepoView unmount = repo tab closed) kills the pty
 *   and drops the session. `restart` = kill + fresh create. `kill` = user
 *   stop button.
 * - Resize: the panel fits the terminal, then calls `resize` — debounced
 *   100 ms before reaching `pty_resize`.
 *
 * The xterm.js classes never appear here: the store only knows the
 * `TerminalLike` / `TerminalHandle` interfaces, and the real factory is
 * registered by `TerminalPanel.svelte` (`setFactory`). Tests inject fakes.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client` (mocked with
 * `vi.mock` in tests).
 */

import {
  onPtyExit,
  onPtyOutput,
  ptyCreate,
  ptyKill,
  ptyResize,
  ptyWrite,
  type PtyExitEvent,
} from "$lib/ipc/client";
import { isTauri } from "$lib/entry/dragdrop";
import { toast } from "$lib/toast";

/** Debounce window between a panel fit and the `pty_resize` call. */
export const RESIZE_DEBOUNCE_MS = 100;

/** Lifecycle of a repo's terminal (drives the panel's UI states). */
export type TerminalStatus =
  | /** pty_create in flight; the terminal exists but has no pty yet. */
  "starting"
  /** Session attached: input/output flow. */
  | "live"
  /** The process exited (or the user killed it); restart to get a new one. */
  | "exited"
  /** pty_create (or the factory) failed; `error` carries the detail. */
  | "failed"
  /** No desktop app / no factory: terminal not available here. */
  | "unavailable";

/**
 * Minimal terminal surface the store drives. Structural superset checks are
 * done at the factory boundary — the real `@xterm/xterm` `Terminal` satisfies
 * this as-is.
 */
export interface TerminalLike {
  /** The element xterm created in `open()` (null while never opened). */
  readonly element: HTMLElement | undefined | null;
  readonly rows: number;
  readonly cols: number;
  open(host: HTMLElement): void;
  write(data: string): void;
  /** Keystroke stream; the returned disposer must unsubscribe. */
  onData(handler: (data: string) => void): { dispose(): void };
  focus(): void;
  dispose(): void;
}

/** xterm theme colors (a `@xterm/xterm` `ITheme`-compatible plain object). */
export type TerminalTheme = Record<string, string | undefined>;

/** What a terminal factory produces: the UI plus its fit/theme hooks. */
export interface TerminalHandle {
  term: TerminalLike;
  /**
   * Fits the terminal to its container (fit addon). Resolves with the new
   * dimensions, or `null` when the terminal is not measurable (hidden,
   * detached or zero-sized — nothing should be sent to `pty_resize` then).
   */
  fit(): { cols: number; rows: number } | null;
  /** Re-applies a theme (scheme changes). */
  setTheme(theme: TerminalTheme): void;
}

/** Creates one terminal UI instance; injectable for tests. */
export type TerminalFactory = () => TerminalHandle;

/** One repo's terminal: the pty identity plus the UI it is attached to. */
export interface TerminalSession {
  readonly repoId: string;
  /** Backend pty id; `null` until `pty_create` resolves (or after exit). */
  sessionId: string | null;
  status: TerminalStatus;
  /** The terminal UI (null while starting-failed / after exit). */
  term: TerminalLike | null;
  fit: (() => { cols: number; rows: number } | null) | null;
  setTheme: ((theme: TerminalTheme) => void) | null;
  /** Failure detail for status `"failed"`. */
  error: string | null;
}

function unavailableSession(repoId: string, error: string | null): TerminalSession {
  return {
    repoId,
    sessionId: null,
    status: "unavailable",
    term: null,
    fit: null,
    setTheme: null,
    error,
  };
}

export class TerminalStore {
  /** Per-repo sessions (reactive record keyed by repoId). */
  sessions: Record<string, TerminalSession> = $state({});

  #factory: TerminalFactory | null = null;
  #started = false;
  #unlisteners: (() => void)[] = [];
  /** pty session id → repoId (non-reactive dispatch index). */
  readonly #bySession = new Map<string, string>();
  /** repoId → freshness token; async `pty_create` continuations check it. */
  readonly #tokens = new Map<string, object>();
  readonly #resizeTimers = new Map<string, ReturnType<typeof setTimeout>>();

  /** Registers the terminal UI factory (the panel does this once). */
  setFactory(factory: TerminalFactory | null): void {
    this.#factory = factory;
  }

  /** The repo's session record, or `null` when none exists. */
  session(repoId: string): TerminalSession | null {
    return this.sessions[repoId] ?? null;
  }

  /**
   * The repo's terminal session, creating it lazily (terminal UI + pty).
   * Safe to call from a mount `$effect`: the first call creates, later ones
   * return the existing starting/live session unchanged.
   */
  ensure(repoId: string): TerminalSession | null {
    const existing = this.sessions[repoId];
    if (existing && (existing.status === "starting" || existing.status === "live")) {
      return existing;
    }
    return this.#create(repoId);
  }

  /** Kill + fresh session (the panel's Restart button). */
  restart(repoId: string): TerminalSession | null {
    this.#teardown(repoId, { markExited: false, remove: false });
    return this.#create(repoId);
  }

  /** User stop: kills the pty and marks the session exited. */
  kill(repoId: string): void {
    this.#teardown(repoId, { markExited: true, remove: false });
  }

  /**
   * Repo teardown (RepoView unmount = the repo tab was closed): kills the
   * pty and forgets the session entirely. No-op for unknown repos.
   */
  dispose(repoId: string): void {
    this.#teardown(repoId, { markExited: false, remove: true });
  }

  /**
   * Size sync (panel ResizeObserver → fit addon → here). Debounced
   * {@link RESIZE_DEBOUNCE_MS}; silently skipped while the repo has no live
   * pty (the panel refits — and lands here again — once it attaches).
   */
  resize(repoId: string, cols: number, rows: number): void {
    if (!this.sessions[repoId]?.sessionId) return;
    const timer = this.#resizeTimers.get(repoId);
    if (timer) clearTimeout(timer);
    this.#resizeTimers.set(
      repoId,
      setTimeout(() => {
        this.#resizeTimers.delete(repoId);
        const sessionId = this.sessions[repoId]?.sessionId;
        if (!sessionId) return;
        void ptyResize(sessionId, rows, cols).catch(() => {});
      }, RESIZE_DEBOUNCE_MS),
    );
  }

  /** Unsubscribes the pty events (test/teardown aid). */
  stop(): void {
    for (const unlisten of this.#unlisteners) {
      try {
        unlisten();
      } catch {
        // Ignore.
      }
    }
    this.#unlisteners = [];
    this.#started = false;
  }

  // -- internals -------------------------------------------------------------

  #create(repoId: string): TerminalSession | null {
    if (!isTauri()) {
      this.sessions[repoId] = unavailableSession(repoId, null);
      return this.sessions[repoId];
    }
    if (!this.#factory) {
      this.sessions[repoId] = unavailableSession(repoId, "no terminal factory");
      return this.sessions[repoId];
    }
    this.#startEvents();

    let handle: TerminalHandle;
    try {
      handle = this.#factory();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      this.sessions[repoId] = {
        repoId,
        sessionId: null,
        status: "failed",
        term: null,
        fit: null,
        setTheme: null,
        error: message,
      };
      return this.sessions[repoId];
    }

    this.sessions[repoId] = {
      repoId,
      sessionId: null,
      status: "starting",
      term: handle.term,
      fit: handle.fit,
      setTheme: handle.setTheme,
      error: null,
    };
    const entry = this.sessions[repoId];
    const token = {};
    this.#tokens.set(repoId, token);

    // Keystrokes: drop silently until the pty id arrives (first ms only).
    entry.term?.onData((data) => {
      const sessionId = this.sessions[repoId]?.sessionId;
      if (!sessionId) return;
      void ptyWrite(sessionId, data).catch(() => {});
    });

    ptyCreate(repoId, handle.term.rows, handle.term.cols)
      .then((sessionId) => {
        if (this.#tokens.get(repoId) !== token) {
          // Session was torn down / replaced meanwhile: kill the orphan.
          void ptyKill(sessionId).catch(() => {});
          return;
        }
        const current = this.sessions[repoId];
        if (!current || current.status !== "starting") {
          void ptyKill(sessionId).catch(() => {});
          return;
        }
        current.sessionId = sessionId;
        current.status = "live";
        this.#bySession.set(sessionId, repoId);
      })
      .catch((err) => {
        if (this.#tokens.get(repoId) !== token) return;
        this.#tokens.delete(repoId);
        const message = err instanceof Error ? err.message : String(err);
        const current = this.sessions[repoId];
        if (current) {
          current.term?.dispose();
          current.term = null;
          current.fit = null;
          current.setTheme = null;
          current.status = "failed";
          current.error = message;
        }
        toast(`Terminal failed to start: ${message}`, { kind: "error" });
      });

    return entry;
  }

  #teardown(
    repoId: string,
    opts: { markExited: boolean; remove: boolean },
  ): void {
    this.#tokens.delete(repoId);
    this.#cancelResize(repoId);
    const entry = this.sessions[repoId];
    if (!entry) return;
    if (entry.sessionId) {
      this.#bySession.delete(entry.sessionId);
      void ptyKill(entry.sessionId).catch(() => {});
    }
    entry.term?.dispose();
    if (opts.remove) {
      delete this.sessions[repoId];
      return;
    }
    entry.sessionId = null;
    entry.term = null;
    entry.fit = null;
    entry.setTheme = null;
    if (opts.markExited) entry.status = "exited";
  }

  #startEvents(): void {
    if (this.#started) return;
    this.#started = true;
    onPtyOutput((event) => this.#applyOutput(event.session_id, event.data))
      .then((unlisten) => this.#unlisteners.push(unlisten))
      .catch((err: unknown) =>
        console.error("[terminal] pty-output subscription failed:", err),
      );
    onPtyExit((event: PtyExitEvent) =>
      this.#applyExit(event.session_id, event.exit_code),
    )
      .then((unlisten) => this.#unlisteners.push(unlisten))
      .catch((err: unknown) =>
        console.error("[terminal] pty-exit subscription failed:", err),
      );
  }

  #applyOutput(sessionId: string, data: string): void {
    const repoId = this.#bySession.get(sessionId);
    if (repoId === undefined) return;
    this.sessions[repoId]?.term?.write(data);
  }

  #applyExit(sessionId: string, exitCode: number | null): void {
    const repoId = this.#bySession.get(sessionId);
    if (repoId === undefined) return;
    this.#bySession.delete(sessionId);
    this.#tokens.delete(repoId);
    this.#cancelResize(repoId);
    const entry = this.sessions[repoId];
    if (entry) {
      entry.sessionId = null;
      entry.status = "exited";
      entry.term?.dispose();
      entry.term = null;
      entry.fit = null;
      entry.setTheme = null;
    }
    toast(
      exitCode === null ? "Terminal exited" : `Terminal exited (code ${exitCode})`,
    );
  }

  #cancelResize(repoId: string): void {
    const timer = this.#resizeTimers.get(repoId);
    if (timer) clearTimeout(timer);
    this.#resizeTimers.delete(repoId);
  }
}

/** The application-wide terminal store. */
export const terminals = new TerminalStore();

// ---------------------------------------------------------------------------
// Theming: map the M3 CSS custom properties (see $lib/theme) onto xterm's
// ITheme. Pure functions over the document so tests / SSR fall back safely.
// ---------------------------------------------------------------------------

interface Rgb {
  r: number;
  g: number;
  b: number;
}

/** Reads an M3 token from `<html>`; `fallback` when unset / no document. */
function cssVar(name: string, fallback: string): string {
  try {
    if (typeof document === "undefined") return fallback;
    const value = getComputedStyle(document.documentElement)
      .getPropertyValue(name)
      .trim();
    return value !== "" ? value : fallback;
  } catch {
    return fallback;
  }
}

/** Parses `#rgb`, `#rrggbb`, `#aarrggbb` and `rgb()/.rgba()` into channels. */
function parseColor(raw: string): Rgb | null {
  const value = raw.trim();
  if (value.startsWith("#")) {
    const hex = value.slice(1);
    const tuple = (part: string): number => parseInt(part, 16);
    if (/^[0-9a-f]{3}$/i.test(hex)) {
      return {
        r: tuple(hex[0]! + hex[0]!),
        g: tuple(hex[1]! + hex[1]!),
        b: tuple(hex[2]! + hex[2]!),
      };
    }
    if (/^[0-9a-f]{6}$/i.test(hex)) {
      return {
        r: tuple(hex.slice(0, 2)),
        g: tuple(hex.slice(2, 4)),
        b: tuple(hex.slice(4, 6)),
      };
    }
    if (/^[0-9a-f]{8}$/i.test(hex)) {
      // #AARRGGBB (material-color-utilities shape) — alpha is dropped here.
      return {
        r: tuple(hex.slice(2, 4)),
        g: tuple(hex.slice(4, 6)),
        b: tuple(hex.slice(6, 8)),
      };
    }
    return null;
  }
  const fn = value.match(/^rgba?\(([^)]*)\)$/i);
  if (fn) {
    const parts = fn[1]!.split(/[,\s/]+/).filter((p) => p !== "");
    if (parts.length >= 3) {
      const r = Number(parts[0]);
      const g = Number(parts[1]);
      const b = Number(parts[2]);
      if ([r, g, b].every((n) => Number.isFinite(n))) {
        return { r, g, b };
      }
    }
  }
  return null;
}

function toHex({ r, g, b }: Rgb): string {
  const channel = (n: number): string =>
    Math.round(Math.min(255, Math.max(0, n))).toString(16).padStart(2, "0");
  return `#${channel(r)}${channel(g)}${channel(b)}`;
}

/**
 * Color mix of `over` at fraction `amount` (0 = base, 1 = over), returned
 * as an opaque hex — the precomputed equivalent of
 * `color-mix(in srgb, <primary> N%, <surface>)`.
 */
function mixColors(rawBase: string, rawOver: string, amount: number): string {
  const base = parseColor(rawBase);
  const over = parseColor(rawOver);
  if (!base) return rawOver;
  if (!over) return rawBase;
  return toHex({
    r: base.r + (over.r - base.r) * amount,
    g: base.g + (over.g - base.g) * amount,
    b: base.b + (over.b - base.b) * amount,
  });
}

/**
 * Static ANSI-16 mapping onto M3 tokens (M3 has no dedicated terminal
 * palette; this is the documented best-effort map — chromatic roles from
 * primary/secondary/tertiary/error, the bright row from their container /
 * fixed variants). Each entry: [xterm key, CSS var, fallback hex].
 */
const ANSI16: readonly (readonly [string, string, string])[] = [
  ["black", "--m3-surface-variant", "#49454f"],
  ["red", "--m3-error", "#f2b8b5"],
  ["green", "--m3-tertiary", "#efb8c8"],
  ["yellow", "--m3-secondary", "#ccc2dc"],
  ["blue", "--m3-primary", "#d0bcff"],
  ["magenta", "--m3-inverse-primary", "#6750a4"],
  ["cyan", "--m3-secondary-fixed", "#e8def8"],
  ["white", "--m3-outline", "#938f99"],
  ["brightBlack", "--m3-outline-variant", "#4a4458"],
  ["brightRed", "--m3-on-error-container", "#f9dedc"],
  ["brightGreen", "--m3-on-tertiary-container", "#31111d"],
  ["brightYellow", "--m3-on-secondary-container", "#1d192b"],
  ["brightBlue", "--m3-on-primary-container", "#4f378b"],
  ["brightMagenta", "--m3-primary-fixed", "#eaddff"],
  ["brightCyan", "--m3-tertiary-fixed", "#ffd8e4"],
  ["brightWhite", "--m3-on-surface", "#e6e0e9"],
];

/** Builds an xterm theme from the current M3 scheme (dark/light aware). */
export function terminalTheme(): TerminalTheme {
  const background = cssVar("--m3-surface", "#141218");
  const foreground = cssVar("--m3-on-surface", "#e6e0e9");
  const primary = cssVar("--m3-primary", "#d0bcff");
  const theme: TerminalTheme = {
    background,
    foreground,
    cursor: primary,
    cursorAccent: cssVar("--m3-on-primary", "#381e72"),
    // Precomputed color-mix(in srgb, primary N%, surface) so xterm never
    // has to parse modern CSS color syntax.
    selectionBackground: mixColors(background, primary, 0.35),
    selectionInactiveBackground: mixColors(background, primary, 0.15),
  };
  for (const [key, token, fallback] of ANSI16) {
    theme[key] = cssVar(token, fallback);
  }
  return theme;
}

/**
 * Subscribes to OS light/dark scheme changes; returns an unlisten function
 * (a no-op where `matchMedia` is unavailable, e.g. jsdom). Never throws.
 */
export function onSchemeChange(cb: () => void): () => void {
  try {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return () => {};
    }
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (): void => cb();
    if (typeof query.addEventListener === "function") {
      query.addEventListener("change", onChange);
      return () => query.removeEventListener("change", onChange);
    }
    if (typeof query.addListener === "function") {
      query.addListener(onChange);
      return () => query.removeListener(onChange);
    }
    return () => {};
  } catch {
    return () => {};
  }
}
