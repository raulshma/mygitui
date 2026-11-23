/**
 * Pure model for the Overview: the repo library (recents ∪ open tabs ∪
 * favorites), the row view-model, filter presets, sort orders, and the
 * summary-strip totals. No IPC, no Svelte — fully unit tested in
 * `overviewModel.test.ts`.
 *
 * The grouping helpers (`groupStatusEntries`, `kindLetter`, `splitPath`,
 * `openSide`) moved here from the deleted Agent-mode `changesModel`: the
 * Overview row is their only consumer now (the main workspace's
 * `statusModel` keeps its own grouping).
 */
import type {
  ChangeKind,
  PrInfo,
  RepoStatus,
  RepoSummary,
  RepoSummaryCommit,
  StatusEntry,
} from "$lib/ipc/types";

// ---------------------------------------------------------------------------
// Library (the repo source)
// ---------------------------------------------------------------------------

/** One recent-repository entry as stored by `$lib/entry/recentRepos`. */
export interface LibraryRecent {
  path: string;
  pinned: boolean;
  lastOpened: number;
}

/** One library entry: a deduped repo root with its provenance flags. */
export interface LibraryEntry {
  /** Canonical display path (first-seen spelling wins). */
  root: string;
  /** Final path segment (row title). */
  name: string;
  pinned: boolean;
  favorite: boolean;
  /** Recency in ms epoch (0 = unknown, e.g. tab- or favorite-only roots). */
  lastOpened: number;
  /** The root has an open tab (live data source). */
  open: boolean;
}

/** Normalizes a path for library deduping (separators + trailing slash). */
export function repoKey(path: string): string {
  return path.replace(/[\\/]+/g, "/").replace(/\/+$/, "").toLowerCase();
}

/** Final path segment (`C:\x\repo` and `repo/` both → `repo`). */
export function repoName(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1]! : path;
}

/**
 * Builds the library: recents ∪ open-tab roots ∪ favorites, deduped by
 * canonical root. Default order is recent activity (newest first), pinned
 * entries first — recents carry pins; tabs and favorites-only roots sort
 * after anything with a recency (their `lastOpened` is 0).
 */
export function buildLibrary(
  recents: LibraryRecent[],
  tabRoots: string[],
  favorites: string[],
): LibraryEntry[] {
  const byKey = new Map<string, LibraryEntry>();
  const upsert = (
    root: string,
    patch: {
      pinned?: boolean;
      favorite?: boolean;
      lastOpened?: number;
      open?: boolean;
    } = {},
  ): LibraryEntry => {
    const key = repoKey(root);
    const existing = byKey.get(key);
    if (existing) {
      if (patch.pinned) existing.pinned = true;
      if (patch.favorite) existing.favorite = true;
      if (patch.lastOpened !== undefined && patch.lastOpened > existing.lastOpened) {
        existing.lastOpened = patch.lastOpened;
      }
      if (patch.open) existing.open = true;
      return existing;
    }
    const entry: LibraryEntry = {
      root,
      name: repoName(root),
      pinned: patch.pinned ?? false,
      favorite: patch.favorite ?? false,
      lastOpened: patch.lastOpened ?? 0,
      open: patch.open ?? false,
    };
    byKey.set(key, entry);
    return entry;
  };

  for (const tab of tabRoots) upsert(tab, { open: true });
  for (const favorite of favorites) upsert(favorite, { favorite: true });
  for (const recent of recents) {
    upsert(recent.path, {
      pinned: recent.pinned,
      lastOpened: recent.lastOpened,
    });
  }

  return [...byKey.values()].sort(
    (a, b) =>
      Number(b.pinned) - Number(a.pinned) || b.lastOpened - a.lastOpened,
  );
}

// ---------------------------------------------------------------------------
// Row view-model
// ---------------------------------------------------------------------------

/** Change counts folded out of status entries (one code path for every tier). */
export interface OverviewCounts {
  staged: number;
  unstaged: number;
  untracked: number;
  conflicts: number;
}

/** Groups status entries into staged / unstaged-tracked / untracked. */
export interface StatusGroups {
  staged: StatusEntry[];
  changed: StatusEntry[];
  untracked: StatusEntry[];
}

/**
 * Groups entries into Staged / Changed / Untracked, preserving the backend's
 * entry order inside each group. One entry lands in exactly one group: an
 * untracked worktree file always goes to `untracked`; otherwise an index-side
 * change is `staged` — even when the worktree is dirty too; otherwise a dirty
 * worktree (including `conflicted`) is `changed`.
 */
export function groupStatusEntries(entries: StatusEntry[]): StatusGroups {
  const groups: StatusGroups = { staged: [], changed: [], untracked: [] };
  for (const entry of entries) {
    if (entry.worktree === "untracked") {
      groups.untracked.push(entry);
    } else if (entry.index !== "unmodified") {
      groups.staged.push(entry);
    } else if (entry.worktree !== "unmodified") {
      groups.changed.push(entry);
    }
  }
  return groups;
}

/** One-letter kind badge for a row (`M`/`A`/`D`/`R`/`C`/`U`, `!`, `·`). */
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

/** Splits `a/b/c.txt` into base `c.txt` + dir `a/b` (rows show both). */
export function splitPath(path: string): { base: string; dir: string } {
  const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (idx === -1) return { base: path, dir: "" };
  return { base: path.slice(idx + 1), dir: path.slice(0, idx) };
}

/**
 * Which diff side a row opens: a purely staged entry diffs HEAD→index,
 * everything else diffs index→worktree (mirrors StatusPanel's scope rule).
 */
export function openSide(entry: StatusEntry): "worktree" | "index" {
  return entry.index !== "unmodified" && entry.worktree === "unmodified"
    ? "index"
    : "worktree";
}

/** The entry's displayed change kind (conflicts show on the index side). */
export function rowKind(entry: StatusEntry): ChangeKind {
  return entry.index === "conflicted"
    ? "conflicted"
    : entry.worktree !== "unmodified"
      ? entry.worktree
      : entry.index;
}

export function countsFromEntries(entries: StatusEntry[]): OverviewCounts {
  const counts: OverviewCounts = {
    staged: 0,
    unstaged: 0,
    untracked: 0,
    conflicts: 0,
  };
  for (const entry of entries) {
    if (entry.index === "conflicted") {
      counts.conflicts += 1;
      continue;
    }
    if (entry.index !== "unmodified") counts.staged += 1;
    if (entry.worktree === "untracked") {
      counts.untracked += 1;
    } else if (entry.worktree !== "unmodified") {
      counts.unstaged += 1;
    }
  }
  return counts;
}

/** Branch/status snapshot shared by every data tier (tab live or summary). */
export interface OverviewSnapshot {
  branch: string | null;
  detached: boolean;
  ahead: number;
  behind: number;
  entries: StatusEntry[];
}

/** Everything an Overview row renders, whatever tier produced it. */
export interface OverviewRowData {
  root: string;
  name: string;
  pinned: boolean;
  favorite: boolean;
  /** Recency in ms epoch from LibraryEntry (0 if none / unknown). */
  lastOpened: number;
  /** Whether this repo has a remote configured. */
  hasRemote: boolean;
  /** Primary remote URL (e.g. from origin), null if none. */
  remoteUrl: string | null;
  /** True if root starts with "ssh://" or the open tab has a deviceId. */
  isRemoteDevice: boolean;
  /** The root has an open tab (live tab-style data). */
  open: boolean;
  /** Repo id backing live reads (tab id, or the warmed session's id). */
  repoId: string | null;
  /** True when the scan failed because the folder is gone / not a repo. */
  missing: boolean;
  /** True while a scan/refresh is in flight for this row. */
  refreshing: boolean;
  /** True once the first data (live or summary) landed. */
  loaded: boolean;
  /** Open PRs (null = no data yet — the row shows an em-dash cell). */
  prs: PrInfo[] | null;
  /** HEAD CI: number of failing checks (null = unknown). */
  failingChecks: number | null;
  branch: string | null;
  detached: boolean;
  ahead: number;
  behind: number;
  counts: OverviewCounts;
  entries: StatusEntry[];
  lastCommit: RepoSummaryCommit | null;
  /** Wall-clock (ms) the row's data last landed (the "updated Xs ago" stamp). */
  refreshedAt: number | null;
}

/** Builds an empty row skeleton (first paint before any data lands). */
export function emptyRow(entry: LibraryEntry): OverviewRowData {
  return {
    root: entry.root,
    name: entry.name,
    pinned: entry.pinned,
    favorite: entry.favorite,
    open: entry.open,
    lastOpened: entry.lastOpened ?? 0,
    hasRemote: false,
    remoteUrl: null,
    isRemoteDevice: entry.root.startsWith("ssh://"),
    repoId: null,
    missing: false,
    refreshing: false,
    loaded: false,
    prs: null,
    failingChecks: null,
    branch: null,
    detached: false,
    ahead: 0,
    behind: 0,
    counts: { staged: 0, unstaged: 0, untracked: 0, conflicts: 0 },
    entries: [],
    lastCommit: null,
    refreshedAt: null,
  };
}

/** Applies a `repo_summary` snapshot to a row (scan tier). */
export function applySummary(
  row: OverviewRowData,
  summary: RepoSummary,
  refreshedAt: number,
): OverviewRowData {
  const summaryAny = summary as unknown as {
    has_remote?: boolean;
    hasRemote?: boolean;
    remote_url?: string | null;
    remoteUrl?: string | null;
  };
  const hasRemote =
    summaryAny.has_remote ??
    summaryAny.hasRemote ??
    row.hasRemote;
  const remoteUrl =
    summaryAny.remote_url !== undefined
      ? summaryAny.remote_url
      : summaryAny.remoteUrl !== undefined
        ? summaryAny.remoteUrl
        : row.remoteUrl;

  return {
    ...row,
    loaded: true,
    missing: false,
    refreshing: false,
    lastOpened: row.lastOpened,
    hasRemote: hasRemote ?? false,
    remoteUrl: remoteUrl ?? null,
    isRemoteDevice: row.isRemoteDevice,
    branch: summary.branch,
    detached: summary.detached,
    ahead: summary.ahead,
    behind: summary.behind,
    counts: countsFromEntries(summary.entries),
    entries: summary.entries,
    lastCommit: summary.last_commit,
    refreshedAt,
  };
}

/** Applies a live `RepoStatus` (open-tab or warmed tier). */
export function applyLiveStatus(
  row: OverviewRowData,
  status: RepoStatus,
  refreshedAt: number,
): OverviewRowData {
  const statusAny = status as unknown as {
    has_remote?: boolean;
    hasRemote?: boolean;
    remote_url?: string | null;
    remoteUrl?: string | null;
  };
  const hasRemote =
    statusAny.has_remote ??
    statusAny.hasRemote ??
    row.hasRemote;
  const remoteUrl =
    statusAny.remote_url !== undefined
      ? statusAny.remote_url
      : statusAny.remoteUrl !== undefined
        ? statusAny.remoteUrl
        : row.remoteUrl;

  return {
    ...row,
    loaded: true,
    missing: false,
    refreshing: false,
    lastOpened: row.lastOpened,
    hasRemote: hasRemote ?? false,
    remoteUrl: remoteUrl ?? null,
    isRemoteDevice: row.isRemoteDevice,
    branch: status.branch,
    detached: status.detached,
    ahead: status.ahead,
    behind: status.behind,
    counts: countsFromEntries(status.entries),
    entries: status.entries,
    refreshedAt,
  };
}

/** Marks a row missing (scan failed: folder gone / no longer a repo). */
export function markMissing(row: OverviewRowData, refreshedAt: number): OverviewRowData {
  return { ...row, loaded: true, missing: true, refreshing: false, refreshedAt };
}

/** True when a row shows open PRs (the filter + the strip's PR total). */
export function rowHasPrs(row: OverviewRowData): boolean {
  return (row.prs?.length ?? 0) > 0;
}

/** True when the row's branch is out of sync in either direction. */
export function rowSyncDebt(row: OverviewRowData): boolean {
  return row.ahead > 0 || row.behind > 0;
}

/** True when the working copy has any change (any kind). */
export function rowDirty(row: OverviewRowData): boolean {
  const c = row.counts;
  return c.staged + c.unstaged + c.untracked + c.conflicts > 0;
}

/** True when the row's HEAD CI is known to have failing checks. */
export function rowFailingChecks(row: OverviewRowData): boolean {
  return (row.failingChecks ?? 0) > 0;
}

// ---------------------------------------------------------------------------
// Remote sync + provider + recency helpers
// ---------------------------------------------------------------------------

export type RemoteSyncState =
  | "in-sync"
  | "needs-push"
  | "needs-pull"
  | "diverged"
  | "local-only";

export interface RemoteSyncStatus {
  status: RemoteSyncState;
  label: string;
  outgoing: number;
  incoming: number;
}

/**
 * Computes the remote sync status and label for a row:
 * - When !row.hasRemote: 'local-only' ("Local only")
 * - When row.ahead > 0 && row.behind > 0: 'diverged' ("↑" + row.ahead + " ↓" + row.behind)
 * - When row.ahead > 0: 'needs-push' ("↑" + row.ahead + " to push")
 * - When row.behind > 0: 'needs-pull' ("↓" + row.behind + " to pull")
 * - When row.ahead === 0 && row.behind === 0: 'in-sync' ("In sync")
 */
export function remoteSyncStatus(row: OverviewRowData): RemoteSyncStatus {
  const outgoing = row.ahead;
  const incoming = row.behind;
  if (!row.hasRemote) {
    return {
      status: "local-only",
      label: "Local only",
      outgoing,
      incoming,
    };
  }
  if (row.ahead > 0 && row.behind > 0) {
    return {
      status: "diverged",
      label: `↑${row.ahead} ↓${row.behind}`,
      outgoing,
      incoming,
    };
  }
  if (row.ahead > 0) {
    return {
      status: "needs-push",
      label: `↑${row.ahead} to push`,
      outgoing,
      incoming,
    };
  }
  if (row.behind > 0) {
    return {
      status: "needs-pull",
      label: `↓${row.behind} to pull`,
      outgoing,
      incoming,
    };
  }
  return {
    status: "in-sync",
    label: "In sync",
    outgoing: 0,
    incoming: 0,
  };
}

export type RepoProvider =
  | "github"
  | "gitlab"
  | "remote-device"
  | "remote"
  | "local";

/**
 * Identifies the repository provider/type:
 * - 'remote-device' if row.isRemoteDevice
 * - 'local' if !row.hasRemote
 * - 'github' if remoteUrl includes github
 * - 'gitlab' if remoteUrl includes gitlab
 * - 'remote' otherwise
 */
export function repoProvider(row: OverviewRowData): RepoProvider {
  if (row.isRemoteDevice) return "remote-device";
  if (!row.hasRemote) return "local";
  const url = (row.remoteUrl ?? "").toLowerCase();
  if (url.includes("github.com") || url.includes("github")) return "github";
  if (url.includes("gitlab.com") || url.includes("gitlab")) return "gitlab";
  return "remote";
}

/** "Accessed 2h ago" for a wall-clock ms epoch; null if lastOpened <= 0. */
export function lastOpenedAgo(lastOpened: number, now: number): string | null {
  if (lastOpened <= 0 || !Number.isFinite(lastOpened)) return null;
  const seconds = Math.max(0, Math.floor((now - lastOpened) / 1000));
  if (seconds < 60) return `Accessed ${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `Accessed ${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `Accessed ${hours}h ago`;
  return `Accessed ${Math.floor(hours / 24)}d ago`;
}

// ---------------------------------------------------------------------------
// Filter presets + sort
// ---------------------------------------------------------------------------

export type OverviewFilterId =
  | "needs-push"
  | "needs-pull"
  | "dirty"
  | "has-prs"
  | "failing-checks"
  | "missing"
  | "local-only";

/** The strip's filter presets, in display order. */
export const OVERVIEW_FILTERS: Array<{ id: OverviewFilterId; label: string }> = [
  { id: "needs-push", label: "Needs push" },
  { id: "needs-pull", label: "Needs pull" },
  { id: "dirty", label: "Dirty" },
  { id: "has-prs", label: "Has PRs" },
  { id: "failing-checks", label: "Failing checks" },
  { id: "missing", label: "Missing" },
  { id: "local-only", label: "Local only" },
];

/** Applies one preset; `null` = no filter (all rows). */
export function matchesFilter(
  row: OverviewRowData,
  filter: OverviewFilterId | null,
): boolean {
  switch (filter) {
    case "needs-push":
      return row.ahead > 0;
    case "needs-pull":
      return row.behind > 0;
    case "dirty":
      return rowDirty(row);
    case "has-prs":
      return rowHasPrs(row);
    case "failing-checks":
      return rowFailingChecks(row);
    case "missing":
      return row.missing;
    case "local-only":
      return !row.hasRemote;
    default:
      return true;
  }
}

export type OverviewSortId =
  | "attention"
  | "name"
  | "last-commit"
  | "ahead"
  | "behind"
  | "changes"
  | "prs";

/** Sort orders the header exposes; "attention" is the default. */
export const OVERVIEW_SORTS: Array<{ id: OverviewSortId; label: string }> = [
  { id: "name", label: "Name" },
  { id: "last-commit", label: "Last commit" },
  { id: "ahead", label: "Ahead" },
  { id: "behind", label: "Behind" },
  { id: "changes", label: "Changes" },
  { id: "prs", label: "PRs" },
];

export type SortDirection = "asc" | "desc";

/** Attention rank: conflicts first, then sync debt, dirty, clean. */
export function attentionRank(row: OverviewRowData): number {
  if (row.counts.conflicts > 0) return 0;
  if (rowSyncDebt(row)) return 1;
  if (rowDirty(row)) return 2;
  return 3;
}

function newestCommit(row: OverviewRowData): number {
  return row.lastCommit?.timestamp ?? 0;
}

/** Numeric/string key of a sort column (ties resolve toward the default). */
function sortValue(row: OverviewRowData, sort: OverviewSortId): number | string {
  switch (sort) {
    case "name":
      return row.name.toLowerCase();
    case "last-commit":
      return newestCommit(row);
    case "ahead":
      return row.ahead;
    case "behind":
      return row.behind;
    case "changes": {
      const c = row.counts;
      return c.staged + c.unstaged + c.untracked + c.conflicts;
    }
    case "prs":
      return row.prs?.length ?? 0;
    default:
      return attentionRank(row);
  }
}

/**
 * Sorts rows by `sort`/`dir`. "attention" always sorts attention-first and
 * `desc` flips to clean-first; the named columns honor `dir` literally. Ties
 * break by newest commit (then name), stable across refreshes.
 */
export function sortRows(
  rows: OverviewRowData[],
  sort: OverviewSortId,
  dir: SortDirection,
): OverviewRowData[] {
  const sign = dir === "asc" ? 1 : -1;
  return [...rows].sort((a, b) => {
    const av = sortValue(a, sort);
    const bv = sortValue(b, sort);
    let cmp: number;
    if (typeof av === "string" || typeof bv === "string") {
      cmp = String(av).localeCompare(String(bv)) * sign;
    } else {
      cmp = (av - bv) * sign;
    }
    if (cmp !== 0) return cmp;
    const byCommit = newestCommit(b) - newestCommit(a);
    if (byCommit !== 0) return byCommit;
    return a.name.localeCompare(b.name);
  });
}

/** Summary-strip totals over the (unfiltered) rows. */
export interface OverviewTotals {
  repos: number;
  dirty: number;
  ahead: number;
  behind: number;
  prs: number;
  /** Number of repos needing attention (conflicts, sync debt, dirty, failing checks). */
  attentionNeeded: number;
  /** Number of repos with sync debt (ahead > 0 or behind > 0). */
  outOfSync: number;
  /** Number of repos in sync with remote and clean. */
  inSync: number;
}

export function totalsOf(rows: OverviewRowData[]): OverviewTotals {
  const totals: OverviewTotals = {
    repos: rows.length,
    dirty: 0,
    ahead: 0,
    behind: 0,
    prs: 0,
    attentionNeeded: 0,
    outOfSync: 0,
    inSync: 0,
  };
  for (const row of rows) {
    const isDirty = rowDirty(row);
    const hasSyncDebt = rowSyncDebt(row);
    const hasFailing = rowFailingChecks(row);
    const hasConflicts = row.counts.conflicts > 0;

    if (isDirty) totals.dirty += 1;
    totals.ahead += row.ahead;
    totals.behind += row.behind;
    totals.prs += row.prs?.length ?? 0;

    if (hasConflicts || hasSyncDebt || isDirty || hasFailing) {
      totals.attentionNeeded += 1;
    } else {
      totals.inSync += 1;
    }

    if (hasSyncDebt) {
      totals.outOfSync += 1;
    }
  }
  return totals;
}

/** "updated 12s ago" for a wall-clock stamp; `null` before the first refresh. */
export function refreshedAgo(refreshedAt: number | null, now: number): string | null {
  if (refreshedAt === null) return null;
  const seconds = Math.max(0, Math.floor((now - refreshedAt) / 1000));
  if (seconds < 60) return `updated ${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `updated ${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `updated ${hours}h ago`;
  return `updated ${Math.floor(hours / 24)}d ago`;
}

/** True when a row should render expanded by default (conflicts/sync debt). */
export function defaultExpanded(row: OverviewRowData): boolean {
  return row.counts.conflicts > 0 || rowSyncDebt(row);
}

/** Short sha for row/diff headers (7 chars, the git-log convention). */
export function shortSha(sha: string): string {
  return sha.slice(0, 7);
}
