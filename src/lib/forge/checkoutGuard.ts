/**
 * Checkout-safety decision (E2, plan §9.4).
 *
 * The footer-chip popover checkout must HARD-REFUSE on a dirty worktree
 * (toast linking to STATUS) instead of letting `gh pr checkout` fail
 * mid-branch-switch; the panel checkout offers a named, restorable stash
 * first. Both surfaces share this pure decision so the two entry points
 * cannot drift.
 */

import type { RepoStatus } from "$lib/ipc/types";

/**
 * Why a PR checkout must not run right now (`null` = safe to proceed).
 *
 * A worktree with any change entry (staged, unstaged, untracked — the
 * status entries cover all three) is dirty: `gh pr checkout` would either
 * refuse or silently carry the changes across a branch switch. In-progress
 * operations (merge/rebase/cherry-pick/bisect) count as dirty too —
 * switching branches mid-sequence corrupts them.
 */
export function checkoutRefusal(status: RepoStatus | null): string | null {
  if (status === null) return null;
  if (status.entries.length > 0) {
    const n = status.entries.length;
    return `The worktree has ${n} uncommitted change${n === 1 ? "" : "s"}`;
  }
  if (status.merging) return "A merge is in progress";
  if (status.rebasing) return "A rebase is in progress";
  if (status.sequencer) return "A cherry-pick/revert is in progress";
  if (status.bisecting) return "A bisect search is in progress";
  return null;
}

/** Number of distinct changed paths (the STATUS panel's own count). */
export function dirtyChangeCount(status: RepoStatus | null): number {
  if (status === null) return 0;
  return new Set(status.entries.map((entry) => entry.path)).size;
}
