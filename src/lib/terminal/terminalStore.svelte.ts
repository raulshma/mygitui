/**
 * Terminal session store (Svelte 5 runes) — multiple PTY sessions per
 * repository (terminal tabs; splits are out of scope).
 *
 * Identity: a session is keyed by the pair `(repoId, terminalId)`; terminal
 * ids are per-repo `"t1"`, `"t2"`, … minted in creation order and tracked
 * in a per-repo ordered list (`order`) with the focused one in `active`
 * (both session-level state — terminals are never restored across
 * restarts, so nothing here persists).
 *
 * Lifecycle:
 * - `ensure(repoId)` lazily creates (via the injected factory) the ACTIVE
 *   terminal's UI instance + backend pty on the first panel mount; later
 *   calls return the same live session (hide/show of the panel does NOT
 *   restart the shell — the xterm element is simply re-parented by the
 *   panel). Repositories on an SSH device pass `remote` and get a remote
 *   shell (`rshell_create`) instead — identical session ids and events.
 * - `create(repoId)` (the panel's "+" button) adds a NEW terminal id,
 *   activates it and creates its session — capped at
 *   {@link MAX_TERMINALS_PER_REPO} per repo (the backend caps 8 pty
 *   sessions globally; the per-repo cap stays conservative). `close`
 *   removes one tab; `setActive` switches without touching sessions.
 * - Input: `term.onData` → `ptyWrite` / `rshellWrite`; snippets write
 *   whole commands through `writeInput` (command + Enter into the ACTIVE
 *   session's pty). Output: `pty-output` events are dispatched by pty
 *   session id → that session's `term.write`. Exit: `pty-exit` → toast +
 *   cleanup of that one session; no auto-recreate (the user restarts from
 *   the panel).
 * - `dispose(repoId)` (RepoView unmount = repo tab closed) kills and
 *   forgets ALL of the repo's terminals. `restart` = kill + fresh create
 *   for the active terminal. `kill` = user stop button (active terminal).
 * - Resize: the panel fits the visible (active) terminal, then calls
 *   `resize` — debounced 100 ms before reaching `pty_resize` /
 *   `rshell_resize`.
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
  ptyCreate,
  ptyKill,
  ptyResize,
  ptyWrite,
  rshellCreate,
  rshellKill,
  rshellResize,
  rshellWrite,
} from "$lib/ipc/client";
import type { PtyExitEvent } from "$lib/ipc/types";
import { onPtyExit, onPtyOutput } from "$lib/events/repoEvents";
import { isTauri } from "$lib/entry/dragdrop";
import { toast } from "$lib/toast";
import { describeError } from "$lib/errors";
import { appSettings } from "$lib/state/appSettings.svelte";

/** Debounce window between a panel fit and the `pty_resize` call. */
export const RESIZE_DEBOUNCE_MS = 100;

/**
 * xterm font size (px) fallback — used when the `terminalFontSize` app
 * setting is somehow not a finite number (kept alongside the font stack,
 * which stays in TerminalPanel's factory).
 */
export const TERMINAL_FONT_SIZE = 13;

/**
 * The configured xterm font size (px) for the panel's terminal factory.
 * Read fresh at every session (re)creation (`ensure` / `restart` call the
 * factory again), so a Settings change applies to NEW sessions; running
 * sessions are deliberately not live-resized (the pty grid would need a
 * re-fit + `pty_resize` round-trip — restart picks the new size up).
 */
export function terminalFontSize(): number {
  const size = appSettings.terminalFontSize;
  return Number.isFinite(size) ? size : TERMINAL_FONT_SIZE;
}

/**
 * Terminal tabs per repo: the conservative FE cap (the backend allows 8 pty
 * sessions globally — all repos combined — so 4 per repo can never wedge a
 * second tab's shell start on its own).
 */
export const MAX_TERMINALS_PER_REPO = 4;

/**
 * Composite session key: one terminal of one repo. Terminal ids are
 * per-repo (`"t1"` …), so the raw repoId alone is no longer a session key.
 */
export function sessionKey(repoId: string, terminalId: string): string {
  return `${repoId}::${terminalId}`;
}

/** Splits a composite key back; `null` for keys without the separator. */
export function splitSessionKey(
  key: string,
): { repoId: string; terminalId: string } | null {
  const sep = key.lastIndexOf("::");
  if (sep <= 0) return null;
  const repoId = key.slice(0, sep);
  const terminalId = key.slice(sep + 2);
  return repoId !== "" && terminalId !== "" ? { repoId, terminalId } : null;
}

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

/** One repo terminal tab: the pty identity plus the UI it is attached to. */
export interface TerminalSession {
  readonly repoId: string;
  /** Per-repo terminal id (`"t1"` …) — half of the session key. */
  readonly terminalId: string;
  /** Backend pty id; `null` until `pty_create` resolves (or after exit). */
  sessionId: string | null;
  /**
   * True for repositories on an SSH device: the session is a remote shell
   * (`rshell_*` commands) instead of a local pty — same events, same ids.
   */
  remote: boolean;
  status: TerminalStatus;
  /** The terminal UI (null while starting-failed / after exit). */
  term: TerminalLike | null;
  fit: (() => { cols: number; rows: number } | null) | null;
  setTheme: ((theme: TerminalTheme) => void) | null;
  /** Failure detail for status `"failed"`. */
  error: string | null;
}

function unavailableSession(
  repoId: string,
  terminalId: string,
  error: string | null,
): TerminalSession {
  return {
    repoId,
    terminalId,
    sessionId: null,
    remote: false,
    status: "unavailable",
    term: null,
    fit: null,
    setTheme: null,
    error,
  };
}

export class TerminalStore {
  /** All sessions, keyed by the composite {@link sessionKey}. */
  sessions: Record<string, TerminalSession> = $state({});

  /**
   * Per-repo terminal ids in tab order. Repos without terminals are absent
   * (empty read), so the panel can fall back to the first terminal.
   */
  order: Record<string, string[]> = $state({});

  /**
   * Per-repo focused terminal id (session-level only — terminals are not
   * restored across restarts, so nothing here persists).
   */
  active: Record<string, string> = $state({});

  #factory: TerminalFactory | null = null;
  #unregisterEvents: (() => void) | null = null;
  /** pty session id → composite session key (non-reactive dispatch index). */
  readonly #bySession = new Map<string, string>();
  /** session key → freshness token; async `pty_create` continuations check it. */
  readonly #tokens = new Map<string, object>();
  readonly #resizeTimers = new Map<string, ReturnType<typeof setTimeout>>();

  /** Registers the terminal UI factory (the panel does this once). */
  setFactory(factory: TerminalFactory | null): void {
    this.#factory = factory;
  }

  /** The repo's ordered terminal ids (empty array when it has none). */
  list(repoId: string): string[] {
    return this.order[repoId] ?? [];
  }

  /** The repo's focused terminal id, or `null` when it has no terminals. */
  activeId(repoId: string): string | null {
    return this.active[repoId] ?? this.list(repoId)[0] ?? null;
  }

  /** The repo's ACTIVE session record, or `null` when none exists. */
  session(repoId: string): TerminalSession | null {
    const id = this.activeId(repoId);
    return id === null ? null : (this.sessions[sessionKey(repoId, id)] ?? null);
  }

  /** One specific terminal's session, or `null` for unknown ids. */
  sessionById(repoId: string, terminalId: string): TerminalSession | null {
    return this.sessions[sessionKey(repoId, terminalId)] ?? null;
  }

  /**
   * The ACTIVE terminal's session, creating it lazily (terminal UI + pty).
   * Safe to call from a mount `$effect`: the first call creates, later ones
   * return the existing starting/live session unchanged. `remote` selects
   * the remote-shell transport for repositories on an SSH device.
   */
  ensure(repoId: string, remote = false): TerminalSession | null {
    let id = this.activeId(repoId);
    if (id === null) {
      // First terminal of the repo — allocate its tab slot.
      id = this.#allocate(repoId);
      this.active[repoId] = id;
    }
    const key = sessionKey(repoId, id);
    const existing = this.sessions[key];
    if (existing && (existing.status === "starting" || existing.status === "live")) {
      return existing;
    }
    return this.#create(repoId, id, remote);
  }

  /**
   * Opens a NEW terminal tab for the repo ("+" button): mints the next
   * terminal id, appends it to the tab order, activates it and creates its
   * session. Returns `null` (no-op) when the per-repo cap
   * {@link MAX_TERMINALS_PER_REPO} is reached.
   */
  create(repoId: string, remote = false): TerminalSession | null {
    if (this.list(repoId).length >= MAX_TERMINALS_PER_REPO) return null;
    const id = this.#allocate(repoId);
    this.active[repoId] = id;
    return this.#create(repoId, id, remote);
  }

  /** Focuses one of the repo's terminal tabs (no-op for unknown ids). */
  setActive(repoId: string, terminalId: string): void {
    if (!this.list(repoId).includes(terminalId)) return;
    this.active[repoId] = terminalId;
  }

  /** Kill + fresh session for the ACTIVE terminal (the Restart button). */
  restart(repoId: string, remote = false): TerminalSession | null {
    const id = this.activeId(repoId);
    if (id === null) return this.ensure(repoId, remote);
    const key = sessionKey(repoId, id);
    this.#teardown(key, { markExited: false, remove: false });
    return this.#create(repoId, id, remote);
  }

  /** User stop: kills the ACTIVE pty and marks the session exited. */
  kill(repoId: string): void {
    const id = this.activeId(repoId);
    if (id === null) return;
    this.#teardown(sessionKey(repoId, id), { markExited: true, remove: false });
  }

  /**
   * Closes ONE terminal tab: kills its pty, drops the session, removes it
   * from the tab order and re-focuses the neighbor (left preferred, then
   * right — the tab-strip rule). Closing the last terminal leaves the repo
   * without terminals (the panel's next `ensure` starts a fresh one).
   */
  close(repoId: string, terminalId: string): void {
    const ids = this.list(repoId);
    const index = ids.indexOf(terminalId);
    if (index === -1) return;
    this.#teardown(sessionKey(repoId, terminalId), { markExited: false, remove: true });
    const remaining = ids.filter((id) => id !== terminalId);
    if (remaining.length === 0) {
      delete this.order[repoId];
      delete this.active[repoId];
      return;
    }
    this.order[repoId] = remaining;
    if (this.active[repoId] === terminalId) {
      // Left neighbor preferred, then the first remaining one.
      this.active[repoId] = remaining[Math.max(0, index - 1)]!;
    }
  }

  /**
   * Repo teardown (RepoView unmount = the repo tab was closed): kills and
   * forgets ALL of the repo's terminals (every tab), plus the tab order and
   * the focused id. No-op for unknown repos.
   */
  dispose(repoId: string): void {
    for (const id of this.list(repoId)) {
      this.#teardown(sessionKey(repoId, id), { markExited: false, remove: true });
    }
    delete this.order[repoId];
    delete this.active[repoId];
  }

  /**
   * Writes a snippet command (plus Enter) into the ACTIVE terminal's pty —
   * the same channel the keystroke stream uses, so the shell sees and
   * echoes it like typed input. Dropped silently (returns `false`) while
   * the session has no live pty (starting / exited / unavailable).
   */
  writeInput(repoId: string, data: string): boolean {
    const entry = this.session(repoId);
    if (!entry?.sessionId) return false;
    void this.#writeShell(entry.sessionId, data, entry.remote).catch(() => {});
    return true;
  }

  /**
   * Size sync (panel ResizeObserver → fit addon → here). Debounced
   * {@link RESIZE_DEBOUNCE_MS}; silently skipped while the ACTIVE session
   * has no live pty (the panel refits — and lands here again — once it
   * attaches). Only the visible terminal is ever measured.
   */
  resize(repoId: string, cols: number, rows: number): void {
    const id = this.activeId(repoId);
    if (id === null) return;
    const key = sessionKey(repoId, id);
    if (!this.sessions[key]?.sessionId) return;
    const timer = this.#resizeTimers.get(key);
    if (timer) clearTimeout(timer);
    this.#resizeTimers.set(
      key,
      setTimeout(() => {
        this.#resizeTimers.delete(key);
        const entry = this.sessions[key];
        if (!entry?.sessionId) return;
        void this.#resizeShell(entry.sessionId, rows, cols, entry.remote).catch(() => {});
      }, RESIZE_DEBOUNCE_MS),
    );
  }

  /** Deregisters the pty events (test/teardown aid). */
  stop(): void {
    this.#unregisterEvents?.();
    this.#unregisterEvents = null;
  }

  // -- internals -------------------------------------------------------------

  // Transport seam (local pty vs remote shell): both share the session-id
  // shape and the `pty-output` / `pty-exit` events, so everything else in
  // the store is transport-agnostic.
  #createShell(
    repoId: string,
    remote: boolean,
    rows?: number,
    cols?: number,
  ): Promise<string> {
    return remote ? rshellCreate(repoId, rows, cols) : ptyCreate(repoId, rows, cols);
  }

  #writeShell(sessionId: string, data: string, remote: boolean): Promise<void> {
    return remote ? rshellWrite(sessionId, data) : ptyWrite(sessionId, data);
  }

  #resizeShell(
    sessionId: string,
    rows: number,
    cols: number,
    remote: boolean,
  ): Promise<void> {
    return remote
      ? rshellResize(sessionId, rows, cols)
      : ptyResize(sessionId, rows, cols);
  }

  #killShell(sessionId: string, remote: boolean): Promise<void> {
    return remote ? rshellKill(sessionId) : ptyKill(sessionId);
  }

  /** Mints the next per-repo terminal id and appends it to the tab order. */
  #allocate(repoId: string): string {
    const ids = this.list(repoId);
    // Highest numeric suffix + 1: ids never collide, even after closes.
    let max = 0;
    for (const id of ids) {
      const n = Number(id.replace(/^t/, ""));
      if (Number.isInteger(n) && n > max) max = n;
    }
    const next = `t${max + 1}`;
    this.order[repoId] = [...ids, next];
    return next;
  }

  #create(repoId: string, terminalId: string, remote = false): TerminalSession | null {
    const key = sessionKey(repoId, terminalId);
    if (!isTauri()) {
      this.sessions[key] = unavailableSession(repoId, terminalId, null);
      return this.sessions[key];
    }
    if (!this.#factory) {
      this.sessions[key] = unavailableSession(repoId, terminalId, "no terminal factory");
      return this.sessions[key];
    }
    this.#startEvents();

    let handle: TerminalHandle;
    try {
      handle = this.#factory();
    } catch (err) {
      const message = describeError(err);
      this.sessions[key] = {
        repoId,
        terminalId,
        sessionId: null,
        remote,
        status: "failed",
        term: null,
        fit: null,
        setTheme: null,
        error: message,
      };
      return this.sessions[key];
    }

    this.sessions[key] = {
      repoId,
      terminalId,
      sessionId: null,
      remote,
      status: "starting",
      term: handle.term,
      fit: handle.fit,
      setTheme: handle.setTheme,
      error: null,
    };
    const entry = this.sessions[key];
    const token = {};
    this.#tokens.set(key, token);

    // Keystrokes: drop silently until the pty id arrives (first ms only).
    // The closure is scoped to THIS terminal's session key — a keystroke
    // can never land in another tab's shell.
    entry.term?.onData((data) => {
      const current = this.sessions[key];
      if (!current?.sessionId) return;
      void this.#writeShell(current.sessionId, data, current.remote).catch(() => {});
    });

    this.#createShell(repoId, remote, handle.term.rows, handle.term.cols)
      .then((sessionId) => {
        if (this.#tokens.get(key) !== token) {
          // Session was torn down / replaced meanwhile: kill the orphan.
          void this.#killShell(sessionId, remote).catch(() => {});
          return;
        }
        const current = this.sessions[key];
        if (!current || current.status !== "starting") {
          void this.#killShell(sessionId, remote).catch(() => {});
          return;
        }
        current.sessionId = sessionId;
        current.status = "live";
        this.#bySession.set(sessionId, key);
      })
      .catch((err) => {
        if (this.#tokens.get(key) !== token) return;
        this.#tokens.delete(key);
        const message = describeError(err);
        const current = this.sessions[key];
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
    key: string,
    opts: { markExited: boolean; remove: boolean },
  ): void {
    this.#tokens.delete(key);
    this.#cancelResize(key);
    const entry = this.sessions[key];
    if (!entry) return;
    if (entry.sessionId) {
      this.#bySession.delete(entry.sessionId);
      void this.#killShell(entry.sessionId, entry.remote).catch(() => {});
    }
    entry.term?.dispose();
    if (opts.remove) {
      delete this.sessions[key];
      return;
    }
    entry.sessionId = null;
    entry.term = null;
    entry.fit = null;
    entry.setTheme = null;
    if (opts.markExited) entry.status = "exited";
  }

  #startEvents(): void {
    if (this.#unregisterEvents !== null) return;
    // Registered through the shared event module (when-first-needed: the
    // backend subscriptions start with the first terminal creation); the
    // per-session dispatch stays here.
    const unregisterOutput = onPtyOutput((event) =>
      this.#applyOutput(event.session_id, event.data),
    );
    const unregisterExit = onPtyExit((event: PtyExitEvent) =>
      // The wire field is `code` (contracts.md M5; the type comes from
      // $lib/ipc/types, generated from the Rust `PtyExit`). The old
      // hand-written mirror said `exit_code`, a field the payload never
      // carried — exit codes were silently `undefined` before.
      this.#applyExit(event.session_id, event.code),
    );
    this.#unregisterEvents = () => {
      unregisterOutput();
      unregisterExit();
    };
  }

  #applyOutput(sessionId: string, data: string): void {
    const key = this.#bySession.get(sessionId);
    if (key === undefined) return;
    this.sessions[key]?.term?.write(data);
  }

  #applyExit(sessionId: string, code: number): void {
    const key = this.#bySession.get(sessionId);
    if (key === undefined) return;
    this.#bySession.delete(sessionId);
    this.#tokens.delete(key);
    this.#cancelResize(key);
    const entry = this.sessions[key];
    if (entry) {
      entry.sessionId = null;
      entry.status = "exited";
      entry.term?.dispose();
      entry.term = null;
      entry.fit = null;
      entry.setTheme = null;
    }
    toast(`Terminal exited (code ${code})`);
  }

  #cancelResize(key: string): void {
    const timer = this.#resizeTimers.get(key);
    if (timer) clearTimeout(timer);
    this.#resizeTimers.delete(key);
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
      // #AARRGGBB hex from the backend — alpha is dropped here.
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
  ["black", "--card-variant", "#49454f"],
  ["red", "--destructive", "#f2b8b5"],
  ["green", "--muted", "#efb8c8"],
  ["yellow", "--secondary", "#ccc2dc"],
  ["blue", "--primary", "#d0bcff"],
  ["magenta", "--primary", "#6750a4"],
  ["cyan", "--secondary-fixed", "#e8def8"],
  ["white", "--border", "#938f99"],
  ["brightBlack", "--border", "#4a4458"],
  ["brightRed", "--destructive", "#f9dedc"],
  ["brightGreen", "--muted-foreground", "#31111d"],
  ["brightYellow", "--secondary-foreground", "#1d192b"],
  ["brightBlue", "--accent-foreground", "#4f378b"],
  ["brightMagenta", "--primary-fixed", "#eaddff"],
  ["brightCyan", "--muted-fixed", "#ffd8e4"],
  ["brightWhite", "--foreground", "#e6e0e9"],
];

/** Builds an xterm theme from the current M3 scheme (dark/light aware). */
export function terminalTheme(): TerminalTheme {
  const background = cssVar("--card", "#141218");
  const foreground = cssVar("--foreground", "#e6e0e9");
  const primary = cssVar("--primary", "#d0bcff");
  const theme: TerminalTheme = {
    background,
    foreground,
    cursor: primary,
    cursorAccent: cssVar("--primary-foreground", "#381e72"),
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

