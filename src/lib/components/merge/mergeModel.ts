/**
 * Pure logic for the merge dialog's dirty-worktree guard (M12, lane C).
 *
 * Merging with a dirty working copy risks half-applied states and
 * unresolvable conflicts; the dialog blocks the confirm (no override)
 * until the user commits or stashes. An entry counts as DIRTY when either
 * side carries a real change; `unmodified`, `ignored` and `untracked`
 * entries are safe (git itself would refuse differently, but merge does
 * not need them clean).
 */

import type { ChangeKind, StatusEntry } from "$lib/ipc/types";

/** Status kinds a merge tolerates in the working copy. */
const CLEAN_KINDS: ReadonlySet<ChangeKind> = new Set([
  "unmodified",
  "ignored",
  "untracked",
]);

/** True when ANY entry has an index- or worktree-side change (or conflict). */
export function workingCopyDirty(entries: readonly StatusEntry[]): boolean {
  return entries.some(
    (entry) => !CLEAN_KINDS.has(entry.index) || !CLEAN_KINDS.has(entry.worktree),
  );
}
