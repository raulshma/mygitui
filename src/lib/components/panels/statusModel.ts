/**
 * Pure logic for the working-copy StatusPanel (no runes, no DOM).
 *
 * Everything testable about the panel lives here: the section classifier
 * (one entry can appear in Staged Changes and Changes), the path splitter, the
 * change-kind letter/color map, the prebuilt search index and the
 * substring filter that must stay under a frame on 10k entries.
 *
 * Section semantics (mirrors VS Code's two source-control groups):
 *   - Staged Changes: `index` side is a real change.
 *   - Changes: `worktree` side changed, the path is untracked, or either
 *              status side is conflicted.
 */

import type { ChangeKind, RepoStatus, StatusEntry, SubmoduleInfo } from "$lib/ipc/types";

export type SectionId = "staged" | "changes";

/** VS Code displays the index group above the working tree group. */
export const SECTION_ORDER: readonly SectionId[] = ["staged", "changes"];

/** VSCode source-control wording. */
export const SECTION_LABELS: Record<SectionId, string> = {
  staged: "Staged Changes",
  changes: "Changes",
};

/** Kinds that count as a change on a status side (drives both groups). */
const CHANGE_KINDS: ReadonlySet<ChangeKind> = new Set([
  "added",
  "deleted",
  "modified",
  "renamed",
  "copied",
]);

/** One rendered row: an entry placed into one or both status sections. */
export interface StatusRow {
  /** `${section}:${path}` or `${prefix}:${section}:${path}` — unique. */
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
  /** Submodule path if this row belongs to a submodule. */
  submodulePath?: string;
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
  if (CHANGE_KINDS.has(index)) sections.push("staged");
  if (
    index === "conflicted" ||
    worktree === "conflicted" ||
    CHANGE_KINDS.has(worktree) ||
    index === "untracked" ||
    worktree === "untracked"
  ) {
    sections.push("changes");
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

/** Human-readable change-kind label (row tooltips / screen readers). */
export function kindLabel(kind: ChangeKind): string {
  switch (kind) {
    case "added":
      return "Added";
    case "deleted":
      return "Deleted";
    case "modified":
      return "Modified";
    case "renamed":
      return "Renamed";
    case "copied":
      return "Copied";
    case "untracked":
      return "Untracked";
    case "conflicted":
      return "Conflicted";
    default:
      return "Unmodified";
  }
}

/** Minimal submodule shape the compact display helpers need. */
export interface SubmoduleState {
  initialized: boolean;
  status: string;
}

/** How a submodule should read inside a dense source-control row. */
export interface SubmoduleDisplay {
  /** Compact tag ("" when clean). */
  tag: string;
  /** Full sentence for tooltips / the submodule panel. */
  detail: string;
  tone: "clean" | "dirty" | "danger";
}

/** Short, VSCode-like wording for git's submodule status string. */
function shortenSubmoduleStatus(status: string): string {
  return status
    .split(" / ")
    .map((part) => {
      switch (part) {
        case "modified content":
          return "modified";
        case "untracked content":
          return "untracked";
        default:
          return part;
      }
    })
    .join(" · ");
}

/**
 * Maps a `SubmoduleInfo` to compact display text. Uninitialized modules are
 * reported before `status`: `submodules_impl` cannot open them, so its
 * "broken" marker would otherwise read as an error instead of "not
 * initialized".
 */
export function submoduleDisplay(info: SubmoduleState): SubmoduleDisplay {
  if (!info.initialized) {
    return {
      tag: "not initialized",
      detail: "Submodule is not initialized",
      tone: "danger",
    };
  }
  const status = info.status.trim();
  if (status === "broken") {
    return { tag: "broken", detail: "Submodule cannot be opened", tone: "danger" };
  }
  if (status === "") {
    return { tag: "", detail: "Up to date", tone: "clean" };
  }
  return {
    tag: shortenSubmoduleStatus(status),
    detail: status,
    tone: "dirty",
  };
}

/** Whether a submodule needs a row in the source-control list. */
export function submoduleNeedsAttention(info: SubmoduleState): boolean {
  return !info.initialized || info.status.trim() !== "";
}

/** Summarizes change counts for a repo/submodule. */
export function submoduleChangeCounts(status: RepoStatus | null): {
  staged: number;
  changes: number;
  total: number;
} {
  if (!status) return { staged: 0, changes: 0, total: 0 };
  let staged = 0;
  let changes = 0;
  for (const entry of status.entries) {
    const sections = classifyEntry(entry);
    if (sections.includes("staged")) staged += 1;
    if (sections.includes("changes")) changes += 1;
  }
  return { staged, changes, total: staged + changes };
}

/** Computes a human-readable branch/head label for a submodule. */
export function submoduleBranchLabel(
  info: SubmoduleInfo,
  status: RepoStatus | null,
): string {
  if (!info.initialized) return "not initialized";
  if (status?.branch) return status.branch;
  if (status?.detached) {
    const sha = status.head ?? info.head_sha ?? info.recorded_sha;
    return sha ? `detached at ${sha.slice(0, 7)}` : "detached";
  }
  if (status?.head) return status.head.slice(0, 7);
  if (info.head_sha) return info.head_sha.slice(0, 7);
  if (info.recorded_sha) return info.recorded_sha.slice(0, 7);
  return "clean";
}

/** Side kind that represents `entry` inside `section` (letter/color source). */
function rowKind(entry: StatusEntry, section: SectionId): ChangeKind {
  if (section === "staged") return entry.index;
  if (entry.index === "conflicted" || entry.worktree === "conflicted") {
    return "conflicted";
  }
  if (entry.worktree !== "unmodified" && entry.worktree !== "ignored") {
    return entry.worktree;
  }
  return entry.index === "untracked" ? "untracked" : entry.worktree;
}

/**
 * Builds the per-status search index: one row per (entry, section) pair,
 * precomputing lowercase search text so filtering is a substring scan.
 */
export function buildStatusIndex(
  entries: readonly StatusEntry[],
  keyPrefix?: string,
  submodulePath?: string,
): StatusIndex {
  const bySection: Record<SectionId, StatusRow[]> = {
    staged: [],
    changes: [],
  };
  let total = 0;
  for (const entry of entries) {
    const { dir, base } = splitPath(entry.path);
    const search = (
      entry.old_path ? `${entry.path} ${entry.old_path}` : entry.path
    ).toLowerCase();
    for (const section of classifyEntry(entry)) {
      const key = keyPrefix
        ? `${keyPrefix}:${section}:${entry.path}`
        : `${section}:${entry.path}`;
      bySection[section].push({
        key,
        section,
        entry,
        kind: rowKind(entry, section),
        dir,
        base,
        search,
        submodulePath,
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
