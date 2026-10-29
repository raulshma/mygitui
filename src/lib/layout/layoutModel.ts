/**
 * Pure layout model (M4 F1) — the workspace splitter tree and its
 * operations. No runes, no DOM, no storage: fully unit-testable, exactly
 * like `statusModel.ts` / `panelModel.ts` before it.
 *
 * Tree schema (serializable JSON — every field is a plain value):
 *
 *   LayoutNode =
 *     | { kind: "leaf",  id, panel: PanelId }                 single panel
 *     | { kind: "tabs",  id, tabs: PanelId[], active }        tab group
 *     | { kind: "split", id, ratio, a: Node, b: Node }        binary splitter
 *
 * A split divides its area horizontally: child `a` renders on the LEFT of
 * the divider, `b` on the RIGHT, and `ratio` is `a`'s width fraction
 * (clamped to RATIO_MIN–RATIO_MAX). The workspace fixes only the vertical
 * frame (repo header above the tree); the tree itself is the VSCode-style
 * three-column workspace — changes sidebar, editor, history sidebar — and
 * tab groups absorb every additional panel. See `WorkspaceLayout` +
 * `DEFAULT_LAYOUT`.
 *
 * Every node carries a unique string `id` so operations (move, hide, ratio
 * drag) can target nodes from UI events and ratio overlays can key by split
 * id. Ids are opaque; `freshId` mints unused ones.
 */

import type { IconifyIcon } from "@iconify/types";

// ---------------------------------------------------------------------------
// Panel registry
// ---------------------------------------------------------------------------

/** Every panel the layout system can place. */
export type PanelId =
  | "files"
  | "status"
  | "branches"
  | "remotes"
  | "stashes"
  | "worktrees"
  | "health"
  | "submodules"
  | "reflog"
  | "undo"
  | "history"
  | "diff"
  | "terminal"
  | "forge"
  | "actions"
  | "stats";

/** Registry order = default tab order (mirrors the pre-M4 RepoView stack,
 * with the file explorer first, VS Code-style). */
export const PANEL_IDS: readonly PanelId[] = [
  "files",
  "status",
  "branches",
  "remotes",
  "stashes",
  "worktrees",
  "health",
  "submodules",
  "reflog",
  "undo",
  "history",
  "diff",
  "terminal",
  "forge",
  "actions",
  "stats",
];

/**
 * Panels that may live in the splitter tree. Every registered panel is
 * placeable, including `diff` — the workspace's main editor pane since the
 * VSCode-style layout (status/changes left, diff center, history right).
 */
export const TREE_PANELS: readonly PanelId[] = PANEL_IDS;

export interface PanelMeta {
  /** Tab / menu label. */
  label: string;
  /** Stroke path on a 24×24 grid (Feather-style), rendered aria-hidden. */
  icon: string;
}

/** Label + icon registry; the only place that knows how panels look. */
export const PANEL_META: Record<PanelId, PanelMeta> = {
  files: {
    label: "Files",
    icon: "M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2zM2 10h20",
  },
  status: {
    label: "Status",
    icon: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
  },
  branches: {
    label: "Branches",
    icon: "M6 3v12M21 6a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM18 9a9 9 0 0 1-9 9",
  },
  remotes: {
    label: "Remotes",
    icon: "M18 10h-1.26A8 8 0 1 0 9 20h9a5 5 0 0 0 0-10z",
  },
  stashes: {
    label: "Stashes",
    icon: "M21 8v13H3V8M1 3h22v5H1zM10 12h4",
  },
  worktrees: {
    label: "Worktrees",
    icon: "M20 9h-9a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2v-9a2 2 0 0 0-2-2zM5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1",
  },
  health: {
    label: "Repo",
    icon: "M12 3v18M3 12h18M5.5 5.5l13 13M18.5 5.5l-13 13",
  },
  submodules: {
    label: "Submodules",
    icon: "M4 4h6l2 2h8v12a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM9 13h6M12 10v6",
  },
  reflog: {
    label: "Reflog",
    icon: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 6v6l4 2",
  },
  undo: {
    label: "Undo",
    icon: "M9 14L4 9l5-5M20 20v-7a4 4 0 0 0-4-4H4",
  },
  history: {
    label: "History",
    icon: "M1 4v6h6M3.51 15a9 9 0 1 0 2.13-9.36L1 10M12 7v5l4 2",
  },
  diff: {
    label: "Diff",
    icon: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M9 15l6-6M9 9h.01M15 15h.01",
  },
  terminal: {
    label: "Terminal",
    icon: "M4 17l6-6-6-6M12 19h8",
  },
  forge: {
    label: "GitHub",
    icon: "M9 12h6m-6 4h4M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20z",
  },
  actions: {
    label: "Actions",
    icon: "M5 7h14M5 12h9M5 17h5",
  },
  stats: {
    label: "Stats",
    icon: "M18 20V10M12 20V4M6 20v-6",
  },
};

/** Human label for a panel (safe for unknown ids too). */
export function panelLabel(panel: PanelId): string {
  return PANEL_META[panel]?.label ?? panel;
}

/**
 * Panels pinned to the activity bar rail, in order. Everything else goes
 * into the rail's "More panels" overflow menu so the rail stays scannable
 * (UI review P1 #1). History is excluded — it keeps its own bottom button
 * because it lives in the right sidebar.
 */
export const RAIL_PANELS: readonly PanelId[] = [
  "files",
  "status",
  "branches",
  "remotes",
  "terminal",
];

/**
 * `PANEL_META` stroke paths as Iconify icon data, so menus and the command
 * palette can reuse the exact panel glyphs (stroke inherits currentColor).
 */
export function panelIconData(panel: PanelId): IconifyIcon {
  const path = PANEL_META[panel]?.icon ?? "";
  return {
    width: 24,
    height: 24,
    body: `<path d="${path}" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>`,
  };
}

// ---------------------------------------------------------------------------
// Tree schema
// ---------------------------------------------------------------------------

export interface LeafNode {
  kind: "leaf";
  id: string;
  panel: PanelId;
}

/** Tab group: stacked panels, `active` indexes into `tabs`. */
export interface TabsNode {
  kind: "tabs";
  id: string;
  tabs: PanelId[];
  active: number;
}

/**
 * Binary splitter: `a` sits left of the divider, `b` right; `ratio` is a's
 * width fraction. (The spec's `side: "left" | "right"` lives on move
 * targets — see `MoveTarget` — not on the node; splits are ordered.)
 */
export interface SplitNode {
  kind: "split";
  id: string;
  ratio: number;
  a: LayoutNode;
  b: LayoutNode;
}

export type LayoutNode = LeafNode | TabsNode | SplitNode;

/** Ratio clamp for splitter drags and validation. */
export const RATIO_MIN = 0.15;
export const RATIO_MAX = 0.85;
/** Keyboard resize step (±2% per arrow press). */
export const RATIO_STEP = 0.02;
/** Splitter-tree depth cap. */
export const MAX_DEPTH = 6;

/** Whole-workspace layout: the fixed vertical frame plus the main tree. */
export interface WorkspaceLayout {
  /** The workspace splitter tree (everything below the repo header). */
  main: LayoutNode;
  /**
   * Legacy: fraction of the old fixed bottom diff pane. Kept in the schema
   * so persisted overlays still parse; the diff is now a normal tree panel.
   */
  diffRatio: number;
  /**
   * Legacy: collapsed the old bottom commit bar in Zen presets. The commit
   * box now lives at the top of the changes sidebar; kept for overlay
   * compatibility.
   */
  autoHideCommitBar: boolean;
}

export function clampRatio(ratio: number): number {
  if (!Number.isFinite(ratio)) return 0.5;
  return Math.min(RATIO_MAX, Math.max(RATIO_MIN, ratio));
}

/** True when `value` structurally matches LayoutNode (loose, pre-parse). */
export function isLayoutNode(value: unknown): value is LayoutNode {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  if (typeof v.id !== "string" || v.id === "") return false;
  switch (v.kind) {
    case "leaf":
      return typeof v.panel === "string" && PANEL_IDS.includes(v.panel as PanelId);
    case "tabs":
      return (
        Array.isArray(v.tabs) &&
        v.tabs.every((t) => typeof t === "string" && PANEL_IDS.includes(t as PanelId)) &&
        typeof v.active === "number" &&
        Number.isInteger(v.active)
      );
    case "split":
      return (
        isLayoutNode(v.a) &&
        isLayoutNode(v.b) &&
        typeof v.ratio === "number"
      );
    default:
      return false;
  }
}

/** Parses an unknown value into a validated tree; `null` when unusable. */
export function parseTree(value: unknown): LayoutNode | null {
  if (!isLayoutNode(value)) return null;
  const tree = value as LayoutNode;
  return validateTree(tree).length === 0 ? tree : null;
}

/** Deep clone via JSON (trees are plain serializable data by construction). */
export function cloneTree(tree: LayoutNode): LayoutNode {
  return JSON.parse(JSON.stringify(tree)) as LayoutNode;
}

/** Finds a node by id (`null` when absent). */
export function nodeById(tree: LayoutNode, id: string): LayoutNode | null {
  if (tree.id === id) return tree;
  if (tree.kind === "split") {
    return nodeById(tree.a, id) ?? nodeById(tree.b, id);
  }
  return null;
}

/**
 * The node currently showing `panel`: a leaf, or the tab group containing
 * it. `null` when the panel is not in the tree.
 */
export function findPanelNode(
  tree: LayoutNode,
  panel: PanelId,
): LeafNode | TabsNode | null {
  if (tree.kind === "leaf") return tree.panel === panel ? tree : null;
  if (tree.kind === "tabs") return tree.tabs.includes(panel) ? tree : null;
  return findPanelNode(tree.a, panel) ?? findPanelNode(tree.b, panel);
}

/**
 * First sidebar holder in layout order: the leftmost leaf/tab group that
 * shows at least one non-editor panel. Used by the activity bar to pick the
 * primary sidebar without assuming a particular preset shape (`diff` by
 * itself is the center editor, never the sidebar).
 */
export function firstSidebarHolder(
  tree: LayoutNode,
): LeafNode | TabsNode | null {
  switch (tree.kind) {
    case "leaf":
      return tree.panel === "diff" ? null : tree;
    case "tabs":
      return tree.tabs.some((panel) => panel !== "diff") ? tree : null;
    case "split":
      return firstSidebarHolder(tree.a) ?? firstSidebarHolder(tree.b);
  }
}

/** All panels present in the tree, in layout order (leaf → tabs entries). */
export function visiblePanels(tree: LayoutNode): PanelId[] {
  switch (tree.kind) {
    case "leaf":
      return [tree.panel];
    case "tabs":
      return [...tree.tabs];
    case "split":
      return [...visiblePanels(tree.a), ...visiblePanels(tree.b)];
  }
}

export function treeDepth(tree: LayoutNode): number {
  if (tree.kind === "split") {
    return 1 + Math.max(treeDepth(tree.a), treeDepth(tree.b));
  }
  return 1;
}

/** All node ids in the tree (uniqueness check for validation + freshId). */
export function nodeIds(tree: LayoutNode): string[] {
  switch (tree.kind) {
    case "leaf":
    case "tabs":
      return [tree.id];
    case "split":
      return [tree.id, ...nodeIds(tree.a), ...nodeIds(tree.b)];
  }
}

/** Mints an id not used anywhere in `tree` (`base1`, `base2`, …). */
export function freshId(tree: LayoutNode, base = "n"): string {
  const used = new Set(nodeIds(tree));
  for (let i = 1; ; i++) {
    const candidate = `${base}${i}`;
    if (!used.has(candidate)) return candidate;
  }
}

/**
 * Validation problems (empty array = valid):
 * - every PanelId appears at most once across leaves and tab groups,
 * - tree depth ≤ MAX_DEPTH,
 * - split ratios within RATIO_MIN–RATIO_MAX,
 * - tab groups non-empty with `active` in range,
 * - node ids unique.
 */
export function validateTree(tree: LayoutNode): string[] {
  const problems: string[] = [];

  const seenPanels = new Map<PanelId, number>();
  for (const panel of visiblePanels(tree)) {
    seenPanels.set(panel, (seenPanels.get(panel) ?? 0) + 1);
  }
  for (const [panel, count] of seenPanels) {
    if (count > 1) problems.push(`panel "${panel}" appears ${count} times`);
  }

  const depth = treeDepth(tree);
  if (depth > MAX_DEPTH) problems.push(`tree depth ${depth} > ${MAX_DEPTH}`);

  const ids = nodeIds(tree);
  if (new Set(ids).size !== ids.length) problems.push("duplicate node ids");

  const walk = (node: LayoutNode): void => {
    if (node.kind === "split") {
      if (node.ratio < RATIO_MIN || node.ratio > RATIO_MAX) {
        problems.push(`split "${node.id}" ratio ${node.ratio} out of range`);
      }
      walk(node.a);
      walk(node.b);
    } else if (node.kind === "tabs") {
      if (node.tabs.length === 0) {
        problems.push(`tab group "${node.id}" is empty`);
      } else if (node.active < 0 || node.active >= node.tabs.length) {
        problems.push(`tab group "${node.id}" active ${node.active} out of range`);
      }
    }
  };
  walk(tree);

  return problems;
}

/** Validates a whole workspace layout (tree + diff ratio clamp). */
export function validateLayout(layout: WorkspaceLayout): string[] {
  const problems = validateTree(layout.main);
  if (layout.diffRatio < 0.05 || layout.diffRatio > 0.95) {
    problems.push(`diffRatio ${layout.diffRatio} out of range`);
  }
  return problems;
}

// ---------------------------------------------------------------------------
// Pure tree operations (all return NEW trees; inputs are never mutated)
// ---------------------------------------------------------------------------

/** Immutably replaces the node with `id` (transform returns the new node). */
export function replaceNode(
  tree: LayoutNode,
  id: string,
  transform: (node: LayoutNode) => LayoutNode,
): LayoutNode {
  if (tree.id === id) return transform(tree);
  if (tree.kind === "split") {
    const a = replaceNode(tree.a, id, transform);
    if (a !== tree.a) return { ...tree, a };
    const b = replaceNode(tree.b, id, transform);
    if (b !== tree.b) return { ...tree, b };
  }
  return tree;
}

/** Sets a split's ratio (clamped); no-op when the id is unknown. */
export function setRatio(
  tree: LayoutNode,
  splitId: string,
  ratio: number,
): LayoutNode {
  const safe = clampRatio(ratio);
  return replaceNode(tree, splitId, (node) =>
    node.kind === "split" ? { ...node, ratio: safe } : node,
  );
}

/** Applies a `{ splitId → ratio }` overlay map to a tree. */
export function applyRatios(
  tree: LayoutNode,
  ratios: Record<string, number>,
): LayoutNode {
  let out = tree;
  for (const [splitId, ratio] of Object.entries(ratios)) {
    out = setRatio(out, splitId, ratio);
  }
  return out;
}

/** Sets a tab group's active index (clamped); no-op when the id is unknown. */
export function setActiveTab(
  tree: LayoutNode,
  tabsId: string,
  index: number,
): LayoutNode {
  return replaceNode(tree, tabsId, (node) => {
    if (node.kind !== "tabs" || node.tabs.length === 0) return node;
    const active = Math.min(node.tabs.length - 1, Math.max(0, Math.round(index)));
    return active === node.active ? node : { ...node, active };
  });
}

/**
 * Removes `panel` from the tree. Removing from a tab group shrinks the
 * group (a 1-remaining group collapses to a leaf, keeping the group id);
 * removing a leaf collapses its parent split to the sibling (keeping the
 * sibling). Returns `null` only when the whole tree collapses (the last
 * panel was removed) — callers guard against emptying the workspace.
 * Returns `tree` unchanged when the panel is not present.
 */
export function removePanel(
  tree: LayoutNode,
  panel: PanelId,
): LayoutNode | null {
  switch (tree.kind) {
    case "leaf":
      return tree.panel === panel ? null : tree;

    case "tabs": {
      if (!tree.tabs.includes(panel)) return tree;
      if (tree.tabs.length === 1) return null;
      const tabs = tree.tabs.filter((t) => t !== panel);
      if (tabs.length === 1) {
        // Collapse to a leaf, keeping the group id (less id churn).
        return { kind: "leaf", id: tree.id, panel: tabs[0]! };
      }
      const oldIndex = tree.tabs.indexOf(panel);
      let active = tree.active;
      if (oldIndex < tree.active) active -= 1;
      else if (oldIndex === tree.active) active = Math.min(active, tabs.length - 1);
      return { ...tree, tabs, active };
    }

    case "split": {
      const a = removePanel(tree.a, panel);
      if (a === null) return tree.b;
      const b = removePanel(tree.b, panel);
      if (b === null) return a;
      if (a === tree.a && b === tree.b) return tree;
      return { ...tree, a, b };
    }
  }
}

/** Where to place a panel relative to an existing node. */
export type MoveTarget =
  /** New split with the panel left of `nodeId`. */
  | { kind: "before"; nodeId: string }
  /** New split with the panel right of `nodeId`. */
  | { kind: "after"; nodeId: string }
  /** Explicit side of `nodeId` (alias of before/after; spec parity). */
  | { kind: "side"; nodeId: string; side: "left" | "right" }
  /** Add into the tab group at `nodeId` (a leaf there becomes a group). */
  | { kind: "tab"; nodeId: string };

/** Actions the tab-strip context menu can request (TabGroup → RepoView). */
export type PanelGroupAction =
  /** Move the panel into the group left of its current group. */
  | "move-left-group"
  /** Move the panel into the group right of its current group. */
  | "move-right-group"
  /** Split a brand-new single-panel group to the left of the panel. */
  | "new-group-left"
  /** Split a brand-new single-panel group to the right of the panel. */
  | "new-group-right"
  /** Remove the panel from the tree (re-addable via "Add panel"). */
  | "hide";

/**
 * Inserts `panel` (not already in the tree) at `target`. Returns the tree
 * unchanged when the target node is missing, the panel is already visible,
 * or a "tab" target lands on anything but a leaf/tab group.
 */
export function insertPanel(
  tree: LayoutNode,
  panel: PanelId,
  target: MoveTarget,
): LayoutNode {
  if (findPanelNode(tree, panel)) return tree;
  const targetNode = nodeById(tree, target.nodeId);
  if (!targetNode) return tree;

  const leaf: LeafNode = { kind: "leaf", id: freshId(tree, "leaf"), panel };
  const side =
    target.kind === "side" ? target.side : target.kind === "before" ? "left" : target.kind === "after" ? "right" : null;

  if (target.kind === "tab") {
    return replaceNode(tree, target.nodeId, (node) => {
      if (node.kind === "leaf") {
        return {
          kind: "tabs",
          id: node.id,
          tabs: [node.panel, panel],
          active: 1,
        };
      }
      if (node.kind === "tabs") {
        return { ...node, tabs: [...node.tabs, panel], active: node.tabs.length };
      }
      return node;
    });
  }

  const id = freshId(tree, "split");
  const split: SplitNode =
    side === "left"
      ? { kind: "split", id, ratio: 0.5, a: leaf, b: targetNode }
      : { kind: "split", id, ratio: 0.5, a: targetNode, b: leaf };
  return replaceNode(tree, target.nodeId, () => split);
}

/**
 * Moves an existing panel to `target`. Removal happens first (a parent
 * split may collapse); if the target node no longer exists afterwards — or
 * the panel is being moved relative to itself — the tree is returned
 * unchanged except for that removal. (That means "move relative to itself"
 * can hide the panel; UIs should disable that case, tests pin it.)
 */
export function movePanel(
  tree: LayoutNode,
  panel: PanelId,
  target: MoveTarget,
): LayoutNode {
  const from = findPanelNode(tree, panel);
  if (!from) return tree;
  const targetNode = nodeById(tree, target.nodeId);
  // Moving relative to a node that IS (or contains) the panel: no structural move.
  if (targetNode && nodeContains(targetNode, panel)) {
    // Reordering inside the same tab group is the one useful self-move.
    if (target.kind === "tab" && from.kind === "tabs" && from.id === target.nodeId) {
      const removed = removePanel(tree, panel);
      return removed
        ? insertPanel(removed, panel, { kind: "tab", nodeId: target.nodeId })
        : tree;
    }
    return tree;
  }
  const removed = removePanel(tree, panel);
  if (removed === null) return tree;
  if (!nodeById(removed, target.nodeId)) return removed;
  return insertPanel(removed, panel, target);
}

/** True when `node` (or any descendant) currently shows `panel`. */
export function nodeContains(node: LayoutNode, panel: PanelId): boolean {
  return findPanelNode(node, panel) !== null;
}

/**
 * Wraps the whole tree with `panel` as a new right-hand column. The
 * collapsed state of a sidebar is "not in the tree"; re-opening it with
 * this helper restores it on the right edge (VSCode-style secondary
 * sidebar) regardless of how the rest of the workspace is arranged.
 */
export function wrapPanelRight(
  tree: LayoutNode,
  panel: PanelId,
  ratio = 0.7,
): LayoutNode {
  return {
    kind: "split",
    id: freshId(tree, "split"),
    ratio: clampRatio(ratio),
    a: tree,
    b: { kind: "leaf", id: freshId(tree, "leaf"), panel },
  };
}

/** Result of moving a panel into an adjacent tab group. */
export interface MergeOutcome {
  tree: LayoutNode;
  ok: boolean;
  reason?: "no-left-sibling" | "no-right-sibling" | "sibling-not-a-group";
}

function moveToSiblingGroup(
  tree: LayoutNode,
  panel: PanelId,
  side: "left" | "right",
): MergeOutcome {
  const reason = side === "left" ? "no-left-sibling" : "no-right-sibling";
  const holder = findPanelNode(tree, panel);
  if (!holder) return { tree, ok: false, reason };
  const sibling =
    side === "left"
      ? leftSiblingOf(tree, holder.id)
      : rightSiblingOf(tree, holder.id);
  if (!sibling) return { tree, ok: false, reason };
  if (sibling.kind === "split") {
    return { tree, ok: false, reason: "sibling-not-a-group" };
  }
  const removed = removePanel(tree, panel);
  if (removed === null || !nodeById(removed, sibling.id)) {
    return { tree, ok: false, reason };
  }
  return {
    tree: insertPanel(removed, panel, { kind: "tab", nodeId: sibling.id }),
    ok: true,
  };
}

/**
 * "Move to right group": moves `panel` into the group immediately right of
 * the group currently holding it (the sibling child of the parent split).
 * Joining a leaf turns both into a tab group; joining a tab group appends.
 * Fails when the holder has no right sibling or the sibling is a split.
 */
export function moveToRightGroup(
  tree: LayoutNode,
  panel: PanelId,
): MergeOutcome {
  return moveToSiblingGroup(tree, panel, "right");
}

/**
 * "Move to left group": mirror of {@link moveToRightGroup} for the sibling
 * rendered left of the holder — the way back for a panel moved right.
 */
export function moveToLeftGroup(
  tree: LayoutNode,
  panel: PanelId,
): MergeOutcome {
  return moveToSiblingGroup(tree, panel, "left");
}

/**
 * The sibling child that renders right of `nodeId` (its parent split's
 * other child), or `null` when `nodeId` is the root or a right child.
 */
export function rightSiblingOf(
  tree: LayoutNode,
  nodeId: string,
): LayoutNode | null {
  if (tree.kind !== "split") return null;
  if (tree.a.id === nodeId) return tree.b;
  return rightSiblingOf(tree.a, nodeId) ?? rightSiblingOf(tree.b, nodeId);
}

/**
 * The sibling child that renders left of `nodeId`, or `null` when `nodeId`
 * is the root or a left child.
 */
export function leftSiblingOf(
  tree: LayoutNode,
  nodeId: string,
): LayoutNode | null {
  if (tree.kind !== "split") return null;
  if (tree.b.id === nodeId) return tree.a;
  return leftSiblingOf(tree.a, nodeId) ?? leftSiblingOf(tree.b, nodeId);
}

/** First tab group in the tree (leftmost), or `null` when there is none. */
export function firstTabsId(tree: LayoutNode): string | null {
  if (tree.kind === "tabs") return tree.id;
  if (tree.kind === "split") return firstTabsId(tree.a) ?? firstTabsId(tree.b);
  return null;
}

// ---------------------------------------------------------------------------
// JSON round-trip
// ---------------------------------------------------------------------------

export function treeToJSON(tree: LayoutNode): string {
  return JSON.stringify(tree);
}

// ---------------------------------------------------------------------------
// Default layout + presets
// ---------------------------------------------------------------------------

export interface LayoutPreset {
  id: string;
  name: string;
  tree: LayoutNode;
  /** Legacy diff pane fraction (default 0.4; see `WorkspaceLayout`). */
  diffRatio?: number;
  /** Legacy Zen flag (default false; see `WorkspaceLayout`). */
  autoHideCommitBar?: boolean;
}

/** Materializes a preset into a full workspace layout. */
export function presetToLayout(preset: LayoutPreset): WorkspaceLayout {
  return {
    main: cloneTree(preset.tree),
    diffRatio: preset.diffRatio ?? 0.4,
    autoHideCommitBar: preset.autoHideCommitBar ?? false,
  };
}

// "terminal" joined the pre-M4 tab stack in M5 (lane G2) — appended last so
// every earlier tab keeps its index. "stats" joined in M7 (lane I1), placed
// before "terminal" per the M7 contracts section.
const ALL_LEFT_TABS: PanelId[] = [
  "files",
  "status",
  "branches",
  "remotes",
  "stashes",
  "worktrees",
  "reflog",
  "undo",
  "stats",
  "terminal",
];

/**
 * VSCode-style workspace: a narrow left sidebar (changes / branches / …)
 * beside a main editor pane (working-copy diff or commit detail) and a
 * secondary commit-history sidebar on the right. The activity bar switches
 * the primary sidebar and either sidebar can collapse to a rail without
 * leaving the tree (the layout store stores `leftCollapsed`/`rightCollapsed`
 * and SplitContainer hides the pane while keeping it mounted). The commit box
 * sits at the top of the status panel.
 */
export const DEFAULT_LAYOUT: WorkspaceLayout = {
  main: {
    kind: "split",
    id: "split-main",
    ratio: 0.24,
    a: { kind: "tabs", id: "tabs-left", tabs: [...ALL_LEFT_TABS], active: 1 },
    b: {
      kind: "split",
      id: "split-main-right",
      ratio: 0.68,
      a: { kind: "leaf", id: "leaf-editor", panel: "diff" },
      b: { kind: "leaf", id: "leaf-history", panel: "history" },
    },
  },
  diffRatio: 0.4,
  autoHideCommitBar: false,
};

/** Built-in presets (user presets layer on top in the layout store). */
export const DEFAULT_PRESETS: LayoutPreset[] = [
  {
    id: "default",
    name: "Default",
    tree: cloneTree(DEFAULT_LAYOUT.main),
    diffRatio: 0.4,
    autoHideCommitBar: false,
  },
  {
    id: "history-focus",
    name: "History Focus",
    tree: {
      kind: "split",
      id: "split-hf",
      ratio: RATIO_MIN,
      a: { kind: "tabs", id: "tabs-hf", tabs: ["status"], active: 0 },
      b: {
        kind: "split",
        id: "split-hf-right",
        ratio: 0.5,
        a: { kind: "leaf", id: "leaf-hf-editor", panel: "diff" },
        b: { kind: "leaf", id: "leaf-hf-history", panel: "history" },
      },
    },
    diffRatio: 0.4,
  },
  {
    id: "zen",
    name: "Zen",
    tree: { kind: "leaf", id: "leaf-zen-editor", panel: "diff" },
    diffRatio: 0.4,
    autoHideCommitBar: true,
  },
  {
    id: "review",
    name: "Review",
    tree: {
      kind: "split",
      id: "split-review",
      ratio: 0.22,
      a: { kind: "tabs", id: "tabs-review", tabs: [...ALL_LEFT_TABS], active: 1 },
      b: {
        kind: "split",
        id: "split-review-right",
        ratio: 0.72,
        a: { kind: "leaf", id: "leaf-review-editor", panel: "diff" },
        b: { kind: "leaf", id: "leaf-review-history", panel: "history" },
      },
    },
    diffRatio: 0.6,
  },
];

/** Fallback preset id used when nothing else resolves. */
export const DEFAULT_PRESET_ID = "default";
