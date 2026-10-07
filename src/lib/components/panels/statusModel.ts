/**
 * Pure logic for the working-copy StatusPanel (no runes, no DOM).
 *
 * Everything testable about the panel lives here: the section classifier
 * (one entry can appear in several sections), the path splitter, the
 * change-kind letter/color map, the prebuilt search index and the
 * substring filter that must stay under a frame on 10k entries.
 *
 * Section semantics (mirrors git's two statuses per path):
 *   - Staged:     `index` side is a real change (added/deleted/modified/
 *                 renamed/copied).
 *   - Unstaged:   `worktree` side is a real change.
 *   - Untracked:  `worktree` (or `index`) is untracked.
 *   - Conflicted: either side is conflicted (the engine puts conflicts on
 *                 the index side; the worktree side may also carry a change).
 */

import type { ChangeKind, StatusEntry } from "$lib/ipc/types";

export type SectionId = "conflicted" | "staged" | "unstaged" | "untracked";

/** Display order: trouble first, then index → worktree → new files. */
export const SECTION_ORDER: readonly SectionId[] = [
  "conflicted",
  "staged",
  "unstaged",
  "untracked",
];

export const SECTION_LABELS: Record<SectionId, string> = {
  conflicted: "Conflicted",
  staged: "Staged",
  unstaged: "Unstaged",
  untracked: "Untracked",
};

/** Kinds that count as a change on a status side (drives Staged/Unstaged). */
const CHANGE_KINDS: ReadonlySet<ChangeKind> = new Set([
  "added",
  "deleted",
  "modified",
  "renamed",
  "copied",
]);

/** One rendered row: an entry placed into exactly one section. */
export interface StatusRow {
  /** `${section}:${path}` — unique even when a path is in two sections. */
  key: string;
  section: SectionId;
  entry: StatusEntry;
  /** The side's kind that put this entry into this section. */
  kind: ChangeKind;
  /** Directory portion of the path ("" when the file is at the root). */
  dir: string;
  /** Final path segment (the bolded basename). */
  base: string;
  /** Lowercased `path` (+ `old_path`) — the prebuilt search index. */
  search: string;
}

/** Search index built once per status snapshot (not per keystroke). */
export interface StatusIndex {
  /** All rows in section order. */
  rows: StatusRow[];
  bySection: Record<SectionId, StatusRow[]>;
  /** Total rows across sections (an entry in two sections counts twice). */
  total: number;
}

/** A section ready to render (already filtered). */
export interface StatusSectionView {
  id: SectionId;
  label: string;
  rows: StatusRow[];
}

/**
 * Classifies one entry into its sections, in SECTION_ORDER order.
 * `unmodified`/`ignored` sides never produce a section.
 */
export function classifyEntry(entry: StatusEntry): SectionId[] {
  const sections: SectionId[] = [];
  const { index, worktree } = entry;
  if (index === "conflicted" || worktree === "conflicted") {
    sections.push("conflicted");
  }
  if (CHANGE_KINDS.has(index)) sections.push("staged");
  if (CHANGE_KINDS.has(worktree)) sections.push("unstaged");
  if (index === "untracked" || worktree === "untracked") {
    sections.push("untracked");
  }
  return sections;
}

/** Splits a repo path into `{dir, base}` on `/` or `\` (dir "" at root). */
export function splitPath(path: string): { dir: string; base: string } {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (cut === -1) return { dir: "", base: path };
  return { dir: path.slice(0, cut + 1), base: path.slice(cut + 1) };
}

/** One-character change-kind letter (A/M/D/R/C/U, `!` for conflicts). */
export function kindLetter(kind: ChangeKind): string {
  switch (kind) {
    case "added":
      return "A";
    case "deleted":
      return "D";
    case "modified":
      return "M";
    case "renamed":
      return "R";
    case "copied":
      return "C";
    case "untracked":
      return "U";
    case "conflicted":
      return "!";
    default:
      return "·";
  }
}

/** CSS modifier class for a change kind (colors live in StatusPanel.svelte). */
export function kindClass(kind: ChangeKind): string {
  switch (kind) {
    case "added":
      return "k-added";
    case "deleted":
      return "k-deleted";
    case "renamed":
      return "k-renamed";
    case "copied":
      return "k-renamed";
    case "untracked":
      return "k-untracked";
    case "conflicted":
      return "k-conflicted";
    default:
      return "k-modified";
  }
}

/** Side kind that represents `entry` inside `section` (letter/color source). */
function rowKind(entry: StatusEntry, section: SectionId): ChangeKind {
  switch (section) {
    case "staged":
      return entry.index;
    case "conflicted":
      return "conflicted";
    default:
      return entry.worktree;
  }
}

/**
 * Builds the per-status search index: one row per (entry, section) pair,
 * precomputing lowercase search text so filtering is a substring scan.
 */
export function buildStatusIndex(entries: readonly StatusEntry[]): StatusIndex {
  const bySection: Record<SectionId, StatusRow[]> = {
    conflicted: [],
    staged: [],
    unstaged: [],
    untracked: [],
  };
  let total = 0;
  for (const entry of entries) {
    const { dir, base } = splitPath(entry.path);
    const search = (
      entry.old_path ? `${entry.path} ${entry.old_path}` : entry.path
    ).toLowerCase();
    for (const section of classifyEntry(entry)) {
      bySection[section].push({
        key: `${section}:${entry.path}`,
        section,
        entry,
        kind: rowKind(entry, section),
        dir,
        base,
        search,
      });
      total += 1;
    }
  }
  const rows: StatusRow[] = [];
  for (const section of SECTION_ORDER) rows.push(...bySection[section]);
  return { rows, bySection, total };
}

/**
 * Filters the index to renderable sections. Case-insensitive substring over
 * the prebuilt lowercase index (path + old_path); an empty needle returns
 * everything. Single pass, no allocations beyond the result rows.
 */
export function filterSections(
  index: StatusIndex,
  needle: string,
): StatusSectionView[] {
  const q = needle.trim().toLowerCase();
  const out: StatusSectionView[] = [];
  for (const id of SECTION_ORDER) {
    const all = index.bySection[id];
    if (all.length === 0) continue;
    const rows =
      q === ""
        ? all
        : all.filter((row) => row.search.indexOf(q) !== -1);
    if (rows.length === 0) continue;
    out.push({ id, label: SECTION_LABELS[id], rows });
  }
  return out;
}
