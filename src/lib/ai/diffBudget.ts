/**
 * File-aware diff budgeting for AI prompts.
 *
 * The naive approach — render the whole staged diff to one string and cut
 * at {@link MAX_DIFF_CHARS} — drops every file after the cap without the
 * model ever knowing they existed, and lets a single lockfile eat the
 * entire budget. This module instead renders an always-complete stat
 * summary first, then spends the remaining character budget across files:
 * every eligible file gets a fair slice (capped per file), oversized
 * slices end with an explicit "[+N more changed lines in this file]"
 * note, generated/lockfile content is excluded (its stat line remains),
 * and files that receive no content are listed by path so the model can
 * still reason about the whole change.
 *
 * All functions are PURE (no IO) so they compose with the prompt builders
 * in `./features` and stay trivially testable.
 */

import type { DiffHunk, FileDiff } from "$lib/ipc/types";

/**
 * Hard cap for the whole rendered text, sized for a ~128k-token LLM
 * context: ~400k chars ≈ 100k+ tokens of diff at a typical 3.5–4
 * chars/token, leaving room for the system prompt and the response.
 */
export const MAX_DIFF_CHARS = 400_000;

/** Max chars of diff content for any single file. */
export const PER_FILE_CAP = 32_000;

/** Smallest useful per-file slice (file headers + hunk header + a few lines). */
export const MIN_FILE_SHARE = 400;

/** Max entries in the stat summary before it collapses to "and N more". */
export const MAX_STAT_ENTRIES = 500;

/** How many omitted file paths to name explicitly before "and K more". */
const MAX_OMITTED_LISTED = 20;

/** Reserve for section headers and the omitted-files note (budget-scaled). */
function overheadReserve(maxChars: number): number {
  return Math.min(2_000, Math.max(300, Math.floor(maxChars / 50)));
}

/** Lockfiles and manifests whose content never earns a slice. */
const GENERATED_BASENAMES = new Set([
  "package-lock.json",
  "yarn.lock",
  "pnpm-lock.yaml",
  "bun.lockb",
  "bun.lock",
  "Cargo.lock",
  "composer.lock",
  "poetry.lock",
  "Gemfile.lock",
  "go.sum",
  "Package.resolved",
]);

/** Directory/minified patterns treated as generated content. */
const GENERATED_PATTERNS: RegExp[] = [
  /(^|\/)(dist|build|vendor|node_modules)\//,
  /\.min\.[cm]?[jt]sx?$/,
  /\.min\.css$/,
  /\.map$/,
];

/**
 * True when `path` looks generated or vendored: its content is excluded
 * from the budget (the stat summary still lists it).
 */
export function isGeneratedPath(path: string): boolean {
  const base = path.split("/").at(-1) ?? path;
  if (GENERATED_BASENAMES.has(base)) return true;
  return GENERATED_PATTERNS.some((re) => re.test(path));
}

/** One stat-summary line (mirrors `diffSummaryText`'s style). */
function statLine(file: FileDiff): string {
  const kind = file.binary || file.is_image ? " (binary)" : "";
  return `- ${file.path}${kind} (+${file.additions}/-${file.deletions})`;
}

/**
 * Renders the complete change overview: one `path (+a/-d)` line per file.
 * Beyond {@link MAX_STAT_ENTRIES} entries collapses into an explicit
 * "and N more files" note so huge changes stay bounded but honest.
 */
export function statSummaryText(
  files: FileDiff[],
  maxEntries: number = MAX_STAT_ENTRIES,
): string {
  const lines = files.slice(0, maxEntries).map(statLine);
  const rest = files.length - lines.length;
  const listed = [`Files changed (${files.length}):`, ...lines];
  if (rest > 0) listed.push(`… and ${rest} more files (not listed)`);
  return listed.join("\n");
}

/** Counts changed (+/-) lines across a file's hunks. */
function changedLineCount(hunks: DiffHunk[]): number {
  let n = 0;
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      if (line.origin === "+" || line.origin === "-") n += 1;
    }
  }
  return n;
}

/**
 * Renders one file's diff within `maxChars`: unified-style headers and
 * hunks, cut mid-hunk when the slice runs out, with an explicit
 * "[+N more changed lines in this file]" note (context lines excluded
 * from the count). The file headers are always included.
 */
export function renderFileSlice(file: FileDiff, maxChars: number): string {
  const old = file.old_path && file.old_path !== file.path ? file.old_path : file.path;
  const header = `--- a/${old}\n+++ b/${file.path}\n`;
  const out: string[] = [header];
  let used = header.length;
  let shownChanged = 0;
  const totalChanged = changedLineCount(file.hunks);

  outer: for (const hunk of file.hunks) {
    const hunkHeader = `@@ -${hunk.old_start} +${hunk.new_start} @@`;
    if (used + hunkHeader.length + 1 > maxChars) break;
    out.push(hunkHeader);
    used += hunkHeader.length + 1;
    for (const line of hunk.lines) {
      const text = `${line.origin === "=" ? " " : line.origin}${line.text}`;
      if (used + text.length + 1 > maxChars) break outer;
      out.push(text);
      used += text.length + 1;
      if (line.origin === "+" || line.origin === "-") shownChanged += 1;
    }
  }

  const omitted = totalChanged - shownChanged;
  if (omitted > 0) out.push(`[+${omitted} more changed lines in this file]`);
  return out.join("\n");
}

/** Options for {@link budgetedDiffText} (all optional). */
export interface DiffBudgetOptions {
  /** Total char budget for the rendered text. Default 24k. */
  maxChars?: number;
  /** Max content chars per file. Default 6k. */
  perFileCap?: number;
  /** Max stat-summary entries. Default 100. */
  maxStatEntries?: number;
}

/**
 * Renders staged files as prompt text under a hard character budget:
 * stat summary first (always complete to its cap), then per-file diff
 * content with a fair per-file slice, then an explicit list of files
 * whose content was omitted. Small inputs render essentially the full
 * diff, so nothing changes for everyday commits.
 */
export function budgetedDiffText(files: FileDiff[], opts: DiffBudgetOptions = {}): string {
  const maxChars = opts.maxChars ?? MAX_DIFF_CHARS;
  const perFileCap = opts.perFileCap ?? PER_FILE_CAP;
  const stats = statSummaryText(files, opts.maxStatEntries);

  const sections = [stats];
  const omitted: string[] = [];
  const parts: string[] = [];
  const contentBudget = Math.max(0, maxChars - stats.length - overheadReserve(maxChars));
  // Binary/image files never had diff content to begin with — they stay
  // out of both the content sections and the omitted note (their stat
  // line already says "(binary)"). Generated/lockfile files ARE listed
  // as omitted so the model never mistakes the budget for the truth.
  const eligible = files.filter((f) => !f.binary && !f.is_image);
  const share =
    eligible.length > 0
      ? Math.min(perFileCap, Math.max(MIN_FILE_SHARE, Math.floor(contentBudget / eligible.length)))
      : 0;
  let spent = 0;
  for (const file of eligible) {
    if (isGeneratedPath(file.path)) {
      omitted.push(file.path);
      continue;
    }
    const remaining = contentBudget - spent;
    if (remaining < MIN_FILE_SHARE) {
      omitted.push(file.path);
      continue;
    }
    const slice = renderFileSlice(file, Math.min(share, remaining));
    parts.push(slice);
    spent += slice.length;
  }
  if (parts.length > 0) {
    // Header only when something is missing — small changes keep the
    // plain "stats + diff" shape.
    if (parts.length < eligible.length) {
      sections.push(`Diff content (${parts.length} of ${files.length} files, per-file budget):`);
    }
    sections.push(parts.join("\n\n"));
  }
  if (omitted.length > 0) {
    const named = omitted.slice(0, MAX_OMITTED_LISTED).join(", ");
    const rest = omitted.length - Math.min(omitted.length, MAX_OMITTED_LISTED);
    sections.push(
      `[Content omitted for ${omitted.length} file${omitted.length === 1 ? "" : "s"} ` +
        `(stats above): ${named}${rest > 0 ? `, and ${rest} more` : ""}]`,
    );
  }
  return sections.join("\n\n");
}
