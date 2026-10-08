/**
 * Command-palette store (M4 F2).
 *
 * Lives in a `.svelte.ts` module because the open/closed flag is a `$state`
 * rune. Hosts the palette's pure pieces so they are unit-testable without a
 * DOM: a small fuzzy scorer (own implementation — deliberately not the
 * switcher's), ranking, section grouping with a "Recent" block, the last-5
 * recents list (localStorage `mygitui.palette.recents`) and command
 * execution (close → record recent → run → toast failures).
 */

import { getToasts, toast } from "$lib/toast";
import {
  COMMANDS,
  activeCtx,
  type Command,
  type CommandCtx,
} from "$lib/palette/commands";
import { getBinding } from "$lib/palette/keybinds";

/** localStorage key holding the last-run command ids. */
export const PALETTE_RECENTS_KEY = "mygitui.palette.recents";

/** How many recent commands the palette pins to the top. */
export const RECENTS_LIMIT = 5;

// ---------------------------------------------------------------------------
// Fuzzy scoring (own scorer, switcher-independent)
// ---------------------------------------------------------------------------

/** A scored match: total score plus the matched haystack indexes. */
export interface FuzzyHit {
  score: number;
  positions: number[];
}

/**
 * Case-insensitive greedy subsequence scorer with bonuses for consecutive
 * runs and word starts (separator or camelCase boundaries) and a capped gap
 * penalty. Returns `null` when `needle` is not a subsequence of `haystack`.
 */
export function fuzzyScore(needle: string, haystack: string): FuzzyHit | null {
  if (needle.length === 0) return { score: 0, positions: [] };
  const n = needle.toLowerCase();
  const h = haystack.toLowerCase();
  let score = 0;
  let from = 0;
  let prev = -2;
  const positions: number[] = [];
  for (let ni = 0; ni < n.length; ni++) {
    const found = h.indexOf(n[ni]!, from);
    if (found === -1) return null;
    positions.push(found);
    score += 1;
    if (found === prev + 1) score += 3;
    if (isWordStart(haystack, found)) score += 2;
    score -= Math.min(found - from, 8) * 0.25;
    from = found + 1;
    prev = found;
  }
  return { score, positions };
}

/** True when `haystack[i]` starts a word (string start / separator / camel). */
function isWordStart(haystack: string, index: number): boolean {
  if (index === 0) return true;
  const prev = haystack[index - 1]!;
  if (/[\s/\\._\-:]/.test(prev)) return true;
  const current = haystack[index]!;
  return prev === prev.toLowerCase() && current !== current.toLowerCase();
}

// ---------------------------------------------------------------------------
// Ranking + sections
// ---------------------------------------------------------------------------

/** A command with its palette score, ready to render. */
export interface RankedCommand {
  command: Command;
  score: number;
}

/** One rendered group: a section heading and its ranked commands. */
export interface PaletteSection {
  name: string;
  commands: RankedCommand[];
}

/** What the scorer ranks against: title, section and keywords. */
function searchHaystack(command: Command): string {
  return [command.section, command.title, ...(command.keywords ?? [])].join(" ");
}

/**
 * Ranks `commands` against `query`, dropping non-matches and sorting by
 * score descending (stable: ties keep registry order). An empty query keeps
 * everything at score 0.
 */
export function rankCommands(
  commands: readonly Command[],
  query: string,
): RankedCommand[] {
  const ranked: RankedCommand[] = [];
  for (const command of commands) {
    const hit = fuzzyScore(query, searchHaystack(command));
    if (hit === null) continue;
    ranked.push({ command, score: hit.score });
  }
  ranked.sort((a, b) => b.score - a.score);
  return ranked;
}

/**
 * Builds the palette's section list from already-filtered commands:
 * with a query — ranked matches grouped under their section headings
 * (registry order); without — the full registry grouped, optionally with a
 * "Recent" block on top (recents that still exist, recency order) and those
 * commands removed from their home sections.
 */
export function paletteSections(
  commands: readonly Command[],
  ctx: CommandCtx,
  query: string,
  recents: readonly string[] = [],
): PaletteSection[] {
  const visible = commands.filter((command) => command.when?.(ctx) ?? true);

  if (query.trim() === "") {
    const recentIds = new Set(
      recents.filter((id) => visible.some((command) => command.id === id)),
    );
    const sections: PaletteSection[] = [];
    if (recentIds.size > 0) {
      sections.push({
        name: "Recent",
        commands: recents
          .filter((id) => recentIds.has(id))
          .map((id) => visible.find((command) => command.id === id))
          .filter((command): command is Command => command !== undefined)
          .map((command) => ({ command, score: 0 })),
      });
    }
    sections.push(...groupBySection(visible, (id) => !recentIds.has(id)));
    return sections;
  }

  return groupByRanked(rankCommands(visible, query));
}

/** Groups registry-ordered commands, keeping order inside each section. */
function groupBySection(
  commands: readonly Command[],
  keep: (id: string) => boolean = () => true,
): PaletteSection[] {
  const sections: PaletteSection[] = [];
  const byName = new Map<string, PaletteSection>();
  for (const command of commands) {
    if (!keep(command.id)) continue;
    let section = byName.get(command.section);
    if (section === undefined) {
      section = { name: command.section, commands: [] };
      byName.set(command.section, section);
      sections.push(section);
    }
    section.commands.push({ command, score: 0 });
  }
  return sections;
}

/** Groups ranked commands by section without disturbing the ranking. */
function groupByRanked(ranked: readonly RankedCommand[]): PaletteSection[] {
  const sections: PaletteSection[] = [];
  const byName = new Map<string, PaletteSection>();
  for (const entry of ranked) {
    let section = byName.get(entry.command.section);
    if (section === undefined) {
      section = { name: entry.command.section, commands: [] };
      byName.set(entry.command.section, section);
      sections.push(section);
    }
    section.commands.push(entry);
  }
  return sections;
}

/** Flattens sections into the arrow-key navigation order. */
export function flattenSections(
  sections: readonly PaletteSection[],
): RankedCommand[] {
  return sections.flatMap((section) => section.commands);
}

// ---------------------------------------------------------------------------
// Recents
// ---------------------------------------------------------------------------

/** Minimal storage surface (subset of DOM `Storage`). */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Parses the persisted recent list, dropping junk (ids are opaque strings). */
function parseRecents(raw: string | null): string[] {
  if (raw === null || raw === "") return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed.filter((id): id is string => typeof id === "string" && id !== "");
}

/** Loads the recent-command ids (newest first), capped at {@link RECENTS_LIMIT}. */
export function getRecents(storage: StorageLike | null = defaultStorage()): string[] {
  if (storage === null) return [];
  try {
    return parseRecents(storage.getItem(PALETTE_RECENTS_KEY)).slice(0, RECENTS_LIMIT);
  } catch {
    return [];
  }
}

/** Persists the recent list (storage failures are swallowed). */
function saveRecents(recents: readonly string[], storage: StorageLike | null): void {
  if (storage === null) return;
  try {
    storage.setItem(PALETTE_RECENTS_KEY, JSON.stringify(recents));
  } catch {
    // Storage may be full or unavailable; recents stay in memory.
  }
}

/** Moves `id` to the front of the recents list and persists it. */
export function recordRecent(id: string, storage: StorageLike | null = defaultStorage()): void {
  recentsCache = [id, ...recentsCache.filter((recent) => recent !== id)].slice(
    0,
    RECENTS_LIMIT,
  );
  saveRecents(recentsCache, storage);
}

/** In-memory recents (what the palette shows; refreshed by recordRecent). */
export function currentRecents(): string[] {
  return [...recentsCache];
}

/** Test helper: clears the in-memory recents cache. */
export function resetRecents(storage: StorageLike | null = defaultStorage()): void {
  recentsCache = storage === null ? [] : getRecents(storage);
}

// In-memory mirror of the persisted list (read once, kept fresh by
// recordRecent) so the palette does not touch localStorage per keystroke.
let recentsCache: string[] = loadRecentsCache();

function loadRecentsCache(): string[] {
  try {
    return getRecents();
  } catch {
    return [];
  }
}

// ---------------------------------------------------------------------------
// Open/close + execution
// ---------------------------------------------------------------------------

let open = $state(false);

export function isPaletteOpen(): boolean {
  return open;
}

export function openPalette(): void {
  open = true;
}

export function closePalette(): void {
  open = false;
}

export function togglePalette(): void {
  open = !open;
}

/**
 * Runs `command` against `ctx`: closes the palette, records the recent, then
 * executes — failures toast (the palette is already closed by then).
 */
export async function runCommand(command: Command, ctx: CommandCtx): Promise<void> {
  closePalette();
  recordRecent(command.id);
  try {
    await command.run(ctx);
  } catch (err) {
    toast(`Command failed: ${err instanceof Error ? err.message : String(err)}`, {
      kind: "error",
    });
  }
}

/**
 * Executes a command by id (registry order) against the current active-tab
 * context — the entry point the keybind engine's `onAction` calls. Unknown
 * ids are ignored silently (e.g. registry shrunk underneath an override).
 */
export async function executeCommandById(id: string): Promise<void> {
  const command = COMMANDS.find((entry) => entry.id === id);
  if (command === undefined) return;
  await runCommand(command, activeCtx());
}

/** The effective shortcut hint for a command, or `null` when unbound. */
export function shortcutHint(command: Command): string | null {
  return getBinding(command.id);
}

/** Test seam: open flag + recents + toast count in one snapshot. */
export function paletteSnapshot(): {
  open: boolean;
  recents: string[];
  toastCount: number;
} {
  return { open, recents: [...recentsCache], toastCount: getToasts().length };
}
