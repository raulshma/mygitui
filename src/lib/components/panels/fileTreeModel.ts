/**
 * Pure file-tree model for the Files panel — no runes, no DOM, no IPC:
 * fully unit-testable, exactly like `statusModel.ts` / `layoutModel.ts`
 * before it.
 *
 * The tree is lazily populated: the FE expands a directory by calling
 * `listDir` and merging the result with {@link insertListing}. Nodes carry
 * an `expanded` flag (UI state, persisted per repo by `fileTreePrefs`)
 * and `children: null` marks "never listed yet".
 *
 * Rendering walks {@link flattenVisible}: one flat row array over the
 * expanded subtree, consumed by the panel's windowed virtualization
 * (`visibleRange` from `$lib/components/graph/layout`). Status badges are
 * built once per status snapshot by {@link buildStatusBadges} — never per
 * keystroke or per render.
 */

import type { DirListing, StatusEntry, ChangeKind } from "$lib/ipc/types";

// ---------------------------------------------------------------------------
// Node model
// ---------------------------------------------------------------------------

/** Same shapes the backend reports (mirrors `DirEntryKind`). */
export type FileEntryKind = "file" | "dir" | "symlink" | "submodule";

/** One tree node (a file, a directory, a symlink, or a submodule dir). */
export interface FileNode {
  /** Last path segment (display name). */
  name: string;
  /** Repository-relative path, forward slashes ("" = workdir root). */
  path: string;
  kind: FileEntryKind;
  /** File size in bytes; null for dirs. */
  size: number | null;
  /**
   * Children when the dir has been listed; null = not loaded yet (shows a
   * placeholder chevron until expanded). Files/symlinks/submodules: null.
   */
  children: FileNode[] | null;
  /** Directory UI state (persisted per repo). */
  expanded: boolean;
  /** The backend listing was capped — some children are missing. */
  truncated: boolean;
}

/** Workdir root node ("" — always a dir, never expanded itself). */
export function createRoot(): FileNode {
  return {
    name: "",
    path: "",
    kind: "dir",
    size: null,
    children: null,
    expanded: true,
    truncated: false,
  };
}

/** Dirs-first, case-insensitive natural (digit-aware) name order — the
 * mirror of the backend's `compare_entries`, so locally-inserted entries
 * (created file/folder) land where the next backend listing would put
 * them. */
export function compareNames(a: string, b: string): number {
  return naturalCmp(a, b);
}

function naturalCmp(a: string, b: string): number {
  let ia = 0;
  let ib = 0;
  while (ia < a.length && ib < b.length) {
    const ca = a[ia]!;
    const cb = b[ib]!;
    if (isDigit(ca) && isDigit(cb)) {
      let ja = ia;
      let jb = ib;
      while (ja < a.length && isDigit(a[ja]!)) ja++;
      while (jb < b.length && isDigit(b[jb]!)) jb++;
      const na = a.slice(ia, ja).replace(/^0+/, "");
      const nb = b.slice(ib, jb).replace(/^0+/, "");
      if (na.length !== nb.length) return na.length - nb.length;
      if (na !== nb) return na < nb ? -1 : 1;
      ia = ja;
      ib = jb;
    } else {
      const la = ca.toLowerCase();
      const lb = cb.toLowerCase();
      if (la !== lb) return la < lb ? -1 : 1;
      if (ca !== cb) return ca < cb ? -1 : 1;
      ia++;
      ib++;
    }
  }
  // One ran out first: shorter (remaining) name sorts first.
  const ra = a.length - ia;
  const rb = b.length - ib;
  return ra === rb ? 0 : ra - rb;
}

function isDigit(c: string): boolean {
  return c >= "0" && c <= "9";
}

/** Insertion order key for a new sibling: dirs (incl. submodules) first. */
function kindRank(kind: FileEntryKind): number {
  return kind === "dir" || kind === "submodule" ? 0 : 1;
}

/** Sort one sibling list in place (dirs first, then natural by name). */
export function sortSiblings(children: FileNode[]): FileNode[] {
  children.sort((a, b) => {
    const rank = kindRank(a.kind) - kindRank(b.kind);
    return rank !== 0 ? rank : compareNames(a.name, b.name);
  });
  return children;
}

// ---------------------------------------------------------------------------
// Tree construction
// ---------------------------------------------------------------------------

/** Find a node by repo-relative path ("" = root). */
export function findNode(root: FileNode, path: string): FileNode | null {
  if (path === "") return root;
  let node: FileNode | undefined = root;
  for (const segment of path.split("/")) {
    if (!node || node.children === null) return null;
    node = node.children.find((child) => child.name === segment);
  }
  return node ?? null;
}

/** Nearest ancestor dir node along `dirPath`, synthesizing placeholders
 * for segments the tree has never seen (stale-parent safety). */
function ensureDirChain(root: FileNode, dirPath: string): FileNode | null {
  let node = root;
  if (dirPath === "") return root;
  for (const segment of dirPath.split("/")) {
    if (node.children === null) {
      // Never listed: the chain insertion itself initializes it. A dir
      // whose children are being placed is by definition "listed enough".
      if (node.kind !== "dir") return null;
      node.children = [];
    }
    let next = node.children.find((child) => child.name === segment);
    if (!next) {
      // Intermediate dir we have not seen: create a placeholder that the
      // next listing of it will fill.
      next = {
        name: segment,
        path: node.path === "" ? segment : `${node.path}/${segment}`,
        kind: "dir",
        size: null,
        children: null,
        expanded: false,
        truncated: false,
      };
      node.children.push(next);
      sortSiblings(node.children);
    }
    node = next;
  }
  return node;
}

/**
 * Merge one `listDir` result into the tree: (re)places the directory's
 * child list. Mutates the tree in place (the panel holds it in `$state`,
 * whose deep proxies make in-place mutation reactive) and returns the node
 * for convenience. `null` when the listing's parent chain isn't in the
 * tree (stale response after an external delete — safe to ignore).
 */
export function insertListing(root: FileNode, listing: DirListing): FileNode | null {
  const parent = ensureDirChain(root, listing.path);
  if (!parent || parent.kind !== "dir") return null;
  parent.children = listing.entries.map((entry) => ({
    name: entry.name,
    path: entry.path,
    kind: entry.kind,
    size: entry.size,
    children: null,
    // A submodule IS a dir conceptually (openable), but its children are
    // another repo's tree — the panel never lists into it in v1.
    expanded: false,
    truncated: false,
  }));
  sortSiblings(parent.children);
  parent.truncated = listing.truncated;
  return parent;
}

/**
 * Add one new entry (created via the tree's New File/Folder action) to its
 * already-listed parent. Returns false when the parent isn't loaded (the
 * next expansion picks it up from the backend).
 */
export function insertEntry(
  root: FileNode,
  parentPath: string,
  entry: { name: string; kind: FileEntryKind; size?: number | null },
): boolean {
  const parent = ensureDirChain(root, parentPath);
  if (!parent || parent.children === null) return false;
  if (parent.children.some((child) => child.name === entry.name)) return false;
  parent.children.push({
    name: entry.name,
    path: parentPath === "" ? entry.name : `${parentPath}/${entry.name}`,
    kind: entry.kind,
    size: entry.size ?? (entry.kind === "file" ? 0 : null),
    children: null,
    expanded: false,
    truncated: false,
  });
  sortSiblings(parent.children);
  return true;
}

/** Drop a node (deleted via the tree) from its listed parent. */
export function removeEntry(root: FileNode, path: string): boolean {
  const idx = path.lastIndexOf("/");
  const parentPath = idx === -1 ? "" : path.slice(0, idx);
  const name = idx === -1 ? path : path.slice(idx + 1);
  const parent = findNode(root, parentPath);
  if (!parent || parent.children === null) return false;
  const before = parent.children.length;
  parent.children = parent.children.filter((child) => child.name !== name);
  return parent.children.length < before;
}

/**
 * Rename/move within the tree (rename via the tree). Re-inserts the node
 * under its new parent, updating `name`/`path` (and descendant paths of a
 * moved dir — the panel re-lists dirs lazily, so stale descendant paths
 * are acceptable, but renamed node paths must be exact for selection).
 */
export function renameEntry(root: FileNode, path: string, newPath: string): boolean {
  const node = findNode(root, path);
  if (!node) return false;
  const oldIdx = path.lastIndexOf("/");
  const newIdx = newPath.lastIndexOf("/");
  const newParentPath = newIdx === -1 ? "" : newPath.slice(0, newIdx);
  const newName = newIdx === -1 ? newPath : newPath.slice(newIdx + 1);
  const detached = detach(root, path);
  if (!detached) return false;
  node.name = newName;
  node.path = newPath;
  const parent = ensureDirChain(root, newParentPath);
  if (!parent || parent.children === null) return true; // detached; parent appears on next listing
  parent.children.push(node);
  sortSiblings(parent.children);
  return true;
}

/** Remove (without deleting) the node at `path`, returning it. */
function detach(root: FileNode, path: string): FileNode | null {
  const idx = path.lastIndexOf("/");
  const parentPath = idx === -1 ? "" : path.slice(0, idx);
  const name = idx === -1 ? path : path.slice(idx + 1);
  const parent = findNode(root, parentPath);
  if (!parent || parent.children === null) return null;
  const at = parent.children.findIndex((child) => child.name === name);
  if (at === -1) return null;
  return parent.children.splice(at, 1)[0] ?? null;
}

// ---------------------------------------------------------------------------
// Flatten (render rows) + filter
// ---------------------------------------------------------------------------

/** One render row: node + tree depth (indent = depth × indent px). */
export interface FlatRow {
  node: FileNode;
  depth: number;
}

/**
 * Depth-first flatten of the expanded tree. The root itself is not a row.
 * This runs on every expansion/collapse — O(visible expanded tree), cheap
 * even at 100k nodes (perf-gated in the tests).
 */
export function flattenVisible(root: FileNode): FlatRow[] {
  const rows: FlatRow[] = [];
  walk(root, 0, rows);
  return rows;

  function walk(node: FileNode, depth: number, out: FlatRow[]): void {
    if (node.children === null || !node.expanded) return;
    for (const child of node.children) {
      out.push({ node: child, depth });
      walk(child, depth + 1, out);
    }
  }
}

/**
 * Filter rows for the panel's filter box: keep nodes whose name matches
 * the case-insensitive substring, plus every ancestor needed to reach
 * them (auto-expanded presentation — the returned rows are the flattened
 * match view; `expanded` flags themselves are not mutated).
 */
export function filterRows(root: FileNode, query: string): FlatRow[] {
  const needle = query.trim().toLowerCase();
  const rows: FlatRow[] = [];
  if (needle === "") return rows;
  walk(root, -1);
  return rows;

  /** Pre-order walk; true when this subtree contains a match. */
  function walk(node: FileNode, depth: number): boolean {
    if (node === root) {
      if (node.children !== null) {
        for (const child of node.children) walk(child, 0);
      }
      return true;
    }
    const self = node.name.toLowerCase().includes(needle);
    if (self) {
      rows.push({ node, depth });
      return true;
    }
    if (node.children === null) return false;
    // Tentatively include the dir; roll back when no descendant matches.
    const mark = rows.length;
    rows.push({ node, depth });
    let matched = false;
    for (const child of node.children) {
      if (walk(child, depth + 1)) matched = true;
    }
    if (!matched) rows.length = mark;
    return matched;
  }
}

// ---------------------------------------------------------------------------
// Status badges (built once per status snapshot)
// ---------------------------------------------------------------------------

/** Badge kind shown next to a file/dir (derived from the git change kind). */
export type BadgeKind =
  | "added"
  | "modified"
  | "deleted"
  | "renamed"
  | "conflicted"
  | "untracked";

/** Severity order for folder rollups (first = wins). */
const BADGE_SEVERITY: readonly BadgeKind[] = [
  "conflicted",
  "deleted",
  "modified",
  "added",
  "renamed",
  "untracked",
];

function badgeOfKind(kind: ChangeKind): BadgeKind {
  switch (kind) {
    case "added":
      return "added";
    case "deleted":
      return "deleted";
    case "renamed":
    case "copied":
      return "renamed";
    case "conflicted":
      return "conflicted";
    case "untracked":
      return "untracked";
    default:
      return "modified";
  }
}

/**
 * Per-path badge map over every node the tree shows: each changed file
 * maps to its badge, and every ancestor directory maps to the most severe
 * descendant badge (VS Code-style folder dots). Built once per status
 * snapshot; the tree lookup is a plain `Map.get(path)`.
 */
export function buildStatusBadges(entries: StatusEntry[]): Map<string, BadgeKind> {
  const byPath = new Map<string, BadgeKind>();
  const dirs = new Map<string, Set<BadgeKind>>();
  for (const entry of entries) {
    // Index-side change wins over worktree-side (what a commit would take).
    const badge =
      entry.index !== "unmodified"
        ? badgeOfKind(entry.index)
        : badgeOfKind(entry.worktree);
    if (!byPath.has(entry.path) || byPath.get(entry.path) === "untracked") {
      byPath.set(entry.path, badge);
    }
    // Ancestors accumulate (a/b/c.txt marks a/, a/b/).
    const segments = entry.path.split("/");
    for (let i = 1; i < segments.length; i++) {
      const dir = segments.slice(0, i).join("/");
      let set = dirs.get(dir);
      if (!set) {
        set = new Set();
        dirs.set(dir, set);
      }
      set.add(badge);
    }
  }
  for (const [dir, badges] of dirs) {
    const worst = BADGE_SEVERITY.find((b) => badges.has(b)) ?? "modified";
    const existing = byPath.get(dir);
    if (!existing || BADGE_SEVERITY.indexOf(worst) < BADGE_SEVERITY.indexOf(existing)) {
      byPath.set(dir, worst);
    }
  }
  return byPath;
}
