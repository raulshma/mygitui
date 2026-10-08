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
 * (clamped to RATIO_MIN–RATIO_MAX). Deeper vertical nesting is not needed
 * for v1 because the workspace root fixes the vertical arrangement (header
 * / main split tree / commit bar / diff pane) and tab groups absorb the
 * rest — see `WorkspaceLayout` + `DEFAULT_LAYOUT`.
 *
 * Every node carries a unique string `id` so operations (move, hide, ratio
 * drag) can target nodes from UI events and ratio overlays can key by split
 * id. Ids are opaque; `freshId` mints unused ones.
 */

// ---------------------------------------------------------------------------
// Panel registry
// ---------------------------------------------------------------------------

/** Every panel the layout system can place. */
export type PanelId =
  | "status"
  | "branches"
  | "remotes"
  | "stashes"
  | "worktrees"
  | "reflog"
  | "undo"
  | "history"
  | "diff";

/** Registry order = default tab order (mirrors the pre-M4 RepoView stack). */
export const PANEL_IDS: readonly PanelId[] = [
  "status",
  "branches",
  "remotes",
  "stashes",
  "worktrees",
  "reflog",
  "undo",
  "history",
  "diff",
];

/**
 * Panels that may live in the splitter tree. `diff` is a fixed bottom pane
 * (with collapse + popout), never a tree leaf, but stays in the PanelId
 * union so the popout query contract and panel metadata stay uniform.
 */
export const TREE_PANELS: readonly PanelId[] = PANEL_IDS.filter(
  (p) => p !== "diff",
);

export interface PanelMeta {
  /** Tab / menu label. */
  label: string;
  /** Stroke path on a 24×24 grid (Feather-style), rendered aria-hidden. */
  icon: string;
}

/** Label + icon registry; the only place that knows how panels look. */
export const PANEL_META: Record<PanelId, PanelMeta> = {
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
};

/** Human label for a panel (safe for unknown ids too). */
export function panelLabel(panel: PanelId): string {
  return PANEL_META[panel]?.label ?? panel;
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
  /** Splitter tree between the repo header and the commit bar. */
  main: LayoutNode;
  /** Diff pane height as a fraction of the workspace (when open). */
  diffRatio: number;
  /** Zen flag: commit bar collapses to a slim auto-expanding strip. */
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
  /** Move the panel into the group right of its current group. */
  | "move-right-group"
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

/** Merges the panel of `nodeId` into its right sibling group (or vice versa?). */
export interface MergeOutcome {
  tree: LayoutNode;
  ok: boolean;
  reason?: "no-right-sibling" | "sibling-not-a-group";
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
  const holder = findPanelNode(tree, panel);
  if (!holder) return { tree, ok: false, reason: "no-right-sibling" };
  const sibling = rightSiblingOf(tree, holder.id);
  if (!sibling) return { tree, ok: false, reason: "no-right-sibling" };
  if (sibling.kind === "split") {
    return { tree, ok: false, reason: "sibling-not-a-group" };
  }
  const removed = removePanel(tree, panel);
  if (removed === null || !nodeById(removed, sibling.id)) {
    return { tree, ok: false, reason: "no-right-sibling" };
  }
  return { tree: insertPanel(removed, panel, { kind: "tab", nodeId: sibling.id }), ok: true };
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
  /** Diff pane fraction (default 0.4). */
  diffRatio?: number;
  /** Zen flag (default false). */
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

const ALL_LEFT_TABS: PanelId[] = [
  "status",
  "branches",
  "remotes",
  "stashes",
  "worktrees",
  "reflog",
  "undo",
];

/**
 * Mirrors the pre-M4 RepoView: a left tabbed stack (status / branches /
 * remotes / stashes / worktrees / reflog / undo) beside history, with the
 * diff as the fixed bottom pane at 40%.
 */
export const DEFAULT_LAYOUT: WorkspaceLayout = {
  main: {
    kind: "split",
    id: "split-main",
    ratio: 0.28,
    a: { kind: "tabs", id: "tabs-left", tabs: [...ALL_LEFT_TABS], active: 0 },
    b: { kind: "leaf", id: "leaf-history", panel: "history" },
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
      b: { kind: "leaf", id: "leaf-hf-history", panel: "history" },
    },
    diffRatio: 0.4,
  },
  {
    id: "zen",
    name: "Zen",
    tree: { kind: "leaf", id: "leaf-zen-history", panel: "history" },
    diffRatio: 0.4,
    autoHideCommitBar: true,
  },
  {
    id: "review",
    name: "Review",
    tree: {
      kind: "split",
      id: "split-review",
      ratio: 0.32,
      a: { kind: "tabs", id: "tabs-review", tabs: [...ALL_LEFT_TABS], active: 0 },
      b: { kind: "leaf", id: "leaf-review-history", panel: "history" },
    },
    diffRatio: 0.6,
  },
];

/** Fallback preset id used when nothing else resolves. */
export const DEFAULT_PRESET_ID = "default";
