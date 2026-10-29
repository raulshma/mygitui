/**
 * Selection algebra for the StatusPanel rows (pure: no runes, no DOM).
 *
 * VS Code source-control semantics. Selection identity is the row key
 * (`section:path`), not the path: one path can appear in Staged Changes
 * and Changes at the same time, and those are distinct resources that
 * select independently. Multi-select rules mirror the VS Code list:
 * ctrl-click / Space toggle membership, shift-click and shift-arrows
 * extend a range from the anchor, ctrl+a selects everything, and any
 * row-targeted action (stage / unstage / discard) applies to the whole
 * selection when its target row is part of the selection — otherwise to
 * that row alone.
 */

import type { SectionId, StatusRow } from "./statusModel";

/** Adds `key` to the selection, or removes it when already present. */
export function toggleSelection(
  selected: readonly string[],
  key: string,
): string[] {
  return selected.includes(key)
    ? selected.filter((k) => k !== key)
    : [...selected, key];
}

/** Contiguous range between two visible-row indexes, as row keys. */
export function rangeSelection(
  rows: readonly StatusRow[],
  from: number,
  to: number,
): string[] {
  const [lo, hi] = from <= to ? [from, to] : [to, from];
  const out: string[] = [];
  for (let i = lo; i <= hi && i < rows.length; i++) out.push(rows[i]!.key);
  return out;
}

/** Every visible row key (ctrl+a). */
export function allSelection(rows: readonly StatusRow[]): string[] {
  return rows.map((row) => row.key);
}

/** Drops keys whose rows no longer exist in `rows`. */
export function pruneSelection(
  selected: readonly string[],
  rows: readonly StatusRow[],
): string[] {
  const live = new Set(rows.map((row) => row.key));
  return selected.filter((key) => live.has(key));
}

/** Parses a status row key into its optional repo/submodule prefix, section, and path. */
export function parseRowKey(
  key: string,
): { prefix: string; section: SectionId; path: string } | null {
  const stagedMarker = ":staged:";
  const changesMarker = ":changes:";
  const stagedIdx = key.indexOf(stagedMarker);
  if (stagedIdx !== -1) {
    return {
      prefix: key.slice(0, stagedIdx),
      section: "staged",
      path: key.slice(stagedIdx + stagedMarker.length),
    };
  }
  const changesIdx = key.indexOf(changesMarker);
  if (changesIdx !== -1) {
    return {
      prefix: key.slice(0, changesIdx),
      section: "changes",
      path: key.slice(changesIdx + changesMarker.length),
    };
  }
  if (key.startsWith("staged:")) {
    return { prefix: "", section: "staged", path: key.slice(7) };
  }
  if (key.startsWith("changes:")) {
    return { prefix: "", section: "changes", path: key.slice(8) };
  }
  return null;
}

/**
 * Selection after a stage/unstage of `paths` lands. A mutated row moves
 * between the groups (staging moves Changes → Staged Changes, unstaging
 * the other way), and VS Code keeps such files selected across the move:
 * the key's section flips when the counterpart row exists in the
 * refreshed list. When neither side exists yet (refresh in flight) the
 * old key is kept so a later prune can decide; keys outside `paths` are
 * kept only while still live.
 */
export function remapSelection(
  selected: readonly string[],
  paths: ReadonlySet<string>,
  unstage: boolean,
  rows: readonly StatusRow[],
  prefix?: string,
): string[] {
  const live = new Set(rows.map((row) => row.key));
  const out: string[] = [];
  for (const key of selected) {
    const parsed = parseRowKey(key);
    const keyPrefix = parsed?.prefix ?? "";
    if (prefix !== undefined && keyPrefix !== prefix) {
      if (live.has(key)) out.push(key);
      continue;
    }
    const path = parsed ? parsed.path : key.slice(key.indexOf(":") + 1);
    if (paths.has(path)) {
      const nextSection: SectionId = unstage ? "changes" : "staged";
      const moved = keyPrefix
        ? `${keyPrefix}:${nextSection}:${path}`
        : `${nextSection}:${path}`;
      out.push(live.has(moved) ? moved : key);
    } else if (live.has(key)) {
      out.push(key);
    }
  }
  return out;
}

/**
 * The rows a row-targeted action applies to: the whole selection when the
 * target row is part of it (VS Code lets hover buttons, `s`/`d` and the
 * context menu act on every selected file), otherwise just the target.
 */
export function actionRows(
  target: StatusRow,
  selected: readonly string[],
  rows: readonly StatusRow[],
): StatusRow[] {
  if (!selected.includes(target.key)) return [target];
  const selectedSet = new Set(selected);
  return rows.filter((row) => selectedSet.has(row.key));
}

/** Paths split by what may happen to them (conflicted rows never qualify). */
export interface SelectionActions {
  /** Changes-side paths to add to the index. */
  stage: string[];
  /** Index-side paths to unstage. */
  unstage: string[];
  /** Changes-side paths safe to discard. */
  discard: string[];
}

/**
 * Splits rows into stage / unstage / discard path sets. Conflicted rows
 * are excluded: resolving a conflict is an explicit edit, not a bulk
 * stage or discard.
 */
export function selectionActions(rows: readonly StatusRow[]): SelectionActions {
  const actions: SelectionActions = { stage: [], unstage: [], discard: [] };
  for (const row of rows) {
    if (row.kind === "conflicted") continue;
    if (row.section === "staged") {
      actions.unstage.push(row.entry.path);
    } else {
      actions.stage.push(row.entry.path);
      actions.discard.push(row.entry.path);
    }
  }
  return actions;
}

/** Count-aware menu label: "Stage" vs "Stage 3 files". */
export function countLabel(verb: string, count: number): string {
  return count > 1 ? `${verb} ${count} files` : verb;
}
