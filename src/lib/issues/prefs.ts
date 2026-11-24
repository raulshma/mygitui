/** Parse-validated, root-keyed Issues view preferences. */

import { clampRatio } from "$lib/layout/layoutModel";
import { loadVersioned, writeEnvelope, type StorageLike } from "$lib/state/persist";
import type { IssueQuery } from "./model";
import { DEFAULT_ISSUE_QUERY, normalizeIssueQuery } from "./query";

export const ISSUE_VIEW_PREFS_PREFIX = "mygitui.issues.view.";
export const DEFAULT_ISSUE_SPLIT_RATIO = 1 / 3;

export interface IssueViewPreferences {
  query: IssueQuery;
  splitRatio: number;
}

export function defaultIssueViewPreferences(): IssueViewPreferences {
  return {
    query: normalizeIssueQuery(DEFAULT_ISSUE_QUERY),
    splitRatio: DEFAULT_ISSUE_SPLIT_RATIO,
  };
}

/** Stable across slash spelling, trailing separators, and platform case. */
export function normalizeIssueRoot(root: string): string {
  if (root === "") return "";
  const normalized = root.replace(/[\\/]+/g, "/").toLowerCase().replace(/\/+$/, "");
  return normalized === "" ? "/" : normalized;
}

export function issueViewPrefsKey(root: string): string {
  return ISSUE_VIEW_PREFS_PREFIX + normalizeIssueRoot(root);
}

/** Invalid fields fall back individually; a non-object payload is dropped. */
export function parseIssueViewPreferences(data: unknown): IssueViewPreferences | null {
  if (typeof data !== "object" || data === null || Array.isArray(data)) return null;
  const value = data as Record<string, unknown>;
  const rawQuery =
    typeof value.query === "object" && value.query !== null && !Array.isArray(value.query)
      ? (value.query as Partial<IssueQuery>)
      : {};
  const splitRatio =
    typeof value.splitRatio === "number" && Number.isFinite(value.splitRatio)
      ? clampRatio(value.splitRatio)
      : DEFAULT_ISSUE_SPLIT_RATIO;
  return { query: normalizeIssueQuery(rawQuery), splitRatio };
}

function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export function loadIssueViewPreferences(
  root: string,
  storage: StorageLike | null = defaultStorage(),
): IssueViewPreferences {
  const key = normalizeIssueRoot(root);
  if (key === "") return defaultIssueViewPreferences();
  return (
    loadVersioned<IssueViewPreferences>({
      storage,
      key: issueViewPrefsKey(key),
      parse: parseIssueViewPreferences,
    }) ?? defaultIssueViewPreferences()
  );
}

export function saveIssueViewPreferences(
  root: string,
  preferences: IssueViewPreferences,
  storage: StorageLike | null = defaultStorage(),
): void {
  const key = normalizeIssueRoot(root);
  if (key === "") return;
  const parsed = parseIssueViewPreferences(preferences);
  if (parsed !== null) writeEnvelope(storage, issueViewPrefsKey(key), parsed);
}

/** Session cache avoids re-reading storage while controls update frequently. */
export class IssueViewPrefsStore {
  #storage: StorageLike | null;
  #cache = new Map<string, IssueViewPreferences>();

  constructor(storage: StorageLike | null = defaultStorage()) {
    this.#storage = storage;
  }

  read(root: string): IssueViewPreferences {
    const key = normalizeIssueRoot(root);
    const cached = this.#cache.get(key);
    if (cached !== undefined) return cached;
    const loaded = loadIssueViewPreferences(key, this.#storage);
    this.#cache.set(key, loaded);
    return loaded;
  }

  setQuery(root: string, patch: Partial<IssueQuery>): IssueQuery {
    const current = this.read(root);
    const query = normalizeIssueQuery({ ...current.query, ...patch });
    this.#write(root, { ...current, query });
    return query;
  }

  setSplitRatio(root: string, ratio: number): number {
    const current = this.read(root);
    const splitRatio = clampRatio(ratio);
    this.#write(root, { ...current, splitRatio });
    return splitRatio;
  }

  reset(root: string): void {
    this.#write(root, defaultIssueViewPreferences());
  }

  /** In-memory cleanup only; durable view preferences intentionally survive. */
  evict(root: string): void {
    this.#cache.delete(normalizeIssueRoot(root));
  }

  #write(root: string, preferences: IssueViewPreferences): void {
    const parsed = parseIssueViewPreferences(preferences);
    if (parsed === null) return;
    const key = normalizeIssueRoot(root);
    this.#cache.set(key, parsed);
    saveIssueViewPreferences(key, parsed, this.#storage);
  }
}

export const issueViewPrefs = new IssueViewPrefsStore();
