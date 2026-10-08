/**
 * Keyboard-shortcut engine (M4 F2) — action-id → key-combo bindings with
 * user overrides.
 *
 * Pure pieces (combo parsing/canonicalization, event matching, conflict
 * detection, override persistence) live here as dependency-free functions so
 * they unit-test without a DOM; `startKeybinds()` wires the single capture-
 * phase `window` keydown listener that maps events to command ids.
 *
 * Combo strings are platform-agnostic and canonical: lowercase modifiers in
 * `ctrl` < `alt` < `shift` order, then the key (`"ctrl+shift+p"`,
 * `"ctrl+alt+arrowright"`, `"f5"`). On macOS `Cmd` plays `Ctrl`'s role: the
 * matcher normalizes `metaKey` to `ctrl` there (and accepts physical Ctrl
 * too), so one stored binding works on every platform. User overrides
 * persist to localStorage `"mygitui.keybinds"` as `{commandId: combo}`.
 *
 * Editable-target rule: while the focus is in an input/textarea/content-
 * editable, only combos containing at least one modifier (ctrl/alt) are
 * honored — modifier-less keys (plain letters, F5) are ignored so typing is
 * never hijacked.
 */

/** Minimal storage surface this module needs (subset of DOM `Storage`). */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** localStorage key holding user overrides (`{commandId: combo}`). */
export const KEYBINDS_STORAGE_KEY = "mygitui.keybinds";

/** Normalized platform for matcher semantics. */
export type Platform = "mac" | "pc";

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/**
 * Default bindings, keyed by command id (the ids in `$lib/palette/commands`).
 * The quick switcher's cmd/ctrl+K is intentionally NOT here — the switcher
 * owns its shortcut in its own component (B4 lane).
 */
export const DEFAULT_BINDINGS: Record<string, string> = {
  "app.palette": "ctrl+shift+p",
  "repo.refresh": "f5",
  "tab.next": "ctrl+alt+arrowright",
  "tab.prev": "ctrl+alt+arrowleft",
  "panel.focus-status": "ctrl+1",
  "panel.focus-branches": "ctrl+2",
  "panel.focus-remotes": "ctrl+3",
  "panel.focus-stashes": "ctrl+4",
  "panel.focus-worktrees": "ctrl+5",
  "panel.focus-reflog": "ctrl+6",
  "panel.focus-undo": "ctrl+7",
};

// ---------------------------------------------------------------------------
// Combo parsing / canonicalization
// ---------------------------------------------------------------------------

/** Parsed combo: modifier flags plus the (normalized) base key. */
export interface ComboParts {
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  /** Lowercase base key (`"p"`, `"f5"`, `"arrowright"`, `"space"`, …). */
  key: string;
}

const MODIFIER_ALIASES: Record<string, "ctrl" | "alt" | "shift"> = {
  ctrl: "ctrl",
  control: "ctrl",
  alt: "alt",
  option: "alt",
  opt: "alt",
  shift: "shift",
  // Meta-ish names all canonicalize to ctrl (platform-agnostic storage):
  meta: "ctrl",
  cmd: "ctrl",
  command: "ctrl",
  super: "ctrl",
  win: "ctrl",
};

/** Normalizes a key token (`" "` → `"space"`, `"Esc"` → `"escape"`, …). */
export function normalizeKey(raw: string): string {
  if (raw === " ") return "space"; // trim would eat a bare space key
  const key = raw.trim().toLowerCase();
  if (key === "spacebar") return "space";
  if (key === "esc") return "escape";
  if (key === "return") return "enter";
  if (key === "del") return "delete";
  if (key === "plus") return "+";
  return key;
}

/** Modifier names that never appear as a combo's base key. */
export function isModifierName(key: string): boolean {
  return key in MODIFIER_ALIASES;
}

/** Serializes parts back to the canonical `ctrl+alt+shift+key` form. */
export function comboToString(parts: ComboParts): string {
  const tokens: string[] = [];
  if (parts.ctrl) tokens.push("ctrl");
  if (parts.alt) tokens.push("alt");
  if (parts.shift) tokens.push("shift");
  tokens.push(parts.key);
  return tokens.join("+");
}

/**
 * Parses a combo string into canonical parts, or `null` when it is not a
 * usable binding (empty, modifier-only, or an empty base key like
 * `"ctrl++"` after normalization).
 */
export function parseCombo(input: string): ComboParts | null {
  const parts: ComboParts = { ctrl: false, alt: false, shift: false, key: "" };
  let sawToken = false;
  for (const rawToken of input.split("+")) {
    const token = rawToken.trim().toLowerCase();
    if (token === "") continue;
    sawToken = true;
    const modifier = MODIFIER_ALIASES[token];
    if (modifier) {
      if (modifier === "ctrl") parts.ctrl = true;
      else if (modifier === "alt") parts.alt = true;
      else parts.shift = true;
      continue;
    }
    if (parts.key !== "") return null; // two base keys — not a combo
    parts.key = normalizeKey(token);
  }
  if (!sawToken || parts.key === "" || isModifierName(parts.key)) return null;
  if (/\s/.test(parts.key)) return null; // real key names have no spaces
  return parts;
}

/** Canonical form of a user-typed combo, or `null` when invalid. */
export function canonicalCombo(input: string): string | null {
  const parts = parseCombo(input);
  return parts === null ? null : comboToString(parts);
}

/** True when the combo carries at least one modifier (ctrl/alt/shift). */
export function hasModifier(combo: string): boolean {
  const parts = parseCombo(combo);
  return parts !== null && (parts.ctrl || parts.alt || parts.shift);
}

/** Display form for hints: `"ctrl+shift+p"` → `"Ctrl+Shift+P"`. */
export function formatCombo(combo: string): string {
  const parts = parseCombo(combo);
  if (parts === null) return combo;
  const tokens: string[] = [];
  if (parts.ctrl) tokens.push("Ctrl");
  if (parts.alt) tokens.push("Alt");
  if (parts.shift) tokens.push("Shift");
  const key = parts.key;
  tokens.push(
    key.length === 1
      ? key.toUpperCase()
      : key.charAt(0).toUpperCase() + key.slice(1),
  );
  return tokens.join("+");
}

// ---------------------------------------------------------------------------
// Event → combo matching
// ---------------------------------------------------------------------------

/** True when `name` is the platform's canonical "ctrl" for this event. */
function eventCtrl(event: KeyboardEvent, platform: Platform): boolean {
  // On mac, Cmd (metaKey) plays Ctrl's role; physical Ctrl is accepted too.
  return platform === "mac"
    ? event.metaKey || event.ctrlKey
    : event.ctrlKey;
}

/**
 * Builds the canonical combo for a keydown event, or `null` when the event
 * cannot be a binding (modifier-only key press, or meta held on non-mac
 * where it would shadow an OS shortcut).
 */
export function comboFromEvent(
  event: KeyboardEvent,
  platform: Platform,
): string | null {
  if (platform !== "mac" && event.metaKey) return null;
  const key = normalizeKey(event.key);
  if (key === "" || isModifierName(key)) return null;
  return comboToString({
    ctrl: eventCtrl(event, platform),
    alt: event.altKey,
    shift: event.shiftKey,
    key,
  });
}

/**
 * Matches a keydown event against `bindings` (commandId → combo). Returns
 * the first matching command id, or `null`.
 */
export function matchEvent(
  event: KeyboardEvent,
  bindings: Record<string, string>,
  platform: Platform,
): string | null {
  const combo = comboFromEvent(event, platform);
  if (combo === null) return null;
  for (const [commandId, binding] of Object.entries(bindings)) {
    if (binding === combo) return commandId;
  }
  return null;
}

/**
 * Conflict detection: groups bindings by combo and returns every combo held
 * by more than one command (the warn list for a bindings UI).
 */
export function findConflicts(
  bindings: Record<string, string>,
): Array<{ combo: string; commandIds: string[] }> {
  const byCombo = new Map<string, string[]>();
  for (const [commandId, combo] of Object.entries(bindings)) {
    const list = byCombo.get(combo) ?? [];
    list.push(commandId);
    byCombo.set(combo, list);
  }
  const conflicts: Array<{ combo: string; commandIds: string[] }> = [];
  for (const [combo, commandIds] of byCombo) {
    if (commandIds.length > 1) {
      conflicts.push({ combo, commandIds: [...commandIds].sort() });
    }
  }
  conflicts.sort((a, b) => (a.combo < b.combo ? -1 : a.combo > b.combo ? 1 : 0));
  return conflicts;
}

// ---------------------------------------------------------------------------
// Overrides persistence
// ---------------------------------------------------------------------------

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Storage the module singleton persists to (rebindable in tests). */
let storageRef: StorageLike | null = defaultStorage();

/** Parses persisted overrides JSON, dropping malformed entries. */
function parseOverrides(raw: string | null): Record<string, string> {
  if (raw === null || raw === "") return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return {};
  }
  const overrides: Record<string, string> = {};
  for (const [id, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (typeof id !== "string" || id === "" || typeof value !== "string") continue;
    const canonical = canonicalCombo(value);
    if (canonical === null) continue; // drop malformed entries
    overrides[id] = canonical;
  }
  return overrides;
}

/** Loads user overrides (invalid JSON / entries are dropped silently). */
export function loadOverrides(storage: StorageLike | null = storageRef): Record<string, string> {
  if (storage === null) return {};
  try {
    return parseOverrides(storage.getItem(KEYBINDS_STORAGE_KEY));
  } catch {
    return {};
  }
}

/** Persists user overrides (storage failures are swallowed). */
export function saveOverrides(
  overrides: Record<string, string>,
  storage: StorageLike | null = storageRef,
): void {
  if (storage === null) return;
  try {
    storage.setItem(KEYBINDS_STORAGE_KEY, JSON.stringify(overrides));
  } catch {
    // Storage may be full or unavailable; bindings stay in memory.
  }
}

/** Default bindings merged with user overrides (overrides win). */
export function effectiveBindings(overrides: Record<string, string>): Record<string, string> {
  return { ...DEFAULT_BINDINGS, ...overrides };
}

// ---------------------------------------------------------------------------
// Module singleton (live bindings + listener)
// ---------------------------------------------------------------------------

let overrides: Record<string, string> = loadOverrides();/** User overrides currently in memory (what `setBinding` et al. mutate). */
export function getOverrides(): Record<string, string> {
  return { ...overrides };
}

/** Effective bindings right now (defaults + in-memory overrides). */
export function currentBindings(): Record<string, string> {
  return effectiveBindings(overrides);
}

/** The effective combo for `commandId`, or `null` when unbound. */
export function getBinding(commandId: string): string | null {
  const bindings = currentBindings();
  return commandId in bindings ? bindings[commandId]! : null;
}

/**
 * Sets (or changes) a command's binding, persists the override and returns
 * the canonical combo. `null` return means the combo was invalid.
 */
export function setBinding(commandId: string, combo: string): string | null {
  const canonical = canonicalCombo(combo);
  if (canonical === null) return null;
  // An override equal to the default is redundant — drop it instead.
  if (DEFAULT_BINDINGS[commandId] === canonical) {
    delete overrides[commandId];
  } else {
    overrides = { ...overrides, [commandId]: canonical };
  }
  saveOverrides(overrides);
  return canonical;
}

/** Clears one command's override (falls back to its default). */
export function clearBinding(commandId: string): void {
  if (!(commandId in overrides)) return;
  const next = { ...overrides };
  delete next[commandId];
  overrides = next;
  saveOverrides(overrides);
}

/** Clears every override (all commands back to defaults). */
export function resetBindings(): void {
  overrides = {};
  saveOverrides(overrides);
}

/** Detects the matcher platform from the user agent (mac → "mac"). */
export function detectPlatform(): Platform {
  try {
    const ua = globalThis.navigator?.userAgent ?? "";
    return /mac|iphone|ipad|ipod/i.test(ua) ? "mac" : "pc";
  } catch {
    return "pc";
  }
}

/** True when the focus sits in an editable field (see module doc). */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.tagName === "SELECT" ||
    target.isContentEditable === true
  );
}

let cleanup: (() => void) | null = null;

/**
 * Installs the global keydown listener (capture phase, on `window`): matches
 * every event against the current bindings and invokes `onAction(commandId)`
 * for hits, swallowing the event (`preventDefault` + `stopPropagation`) so
 * shortcuts never leak into inputs. Idempotent — a second call attaches
 * nothing and returns a no-op cleanup. The returned function detaches.
 */
export function startKeybinds(onAction: (commandId: string) => void): () => void {
  if (cleanup !== null) return () => {};
  const platform = detectPlatform();
  const handler = (event: KeyboardEvent): void => {
    const combo = comboFromEvent(event, platform);
    if (combo === null) return;
    if (isEditableTarget(event.target) && !hasModifier(combo)) return;
    const commandId = matchEvent(event, currentBindings(), platform);
    if (commandId === null) return;
    event.preventDefault();
    event.stopPropagation();
    onAction(commandId);
  };
  window.addEventListener("keydown", handler, true);
  cleanup = () => {
    window.removeEventListener("keydown", handler, true);
    cleanup = null;
  };
  return cleanup;
}

/**
 * Test helper: points the singleton at `storage` (`null` = in-memory only),
 * drops in-memory overrides and any installed listener.
 */
export function __resetKeybindsForTests(storage: StorageLike | null = null): void {
  cleanup?.();
  cleanup = null;
  storageRef = storage;
  overrides = {};
}
