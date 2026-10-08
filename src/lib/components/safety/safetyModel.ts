/**
 * Pure model for the M3 safety/undo UI (no Svelte, no IPC — fully tested).
 *
 * Everything the safety dialogs and the undo panel render is computed here:
 * relative ages, checkpoint titles, danger classification, preview text
 * lines and the parameter shapes the backend preview/reset commands expect.
 */

import type {
  CheckpointInfo,
  PreviewFile,
  PreviewInfo,
  ResetKind,
} from "$lib/ipc/types";

/** Max file/commit rows shown from a preview before truncation. */
export const PREVIEW_FILE_CAP = 50;

// ---------------------------------------------------------------------------
// Ages + checkpoint titles
// ---------------------------------------------------------------------------

/**
 * Human relative age of a `created_at` unix-seconds timestamp:
 * "just now", "5m ago", "3h ago", "2d ago", "4mo ago", "1y ago".
 * `now` (unix seconds) is injectable for deterministic tests.
 */
export function formatRelativeAge(createdAt: number, now: number): string {
  const diff = Math.max(0, Math.floor(now) - createdAt);
  if (diff < 60) return "just now";
  const minutes = Math.floor(diff / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 60) return `${days}d ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

/**
 * Display title for a checkpoint: `<reason> · <relative age> · <branch>`.
 * Detached checkpoints (no branch) show `detached`.
 */
export function checkpointTitle(cp: CheckpointInfo, now: number): string {
  const branch = cp.branch && cp.branch !== "" ? cp.branch : "detached";
  return `${cp.reason} · ${formatRelativeAge(cp.created_at, now)} · ${branch}`;
}

/**
 * Sorts checkpoints newest-first (UndoPanel order). Pure: returns a new array.
 */
export function sortNewestFirst(cps: CheckpointInfo[]): CheckpointInfo[] {
  return [...cps].sort((a, b) => b.created_at - a.created_at);
}

// ---------------------------------------------------------------------------
// Danger classification
// ---------------------------------------------------------------------------

export type DangerLevel = "warn" | "danger";

/** How a dangerous op is presented: strength + wording. */
export interface DangerInfo {
  /** `warn` = confirm; `danger` = guard checkpoint + stronger confirmation. */
  level: DangerLevel;
  /** Button/heading verb, e.g. "Hard reset". */
  verb: string;
  /** Lower-case noun for sentences, e.g. "hard reset". */
  noun: string;
}

const DANGER_BY_KIND: Record<string, DangerInfo> = {
  reset_hard: { level: "danger", verb: "Hard reset", noun: "hard reset" },
  clean: { level: "danger", verb: "Clean", noun: "clean" },
  checkout_force: {
    level: "danger",
    verb: "Force checkout",
    noun: "force checkout",
  },
};

/**
 * Classifies a preview kind for the safety UI:
 * - `reset_hard` / `clean` / `checkout_force` → danger;
 * - `branch_delete` of a **merged** branch → warn (safe delete);
 * - `branch_delete` of an **unmerged** branch → danger (force delete);
 * - unknown kinds fail loud (danger).
 */
export function classifyDanger(
  kind: string,
  opts?: { merged?: boolean },
): DangerInfo {
  if (kind === "branch_delete") {
    return opts?.merged
      ? { level: "warn", verb: "Delete", noun: "branch delete" }
      : { level: "danger", verb: "Force delete", noun: "force branch delete" };
  }
  return DANGER_BY_KIND[kind] ?? { level: "danger", verb: kind, noun: kind };
}

// ---------------------------------------------------------------------------
// Preview text
// ---------------------------------------------------------------------------

/** One rendered row for a preview file entry. */
export function formatPreviewFile(file: PreviewFile): string {
  return file.change ? `${file.path} — ${file.change}` : file.path;
}

/**
 * The dialog's text lines for a preview: the summary first, then up to
 * {@link PREVIEW_FILE_CAP} formatted file rows; when truncated, a final
 * `and N more` line reports the remainder.
 */
export function previewSummaryLines(info: PreviewInfo): string[] {
  const lines = info.summary ? [info.summary] : [];
  const files = info.files ?? [];
  for (const file of files.slice(0, PREVIEW_FILE_CAP)) {
    lines.push(formatPreviewFile(file));
  }
  if (files.length > PREVIEW_FILE_CAP) {
    lines.push(`and ${files.length - PREVIEW_FILE_CAP} more`);
  }
  return lines;
}

/**
 * True when a `branch_delete` preview reports the branch as fully merged
 * (backend returns an empty file list and a `merged into …` summary).
 */
export function isMergedPreview(info: PreviewInfo): boolean {
  return (info.files ?? []).length === 0 && info.summary.startsWith("merged");
}

// ---------------------------------------------------------------------------
// Reset params
// ---------------------------------------------------------------------------

/**
 * Payload for `reset` (and, without `kind`, for the `reset_hard` preview):
 * `{ kind, to }` — snake_case values only, passed through verbatim.
 */
export function buildResetParams(
  kind: ResetKind,
  to: string,
): { kind: ResetKind; to: string } {
  return { kind, to };
}

/** Preview kind for reset previews (the backend models all resets as tree switches). */
export const RESET_PREVIEW_KIND = "reset_hard";

/** `ops_preview` params for a reset preview: `{ to }`. */
export function buildResetPreviewParams(to: string): Record<string, unknown> {
  return { to };
}

/** Radio options for the reset dialog (plain-language descriptions). */
export const RESET_MODES: Array<{
  kind: ResetKind;
  label: string;
  description: string;
}> = [
  {
    kind: "soft",
    label: "Soft",
    description: "Move the branch only — everything stays staged.",
  },
  {
    kind: "mixed",
    label: "Mixed",
    description:
      "Move the branch and unstage — changes stay in the working copy.",
  },
  {
    kind: "hard",
    label: "Hard",
    description: "Move the branch and discard ALL uncommitted changes.",
  },
];
