/**
 * Branch color rules (Svelte 5 runes) — M7 lane I1.
 *
 * Per-repo map of branch-name pattern → color hint, persisted FE-side (per
 * contracts.md) under localStorage `mygitui.branchcolors.<root>` as
 * `[{ pattern, color }]`. The StatsPanel hosts the editor ("Colors"
 * popover); the commit graph consumes `branchColorForRefs(refs, rules)` —
 * when a row's ref decorations match a rule, its node + edges render in
 * that color instead of the lane palette (first matching rule wins).
 *
 * Pattern grammar (glob-ish, deliberately tiny):
 *   `feature/*`  prefix match (trailing `*`)
 *   `*-hotfix`   suffix match (leading `*`)
 *   `*`          matches everything
 *   `main`       exact match
 * Patterns apply to SHORT ref names ("main", "origin/main"); HEAD
 * decorations never match. Matching is case-sensitive and deterministic.
 *
 * Storage keys off the repo ROOT (like the layout overlays); hydration is
 * lazy but must happen OUTSIDE derived reads: call `ensure(root)` from an
 * `$effect`, mirroring the layouts store contract.
 */

import { classifyRef, shortRefName } from "$lib/stores/history-logic";

/** localStorage key for one repo root. */
export function branchColorsKey(root: string): string {
  return `mygitui.branchcolors.${root}`;
}

/** One rule: branch-name pattern → CSS color hint. */
export interface BranchColorRule {
  /** Glob-ish pattern (see module docs). */
  pattern: string;
  /** Any non-blank CSS color string (free-form hint). */
  color: string;
}

/** Minimal storage surface this store needs (subset of DOM `Storage`). */
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

/** True when `name` (short ref name) satisfies the glob-ish `pattern`. */
export function matchesBranchPattern(name: string, pattern: string): boolean {
  const p = pattern.trim();
  const n = name.trim();
  if (!p || !n) return false;
  if (p === "*") return true;
  if (p.startsWith("*")) return n.endsWith(p.slice(1));
  if (p.endsWith("*")) return n.startsWith(p.slice(0, -1));
  return n === p;
}

/**
 * Color for a commit's ref decorations: the first rule whose pattern
 * matches a branch-ish (local/remote) short ref name wins; `null` when no
 * rule matches (the row keeps its lane palette color).
 */
export function branchColorForRefs(
  refs: readonly string[],
  rules: readonly BranchColorRule[],
): string | null {
  for (const rule of rules) {
    if (!rule.color.trim()) continue;
    for (const ref of refs) {
      // HEAD decorations are pointers, not branches; tags keep their own
      // chip identity and are not recolored.
      if (classifyRef(ref) === "head") continue;
      if (matchesBranchPattern(shortRefName(ref), rule.pattern)) {
        return rule.color;
      }
    }
  }
  return null;
}

/** Parses persisted rules; drops malformed/blank entries. */
function parseRules(raw: string | null): BranchColorRule[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const out: BranchColorRule[] = [];
  for (const item of parsed) {
    const r = item as Partial<BranchColorRule> | null;
    if (
      typeof r?.pattern === "string" &&
      r.pattern.trim() !== "" &&
      typeof r?.color === "string" &&
      r.color.trim() !== ""
    ) {
      out.push({ pattern: r.pattern, color: r.color });
    }
  }
  return out;
}

export class BranchColorStore {
  #byRoot: Record<string, BranchColorRule[]> = $state({});
  readonly #storage: StorageLike | null;
  #hydrated = new Set<string>();

  constructor(storage: StorageLike | null = defaultStorage()) {
    this.#storage = storage;
  }

  /** Hydrates `root`'s persisted rules (idempotent; call from an `$effect`). */
  ensure(root: string): void {
    if (this.#hydrated.has(root)) return;
    this.#hydrated.add(root);
    this.#byRoot[root] = parseRules(
      this.#storage?.getItem(branchColorsKey(root)) ?? null,
    );
  }

  /** Rules of `root`, array order = match priority (empty when unknown). */
  rules(root: string): BranchColorRule[] {
    return this.#byRoot[root] ?? [];
  }

  /** Appends a rule (both fields required; duplicate patterns rejected). */
  addRule(root: string, pattern: string, color: string): BranchColorRule | null {
    const p = pattern.trim();
    const c = color.trim();
    if (!p || !c) return null;
    if (this.rules(root).some((r) => r.pattern === p)) return null;
    const rule: BranchColorRule = { pattern: p, color: c };
    this.#setRules(root, [...this.rules(root), rule]);
    return rule;
  }

  /** Patches a rule by index (trimmed; blank fields keep the old value). */
  updateRule(
    root: string,
    index: number,
    patch: Partial<Omit<BranchColorRule, never>>,
  ): void {
    const list = this.rules(root);
    if (index < 0 || index >= list.length) return;
    const next = list.map((rule, i) =>
      i === index
        ? {
            pattern: patch.pattern?.trim() ? patch.pattern.trim() : rule.pattern,
            color: patch.color?.trim() ? patch.color.trim() : rule.color,
          }
        : rule,
    );
    this.#setRules(root, next);
  }

  /** Removes the rule at `index`; true when something was removed. */
  removeRule(root: string, index: number): boolean {
    const list = this.rules(root);
    if (index < 0 || index >= list.length) return false;
    this.#setRules(root, list.filter((_, i) => i !== index));
    return true;
  }

  /** Drops all state for `root` (repo closed; test helper). */
  clear(root: string): void {
    this.#hydrated.delete(root);
    this.#setRules(root, []);
  }

  #setRules(root: string, rules: BranchColorRule[]): void {
    this.#byRoot[root] = rules;
    this.#storage?.setItem(branchColorsKey(root), JSON.stringify(rules));
  }
}

/** The application-wide branch-color rule store. */
export const branchColorStore = new BranchColorStore();
