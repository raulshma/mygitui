/**
 * Pure logic for the commit bar's fixup quick action (M12, lane C).
 *
 * "Fixup into HEAD" commits the staged changes as `fixup! <HEAD summary>`.
 * The helpers here are the only nontrivial bits: detecting whether anything
 * is staged (from the tab store's status snapshot), locating HEAD inside a
 * streamed log page, and building the fixup message.
 */

import type { CommitInfo, LogPage, StatusEntry } from "$lib/ipc/types";

/** Status kinds that count as a staged change (index side). */
const STAGED_KINDS: ReadonlySet<string> = new Set([
  "added",
  "deleted",
  "modified",
  "renamed",
  "copied",
]);

/** True when at least one entry has a real change on the index side. */
export function hasStagedChanges(entries: readonly StatusEntry[]): boolean {
  return entries.some((entry) => STAGED_KINDS.has(entry.index));
}

/**
 * The HEAD commit inside a streamed log page: matched by sha (page order
 * is topological, not guaranteed HEAD-first across ref merges).
 */
export function headCommitInPage(
  page: LogPage,
  headSha: string | null,
): CommitInfo | null {
  if (!headSha) return null;
  return page.commits.find((c) => c.sha === headSha) ?? null;
}

/** The fixup commit message: `fixup! <first line of the HEAD summary>`. */
export function fixupMessage(summary: string): string {
  const firstLine = summary.split("\n", 1)[0]?.trim() ?? "";
  return `fixup! ${firstLine}`;
}
