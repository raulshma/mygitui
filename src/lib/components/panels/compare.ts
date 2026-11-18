/**
 * Shared compare-selection shape for the history views: what CompareBar
 * hands back and what HistoryView / FileHistoryView hold as `compare`
 * state and pass into CommitDetail's `compare` prop.
 */
import type { DiffSide, FileDiff } from "$lib/ipc/types";

export interface CompareResult {
  /** Aggregate diff files across the selection. */
  files: FileDiff[];
  /** Base side sha (a range start carries the `^` parent marker). */
  base: string;
  /** Target side sha (the newest selected commit; a label for worktree). */
  target: string;
  /**
   * The selected range's commits (summary + sha, newest first) when the
   * compare came from a row multi-selection — the release-notes feature's
   * input. Absent for plain two-ref compares (typed, not selected), where
   * the AI button therefore does not render.
   */
  commits?: Array<{ summary: string; sha: string }>;
  /**
   * The exact diff sides the compare ran with, when it came from
   * CompareBar (three-dot bases and worktree/index/head target sides are
   * DiffSide objects, not plain shas). Image-diff loaders use these;
   * absent = fall back to `{ commit: base } / { commit: target }`.
   */
  sides?: { old: DiffSide; new: DiffSide };
}

/** How the two refs compare: direct range (`A..B`) vs merge-base (`A...B`). */
export type CompareMode = "twodot" | "threedot";

/** What the target (B) side resolves to when the compare runs. */
export type CompareTargetKind = "commit" | "worktree" | "index" | "head";

/** DiffSide for a target kind ("commit" carries the typed ref instead). */
export function targetKindToSide(kind: CompareTargetKind, ref: string): DiffSide {
  return kind === "commit" ? { commit: ref } : kind;
}

/**
 * Persisted per-repo compare-bar choices (mode + target side kind), keyed
 * by repo root like the other per-feature prefs — repo ids are per-session,
 * so cross-launch state keys on the worktree root
 * (`mygitui.compare.<root>`). Small enough to write through on change (no
 * debounce) with the usual swallow-all storage access convention (privacy
 * modes / SSR / tests may not have localStorage).
 */
export const COMPARE_PREFS_PREFIX = "mygitui.compare.";

/** What gets persisted per repo root. */
export interface ComparePrefs {
  mode: CompareMode;
  targetKind: CompareTargetKind;
}

/** The defaults every malformed/absent payload falls back to. */
export const DEFAULT_COMPARE_PREFS: ComparePrefs = {
  mode: "twodot",
  targetKind: "commit",
};

const MODES: readonly CompareMode[] = ["twodot", "threedot"];
const TARGET_KINDS: readonly CompareTargetKind[] = [
  "commit",
  "worktree",
  "index",
  "head",
];

/** Parses persisted prefs JSON, defaulting malformed fields (never throws). */
export function parseComparePrefs(raw: string | null): ComparePrefs {
  if (!raw) return { ...DEFAULT_COMPARE_PREFS };
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { ...DEFAULT_COMPARE_PREFS };
  }
  if (typeof parsed !== "object" || parsed === null) {
    return { ...DEFAULT_COMPARE_PREFS };
  }
  const v = parsed as Record<string, unknown>;
  return {
    mode: MODES.includes(v.mode as CompareMode) ? (v.mode as CompareMode) : "twodot",
    targetKind: TARGET_KINDS.includes(v.targetKind as CompareTargetKind)
      ? (v.targetKind as CompareTargetKind)
      : "commit",
  };
}

/** Reads the persisted compare prefs for a repo root (defaults when none). */
export function loadComparePrefs(root: string): ComparePrefs {
  if (root === "") return { ...DEFAULT_COMPARE_PREFS };
  try {
    return parseComparePrefs(
      globalThis.localStorage?.getItem(COMPARE_PREFS_PREFIX + root) ?? null,
    );
  } catch {
    return { ...DEFAULT_COMPARE_PREFS };
  }
}

/** Records the compare prefs for a root (write-through; errors swallowed). */
export function saveComparePrefs(root: string, prefs: ComparePrefs): void {
  if (root === "") return;
  try {
    globalThis.localStorage?.setItem(
      COMPARE_PREFS_PREFIX + root,
      JSON.stringify(prefs),
    );
  } catch {
    // storage unavailable (privacy mode / tests)
  }
}
