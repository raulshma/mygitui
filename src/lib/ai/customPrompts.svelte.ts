/**
 * Custom prompt library (Svelte 5 runes) — user-editable AI prompt
 * templates, GLOBAL (app-wide), unlike the per-repo config the AiStore
 * keeps. Each enabled entry surfaces as a runnable feature kind
 * `` `custom.<entryId>` `` via the registry's query-time merge
 * (`registry.ts` resolves it through the provider registered below) and a
 * headless palette command (`ai.custom.<id>` in `palette/commands.ts`).
 *
 * Templates support the SAME variable placeholders the builtin prompt
 * builders gather data for — the documented set is
 * {@link CUSTOM_PROMPT_VARS}:
 *
 *   - `{{diff}}`    — the budgeted staged diff (`budgetedDiffText`, the
 *                     commit-message runner's rendering: stat summary
 *                     first, fair per-file slices, explicit omissions);
 *   - `{{summary}}` — per-file add/del stats (`diffSummaryText`);
 *   - `{{commits}}` — the branch's recent commits (first log page,
 *                     sha + summary lines, capped at 20).
 *
 * Variables are gathered ONLY when the template references them, and
 * interpolated by {@link renderTemplate} — the one helper shared by the
 * store (validation previews) and the runner (`features.ts`). Placeholders
 * without a gathered value — and unknown `{{...}}` placeholders — are left
 * VERBATIM, so an ungathered variable stays visible in the prompt instead
 * of silently vanishing (and prose like "{{user notes here}}" survives).
 *
 * Persistence follows the versioned envelope (`$lib/state/persist`)
 * under `mygitui.ai.customPrompts`, parse-and-drop: malformed entries are
 * dropped on load, templates are capped at {@link CUSTOM_TEMPLATE_CAP} on
 * both write and load (a runaway paste must not wedge storage), ids are
 * deduplicated. Lives in a `.svelte.ts` module because `$state` only
 * compiles there; tests construct isolated instances with an in-memory
 * storage.
 */

import { loadVersioned, writeEnvelope } from "$lib/state/persist";
import type { StorageLike } from "$lib/state/persist";
import { registerCustomPromptProvider } from "./registry";

/** localStorage key for the custom prompt library envelope. */
export const CUSTOM_PROMPTS_KEY = "mygitui.ai.customPrompts";

/** Max characters of template text per entry (write- and load-side). */
export const CUSTOM_TEMPLATE_CAP = 8_000;

/** Max entries before "add" refuses (localStorage quota guard). */
export const CUSTOM_PROMPTS_CAP = 50;

/** The documented variable placeholder set a template may reference. */
export const CUSTOM_PROMPT_VARS = ["diff", "summary", "commits"] as const;

/** One user-defined prompt template. */
export interface CustomPromptEntry {
  /** Stable id (also the feature kind suffix: `custom.<id>`). */
  id: string;
  /** Display name (registry title / palette label / settings row). */
  name: string;
  /** Raw template with `{{var}}` placeholders (capped). */
  template: string;
  /** Disabled entries stay stored but never surface as features. */
  enabled: boolean;
}

/** Placeholder regex: `{{ name }}` with optional inner whitespace. */
const PLACEHOLDER = /\{\{\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*\}\}/g;

/**
 * Replaces every `{{name}}` placeholder present in `vars` with its value;
 * anything else — unknown names, unpadded/malformed braces — is left
 * verbatim so template prose like "{{user notes here}}" survives and an
 * ungathered variable stays visible. Pure — shared by the custom runner
 * in `features.ts`.
 */
export function renderTemplate(
  template: string,
  vars: Record<string, string>,
): string {
  return template.replace(PLACEHOLDER, (match, name: string) =>
    name in vars ? (vars[name] ?? "") : match,
  );
}

function capTemplate(template: unknown): string {
  return typeof template === "string" ? template.slice(0, CUSTOM_TEMPLATE_CAP) : "";
}

/** Parse-and-drop loader: malformed entries are dropped, ids deduped. */
export function parseCustomPrompts(data: unknown): CustomPromptEntry[] | null {
  if (!Array.isArray(data)) return null;
  const seen = new Set<string>();
  const entries: CustomPromptEntry[] = [];
  for (const raw of data) {
    if (typeof raw !== "object" || raw === null) continue;
    const r = raw as Record<string, unknown>;
    if (
      typeof r.id !== "string" ||
      r.id.length === 0 ||
      typeof r.name !== "string" ||
      r.name.trim().length === 0
    ) {
      continue;
    }
    if (seen.has(r.id)) continue;
    seen.add(r.id);
    entries.push({
      id: r.id,
      name: r.name.trim().slice(0, 120),
      template: capTemplate(r.template),
      enabled: r.enabled === true,
    });
  }
  return entries.slice(0, CUSTOM_PROMPTS_CAP);
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Injectable dependencies (tests). */
export interface CustomPromptStoreDeps {
  /** Storage for the library envelope; `null` disables persistence. */
  storage?: StorageLike | null;
  /** Entry id minting (tests; default counter + random salt). */
  id?: () => string;
}

/** The custom prompt library — construct for tests; the app uses the
 * module-level {@link customPrompts} singleton. */
export class CustomPromptStore {
  /** All entries, in insertion order (deep-reactive). */
  entries = $state<CustomPromptEntry[]>([]);

  readonly #storage: StorageLike | null;
  readonly #mintId: () => string;

  constructor(deps: CustomPromptStoreDeps = {}) {
    this.#storage = deps.storage === undefined ? defaultStorage() : deps.storage;
    this.#mintId =
      deps.id ??
      (() => `cp-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`);
    const loaded = loadVersioned<CustomPromptEntry[]>({
      storage: this.#storage,
      key: CUSTOM_PROMPTS_KEY,
      parse: parseCustomPrompts,
    });
    if (loaded !== null) this.entries = loaded;
    // The registry's query-time merge reads the library live from here.
    registerCustomPromptProvider(() => this.entries);
  }

  /** The entry for `id`, or `undefined` (runners check `enabled` too). */
  get(id: string): CustomPromptEntry | undefined {
    return this.entries.find((entry) => entry.id === id);
  }

  /** Enabled entries in list order (what surfaces as features). */
  enabled(): CustomPromptEntry[] {
    return this.entries.filter((entry) => entry.enabled);
  }

  /** Adds an entry and returns it (empty name → "Custom prompt"). */
  add(init?: { name?: string; template?: string; enabled?: boolean }): CustomPromptEntry {
    if (this.entries.length >= CUSTOM_PROMPTS_CAP) {
      throw new Error(`the custom prompt library is full (${CUSTOM_PROMPTS_CAP} entries)`);
    }
    const entry: CustomPromptEntry = {
      id: this.#mintId(),
      name: (init?.name ?? "Custom prompt").trim().slice(0, 120) || "Custom prompt",
      template: (init?.template ?? "").slice(0, CUSTOM_TEMPLATE_CAP),
      enabled: init?.enabled ?? true,
    };
    this.entries = [...this.entries, entry];
    this.#persist();
    return entry;
  }

  /** Patches one entry (template re-capped) and persists. */
  update(id: string, patch: Partial<Pick<CustomPromptEntry, "name" | "template" | "enabled">>): void {
    let changed = false;
    this.entries = this.entries.map((entry) => {
      if (entry.id !== id) return entry;
      changed = true;
      return {
        ...entry,
        ...(patch.name !== undefined
          ? { name: patch.name.trim().slice(0, 120) || entry.name }
          : null),
        ...(patch.template !== undefined
          ? { template: patch.template.slice(0, CUSTOM_TEMPLATE_CAP) }
          : null),
        ...(patch.enabled !== undefined ? { enabled: patch.enabled } : null),
      };
    });
    if (changed) this.#persist();
  }

  /** Removes one entry (idempotent) and persists. */
  remove(id: string): void {
    const before = this.entries.length;
    this.entries = this.entries.filter((entry) => entry.id !== id);
    if (this.entries.length !== before) this.#persist();
  }

  #persist(): void {
    writeEnvelope(this.#storage, CUSTOM_PROMPTS_KEY, this.entries);
  }
}

/** The app-wide custom prompt library singleton. */
export const customPrompts = new CustomPromptStore();
