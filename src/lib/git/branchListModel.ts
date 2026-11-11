/**
 * Pure presentation model for the branches surfaces — filtering, head-first
 * ordering, remote-group construction, show-more paging, and tracking
 * lookups shared by BranchPanel (and reusable by the header switcher).
 *
 * Plain TypeScript on purpose: no runes, no IPC, no UI, so the panel
 * component stays thin and every rule here is unit-testable (same split as
 * `statusModel` / `panelModel`). Row order for remotes is whatever the
 * backend produced (already sorted remote-then-name); locals are re-ordered
 * head-first for the pinned "current branch" row.
 */

import type { BranchInfo, RemoteBranchInfo } from "$lib/ipc/types";

/** Rows rendered before a section's "Show all N" expander button. */
export const BRANCH_PAGE_SIZE = 200;

/** Case-insensitive substring match on one candidate name. */
export function branchMatchesFilter(name: string, filter: string): boolean {
  const q = filter.trim().toLowerCase();
  if (q === "") return true;
  return name.toLowerCase().includes(q);
}

/**
 * Local rows for rendering: the filter applies first, then the HEAD branch
 * is pinned on top (stable for the rest, preserving the backend's
 * alphabetical order).
 */
export function localSectionRows(
  list: readonly BranchInfo[],
  filter: string,
): BranchInfo[] {
  const matched = list.filter((b) => branchMatchesFilter(b.name, filter));
  const head = matched.findIndex((b) => b.is_head);
  if (head <= 0) return matched;
  const rows = [...matched];
  const [current] = rows.splice(head, 1);
  rows.unshift(current);
  return rows;
}

/**
 * Remote rows for rendering: matches on the full `remote/name` (so a
 * remote name alone filters its whole group); backend order preserved.
 * Remote branches a local branch tracks are NOT dropped — surfaces show a
 * "tracked by" affordance instead (the user asked to see every branch).
 */
export function remoteSectionRows(
  list: readonly RemoteBranchInfo[],
  filter: string,
): RemoteBranchInfo[] {
  return list.filter((rb) =>
    branchMatchesFilter(`${rb.remote}/${rb.name}`, filter),
  );
}

/** One collapsible per-remote group in the panel's remote section. */
export interface RemoteGroup {
  remote: string;
  branches: RemoteBranchInfo[];
}

/**
 * Groups remote rows by remote name, preserving the input order (the
 * backend sorts remote-then-name, so groups appear alphabetically with
 * sorted branches inside).
 */
export function groupRemoteBranches(
  rows: readonly RemoteBranchInfo[],
): RemoteGroup[] {
  const groups: RemoteGroup[] = [];
  const index = new Map<string, RemoteGroup>();
  for (const rb of rows) {
    let group = index.get(rb.remote);
    if (!group) {
      group = { remote: rb.remote, branches: [] };
      index.set(rb.remote, group);
      groups.push(group);
    }
    group.branches.push(rb);
  }
  return groups;
}

/** Result of {@link pageRows}: the rows to render plus the expander count. */
export interface BranchPage<T> {
  /** Rows to render (all of them once expanded). */
  visible: T[];
  /** Rows hidden behind the "Show all N" expander (0 when expanded). */
  hidden: number;
  /** Total row count (what the expander button announces). */
  total: number;
}

/**
 * Large-list guard: renders the first {@link BRANCH_PAGE_SIZE} rows until
 * the user expands. No virtualization — one slice, one expander button.
 */
export function pageRows<T>(
  rows: readonly T[],
  showAll: boolean,
  limit: number = BRANCH_PAGE_SIZE,
): BranchPage<T> {
  const total = rows.length;
  if (showAll || total <= limit) {
    return { visible: [...rows], hidden: 0, total };
  }
  return { visible: rows.slice(0, limit), hidden: total - limit, total };
}

/**
 * The local branch tracking `rb`: the backend's `tracked_by` pointer first,
 * falling back to an upstream match (keeps working when either list is
 * staler than the other). `null` = no local branch checks this out.
 */
export function trackingLocalBranch(
  localList: readonly BranchInfo[],
  rb: RemoteBranchInfo,
): BranchInfo | null {
  if (rb.tracked_by) {
    const byName = localList.find((b) => b.name === rb.tracked_by);
    if (byName) return byName;
  }
  return (
    localList.find((b) => b.upstream === `${rb.remote}/${rb.name}`) ?? null
  );
}
