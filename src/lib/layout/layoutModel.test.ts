/**
 * Unit tests for the pure layout model (`$lib/layout/layoutModel`):
 * tree parsing/validation, visibility, and every structural operation
 * (remove/insert/move, ratios, tab groups, right-group merge, presets).
 */

import { describe, expect, it } from "vitest";
import {
  applyRatios,
  cloneTree,
  DEFAULT_LAYOUT,
  DEFAULT_PRESETS,
  DEFAULT_PRESET_ID,
  firstTabsId,
  findPanelNode,
  freshId,
  insertPanel,
  isLayoutNode,
  MAX_DEPTH,
  movePanel,
  moveToRightGroup,
  nodeById,
  panelLabel,
  PANEL_IDS,
  parseTree,
  presetToLayout,
  removePanel,
  RATIO_MAX,
  RATIO_MIN,
  rightSiblingOf,
  setActiveTab,
  setRatio,
  treeDepth,
  treeToJSON,
  validateLayout,
  validateTree,
  visiblePanels,
  type LayoutNode,
  type PanelId,
  type SplitNode,
  type TabsNode,
} from "./layoutModel";

// -- fixtures -----------------------------------------------------------------

function leaf(id: string, panel: PanelId): LayoutNode {
  return { kind: "leaf", id, panel };
}

const STATUS = leaf("n1", "status");
const HISTORY = leaf("n2", "history");

function split(id: string, ratio: number, a: LayoutNode, b: LayoutNode): SplitNode {
  return { kind: "split", id, ratio, a, b };
}

function tabs(id: string, panels: PanelId[], active = 0): TabsNode {
  return { kind: "tabs", id, tabs: panels, active };
}

/** status | branches — the simplest tree with one split. */
const SIMPLE = split("s1", 0.3, leaf("n1", "status"), leaf("n2", "branches"));

// -- schema / parsing ---------------------------------------------------------

describe("tree schema + parsing", () => {
  it("recognizes valid nodes and rejects junk", () => {
    expect(isLayoutNode(STATUS)).toBe(true);
    expect(isLayoutNode(tabs("t", ["status"]))).toBe(true);
    expect(isLayoutNode(SIMPLE)).toBe(true);
    expect(isLayoutNode(null)).toBe(false);
    expect(isLayoutNode({ kind: "leaf", id: "x", panel: "bogus" })).toBe(false);
    expect(isLayoutNode({ kind: "nope", id: "x" })).toBe(false);
    expect(isLayoutNode({ kind: "split", id: "x", ratio: 0.5 })).toBe(false);
  });

  it("round-trips through JSON", () => {
    const tree = DEFAULT_LAYOUT.main;
    const parsed = parseTree(JSON.parse(treeToJSON(tree)));
    expect(parsed).toEqual(tree);
  });

  it("parseTree rejects structurally-valid but rule-breaking trees", () => {
    // Duplicate panel across two leaves.
    const dup = split("s", 0.5, leaf("a", "status"), leaf("b", "status"));
    expect(parseTree(dup)).toBeNull();
    // Out-of-range ratio.
    expect(parseTree(split("s", 0.99, STATUS, HISTORY))).toBeNull();
  });

  it("mints unique ids", () => {
    expect(freshId(SIMPLE, "n")).toBe("n3");
    expect(freshId(SIMPLE, "s")).toBe("s2");
    expect(freshId({ ...SIMPLE, id: "n3" }, "n")).toBe("n4");
  });
});

// -- queries ------------------------------------------------------------------

describe("queries", () => {
  it("finds nodes by id", () => {
    expect(nodeById(DEFAULT_LAYOUT.main, "tabs-left")).not.toBeNull();
    expect(nodeById(DEFAULT_LAYOUT.main, "nope")).toBeNull();
  });

  it("finds the node showing a panel (leaf or tab group)", () => {
    const tree = DEFAULT_LAYOUT.main;
    expect(findPanelNode(tree, "history")).toEqual({
      kind: "leaf",
      id: "leaf-history",
      panel: "history",
    });
    const group = findPanelNode(tree, "status");
    expect(group?.kind).toBe("tabs");
    expect(findPanelNode(tree, "diff")).toBeNull();
  });

  it("lists visible panels in layout order", () => {
    expect(visiblePanels(DEFAULT_LAYOUT.main)).toEqual([
      "status",
      "branches",
      "remotes",
      "stashes",
      "worktrees",
      "reflog",
      "undo",
      "stats",
      "terminal",
      "history",
    ]);
  });

  it("computes depth, right siblings and the first tab group", () => {
    expect(treeDepth(STATUS)).toBe(1);
    expect(treeDepth(DEFAULT_LAYOUT.main)).toBe(2);
    expect(rightSiblingOf(SIMPLE, "n1")).toBe(SIMPLE.b);
    expect(rightSiblingOf(SIMPLE, "n2")).toBeNull();
    expect(firstTabsId(DEFAULT_LAYOUT.main)).toBe("tabs-left");
    expect(firstTabsId(HISTORY)).toBeNull();
  });

  it("labels every registered panel", () => {
    for (const id of PANEL_IDS) {
      expect(panelLabel(id)).not.toBe(id);
    }
    // Unknown (cast) ids fall through to the raw id.
    expect(panelLabel("bogus" as never)).toBe("bogus");
  });
});

// -- validation ---------------------------------------------------------------

describe("validateTree", () => {
  it("accepts the default layout", () => {
    expect(validateTree(DEFAULT_LAYOUT.main)).toEqual([]);
    expect(validateLayout(DEFAULT_LAYOUT)).toEqual([]);
  });

  it("flags duplicate panels", () => {
    const dup = split("s", 0.5, leaf("a", "status"), tabs("t", ["status", "history"]));
    expect(validateTree(dup).some((p) => p.includes('"status"'))).toBe(true);
  });

  it("flags out-of-range ratios, empty groups, bad active, deep trees, duplicate ids", () => {
    expect(
      validateTree(split("s", 0.9, STATUS, HISTORY)).some((p) => p.includes("ratio")),
    ).toBe(true);
    expect(
      validateTree({ kind: "tabs", id: "t", tabs: [], active: 0 }).some((p) =>
        p.includes("empty"),
      ),
    ).toBe(true);
    expect(
      validateTree(tabs("t", ["status"], 5)).some((p) => p.includes("active")),
    ).toBe(true);

    // Depth 8 (cap is 6): chain splits.
    let deep: LayoutNode = STATUS;
    for (let i = 0; i < 7; i++) deep = split(`d${i}`, 0.5, deep, HISTORY);
    expect(treeDepth(deep)).toBe(MAX_DEPTH + 2);
    expect(validateTree(deep).some((p) => p.includes("depth 8"))).toBe(true);

    expect(
      validateTree(split("s", 0.5, STATUS, STATUS)).some((p) =>
        p.includes("duplicate node ids"),
      ),
    ).toBe(true);
  });

  it("flags a bad diffRatio in validateLayout", () => {
    expect(
      validateLayout({ ...DEFAULT_LAYOUT, diffRatio: 0.99 }).some((p) =>
        p.includes("diffRatio"),
      ),
    ).toBe(true);
  });
});

// -- ratios + tab activation --------------------------------------------------

describe("setRatio / applyRatios / setActiveTab", () => {
  it("clamps and only touches the targeted split (immutably)", () => {
    const out = setRatio(SIMPLE, "s1", 0.99);
    expect((out as SplitNode).ratio).toBe(RATIO_MAX);
    expect((out as SplitNode).a).toBe(SIMPLE.a); // untouched subtree kept by reference
    expect(SIMPLE.ratio).toBe(0.3);

    expect((setRatio(SIMPLE, "s1", -1) as SplitNode).ratio).toBe(RATIO_MIN);
    expect((setRatio(SIMPLE, "s1", NaN) as SplitNode).ratio).toBe(0.5);
    expect(setRatio(SIMPLE, "missing", 0.7)).toBe(SIMPLE);
  });

  it("applies ratio overlays keyed by split id", () => {
    const out = applyRatios(SIMPLE, { s1: 0.6, bogus: 0.1 });
    expect((out as SplitNode).ratio).toBe(0.6);
  });

  it("clamps tab activation; no-op for unknown groups", () => {
    const group = tabs("t", ["status", "branches", "remotes"]);
    expect((setActiveTab(group, "t", 9) as TabsNode).active).toBe(2);
    expect((setActiveTab(group, "t", -3) as TabsNode).active).toBe(0);
    expect(setActiveTab(group, "other", 1)).toBe(group);
  });
});

// -- remove -------------------------------------------------------------------

describe("removePanel", () => {
  it("collapses a split to the surviving sibling", () => {
    const out = removePanel(SIMPLE, "status");
    // The surviving child is returned by reference.
    expect(out).toBe(SIMPLE.b);
  });

  it("shrinks tab groups; pairs collapse to a leaf keeping the group id", () => {
    const group = tabs("t", ["status", "branches", "remotes"], 2);
    const shrunk = removePanel(group, "remotes") as TabsNode;
    expect(shrunk.tabs).toEqual(["status", "branches"]);
    expect(shrunk.active).toBe(1);

    const pair = removePanel(tabs("t", ["status", "branches"], 1), "branches");
    expect(pair).toEqual({ kind: "leaf", id: "t", panel: "status" });
  });

  it("fixes active index when an earlier tab is removed", () => {
    const group = tabs("t", ["status", "branches", "remotes"], 2);
    const shrunk = removePanel(group, "status") as TabsNode;
    expect(shrunk.tabs).toEqual(["branches", "remotes"]);
    expect(shrunk.active).toBe(1);
  });

  it("returns null only when the whole tree collapses", () => {
    expect(removePanel(STATUS, "status")).toBeNull();
    expect(removePanel(tabs("t", ["status"]), "status")).toBeNull();
  });

  it("leaves trees without the panel untouched (same reference)", () => {
    expect(removePanel(SIMPLE, "history")).toBe(SIMPLE);
  });
});

// -- insert / move ------------------------------------------------------------

describe("insertPanel / movePanel", () => {
  it("wraps a target in a new split before/after (side aliases)", () => {
    const before = insertPanel(SIMPLE, "history", { kind: "before", nodeId: "n2" });
    expect(visiblePanels(before)).toEqual(["status", "history", "branches"]);
    expect(validateTree(before)).toEqual([]);

    const side = insertPanel(SIMPLE, "history", {
      kind: "side",
      nodeId: "n2",
      side: "left",
    });
    expect(visiblePanels(side)).toEqual(["status", "history", "branches"]);

    const after = insertPanel(STATUS, "branches", { kind: "after", nodeId: "n1" });
    expect(visiblePanels(after)).toEqual(["status", "branches"]);
  });

  it("tab-insert converts a leaf to a group and appends to groups", () => {
    const made = insertPanel(STATUS, "branches", { kind: "tab", nodeId: "n1" }) as TabsNode;
    expect(made.kind).toBe("tabs");
    expect(made.tabs).toEqual(["status", "branches"]);
    expect(made.active).toBe(1);

    const appended = insertPanel(made, "remotes", { kind: "tab", nodeId: made.id }) as TabsNode;
    expect(appended.tabs).toEqual(["status", "branches", "remotes"]);
    expect(appended.active).toBe(2);
  });

  it("refuses impossible inserts (visible panel, missing target, tab on split)", () => {
    expect(insertPanel(SIMPLE, "status", { kind: "tab", nodeId: "n2" })).toBe(SIMPLE);
    expect(insertPanel(SIMPLE, "history", { kind: "after", nodeId: "gone" })).toBe(SIMPLE);
    expect(
      insertPanel(SIMPLE, "history", { kind: "tab", nodeId: "s1" }),
    ).toBe(SIMPLE);
  });

  it("moves a panel before/after another (removal collapses the old spot)", () => {
    const tree = split(
      "outer",
      0.5,
      split("inner", 0.5, leaf("n1", "status"), leaf("n2", "branches")),
      leaf("n3", "history"),
    );
    const moved = movePanel(tree, "branches", { kind: "after", nodeId: "n3" });
    expect(visiblePanels(moved)).toEqual(["status", "history", "branches"]);
    expect(validateTree(moved)).toEqual([]);
    // The emptied inner split collapsed away (to its surviving child).
    expect(nodeById(moved, "inner")).toBeNull();
    expect(nodeById(moved, "n1")).not.toBeNull();
  });

  it("supports the explicit side target", () => {
    const moved = movePanel(SIMPLE, "branches", {
      kind: "side",
      nodeId: "n1",
      side: "right",
    });
    expect(visiblePanels(moved)).toEqual(["status", "branches"]);
    expect((moved as SplitNode).a).toEqual(STATUS);
  });

  it("reorders within the same tab group", () => {
    const group = tabs("t", ["status", "branches", "remotes"], 0);
    const moved = movePanel(group, "remotes", { kind: "tab", nodeId: "t" }) as TabsNode;
    expect(moved.tabs).toEqual(["status", "branches", "remotes"]);
    expect(moved.active).toBe(2);
  });

  it("no-ops self-referential moves", () => {
    expect(movePanel(SIMPLE, "status", { kind: "before", nodeId: "n1" })).toBe(SIMPLE);
    // Moving relative to an ancestor that contains the panel: no-op.
    expect(movePanel(SIMPLE, "status", { kind: "after", nodeId: "s1" })).toBe(SIMPLE);
    // Moving a panel that is not in the tree: no-op.
    expect(movePanel(SIMPLE, "history", { kind: "after", nodeId: "n1" })).toBe(SIMPLE);
  });
});

// -- right-group merge ----------------------------------------------------------

describe("moveToRightGroup", () => {
  const TREE = split(
    "s",
    0.5,
    tabs("t-left", ["status", "branches"]),
    tabs("t-right", ["history"], 0),
  );

  it("moves a panel into the right sibling group", () => {
    const out = moveToRightGroup(TREE, "status");
    expect(out.ok).toBe(true);
    expect(visiblePanels(out.tree)).toEqual(["branches", "history", "status"]);
    const right = nodeById(out.tree, "t-right") as TabsNode;
    expect(right.tabs).toEqual(["history", "status"]);
    expect(right.active).toBe(1);
  });

  it("joins a leaf sibling into a new tab group", () => {
    const tree = split("s", 0.5, leaf("n1", "status"), leaf("n2", "history"));
    const out = moveToRightGroup(tree, "status");
    expect(out.ok).toBe(true);
    const merged = nodeById(out.tree, "n2") as TabsNode;
    expect(merged.kind).toBe("tabs");
    expect(merged.tabs).toEqual(["history", "status"]);
  });

  it("fails without a right sibling or when the sibling is a split", () => {
    // history is the rightmost child of the root split → no right sibling.
    expect(moveToRightGroup(TREE, "history").ok).toBe(false);
    // branches is the right child of the SIMPLE split → no right sibling.
    expect(moveToRightGroup(SIMPLE, "branches").ok).toBe(false);

    const nested = split(
      "s",
      0.5,
      leaf("n1", "status"),
      split("inner", 0.5, leaf("n2", "branches"), leaf("n3", "history")),
    );
    expect(moveToRightGroup(nested, "status").reason).toBe("sibling-not-a-group");
  });
});

// -- defaults + presets ---------------------------------------------------------

describe("default layout + presets", () => {
  it("default layout mirrors the pre-M4 RepoView", () => {
    expect(DEFAULT_LAYOUT.main.kind).toBe("split");
    const root = DEFAULT_LAYOUT.main as SplitNode;
    expect(root.a.kind).toBe("tabs");
    expect((root.a as TabsNode).tabs).toEqual([
      "status",
      "branches",
      "remotes",
      "stashes",
      "worktrees",
      "reflog",
      "undo",
      "stats",
      "terminal",
    ]);
    expect((root.a as TabsNode).active).toBe(0);
    expect(root.b).toEqual({ kind: "leaf", id: "leaf-history", panel: "history" });
    expect(DEFAULT_LAYOUT.diffRatio).toBe(0.4);
    expect(DEFAULT_LAYOUT.autoHideCommitBar).toBe(false);
    expect(validateLayout(DEFAULT_LAYOUT)).toEqual([]);
  });

  it("ships the four documented presets, all valid and unique", () => {
    expect(DEFAULT_PRESETS.map((p) => p.id)).toEqual([
      "default",
      "history-focus",
      "zen",
      "review",
    ]);
    for (const preset of DEFAULT_PRESETS) {
      expect(validateTree(preset.tree)).toEqual([]);
    }
  });

  it("preset shapes match their names", () => {
    const byId = (id: string) => DEFAULT_PRESETS.find((p) => p.id === id)!;

    // History Focus: history wide, left collapsed to status only.
    const hf = byId("history-focus");
    expect(visiblePanels(hf.tree)).toEqual(["status", "history"]);
    expect((hf.tree as SplitNode).ratio).toBe(RATIO_MIN);

    // Zen: history only + auto-hiding commit bar.
    const zen = byId("zen");
    expect(visiblePanels(zen.tree)).toEqual(["history"]);
    expect(zen.autoHideCommitBar).toBe(true);

    // Review: diff pane at 60%.
    const review = byId("review");
    expect(review.diffRatio).toBe(0.6);
  });

  it("presetToLayout materializes defaults", () => {
    const layout = presetToLayout({
      id: "x",
      name: "X",
      tree: HISTORY,
    });
    expect(layout).toEqual({
      main: HISTORY,
      diffRatio: 0.4,
      autoHideCommitBar: false,
    });
    expect(layout.main).not.toBe(HISTORY); // cloned
  });

  it("default preset id resolves", () => {
    expect(DEFAULT_PRESET_ID).toBe("default");
    expect(cloneTree(STATUS)).toEqual(STATUS);
  });
});
