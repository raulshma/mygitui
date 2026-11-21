/**
 * Unit tests for the §10 Stacked fallback plan (`$lib/layout/stackedPlan`):
 * the bottom-bar inventory (5 primaries + More), the display-label
 * overrides, and the full-screen host chrome classes/attributes.
 */

import { describe, expect, it } from "vitest";
import {
  STACKED_DEFAULT_VIEW,
  STACKED_PRIMARY_VIEWS,
  isStackedPrimaryView,
  stackedHostAttrs,
  stackedMorePanels,
  stackedPaneClass,
  stackedViewLabel,
} from "./stackedPlan";
import { PANEL_IDS, TREE_PANELS } from "./layoutModel";
import { WIDTH_MODES } from "./widthMode.svelte";

describe("STACKED_PRIMARY_VIEWS (§10 matrix bar inventory)", () => {
  it("is the five matrix views, in bar order", () => {
    expect(STACKED_PRIMARY_VIEWS).toEqual([
      "files",
      "diff",
      "status",
      "history",
      "forge",
    ]);
  });

  it("keeps every primary view a registered panel id", () => {
    for (const view of STACKED_PRIMARY_VIEWS) {
      expect(PANEL_IDS).toContain(view);
    }
  });
});

describe("isStackedPrimaryView", () => {
  it("accepts exactly the five primary views", () => {
    for (const view of STACKED_PRIMARY_VIEWS) {
      expect(isStackedPrimaryView(view)).toBe(true);
    }
  });

  it("rejects rail panels that belong behind More", () => {
    expect(isStackedPrimaryView("branches")).toBe(false);
    expect(isStackedPrimaryView("remotes")).toBe(false);
    expect(isStackedPrimaryView("terminal")).toBe(false);
    expect(isStackedPrimaryView("assistant")).toBe(false);
  });
});

describe("stackedMorePanels", () => {
  it("keeps every non-primary rail destination, in given order", () => {
    // The ActivityBar's panel list is TREE_PANELS minus diff/history;
    // More = that list minus the primary views it already contains.
    const railPanels = TREE_PANELS.filter(
      (panel) => panel !== "diff" && panel !== "history",
    );
    expect(stackedMorePanels(railPanels)).toEqual([
      "branches",
      "remotes",
      "stashes",
      "worktrees",
      "health",
      "submodules",
      "reflog",
      "undo",
      "terminal",
      "actions",
      "stats",
      "assistant",
    ]);
  });

  it("is exhaustive: primary + More cover the whole panel registry", () => {
    const railPanels = TREE_PANELS.filter(
      (panel) => panel !== "diff" && panel !== "history",
    );
    const reachable = new Set([...STACKED_PRIMARY_VIEWS, ...stackedMorePanels(railPanels)]);
    for (const panel of PANEL_IDS) {
      expect(reachable.has(panel)).toBe(true);
    }
  });
});

describe("stackedViewLabel", () => {
  it("names the editor view 'Editor' (§10 matrix row)", () => {
    expect(stackedViewLabel("diff")).toBe("Editor");
  });

  it("falls back to the panel registry label", () => {
    expect(stackedViewLabel("files")).toBe("Files");
    expect(stackedViewLabel("status")).toBe("Status");
    expect(stackedViewLabel("history")).toBe("History");
    expect(stackedViewLabel("forge")).toBe("GitHub");
    expect(stackedViewLabel("branches")).toBe("Branches");
  });
});

describe("STACKED_DEFAULT_VIEW", () => {
  it("starts on the editor (the workspace's main surface)", () => {
    expect(STACKED_DEFAULT_VIEW).toBe("diff");
    expect(isStackedPrimaryView(STACKED_DEFAULT_VIEW)).toBe(true);
  });
});

describe("stackedPaneClass", () => {
  it("fills the containing block (inset-0) — a full-screen route", () => {
    const open = stackedPaneClass(true);
    expect(open).toContain("absolute");
    expect(open).toContain("inset-0");
    expect(open).toContain("visible");
  });

  it("keeps a closed view mounted but invisible (keep-alive)", () => {
    const closed = stackedPaneClass(false);
    expect(closed).toContain("invisible");
    // Visibility-driven (never `display: none`, which would kill the
    // keep-alive content): no standalone `hidden` class.
    expect(closed).not.toMatch(/(^|\s)hidden(\s|$)/);
    // No slide transform: a full-screen route has no edge, and a
    // transform would make the host the containing block for `fixed`
    // panel UI (in-panel menus).
    expect(closed).not.toContain("translate");
    expect(stackedPaneClass(true)).not.toContain("translate");
  });

  it("stretches panel content (flex-1 children) like the docked hosts", () => {
    const open = stackedPaneClass(true);
    expect(open).toContain("flex-col");
    expect(open).toContain("[&>*]:flex-1");
  });
});

describe("stackedHostAttrs", () => {
  it("marks the host as a labeled full-screen region", () => {
    const attrs = stackedHostAttrs({
      key: "diff",
      open: true,
      label: "Editor view",
    });
    expect(attrs.role).toBe("group");
    expect(attrs["aria-label"]).toBe("Editor view");
    expect(attrs["data-stacked-view"]).toBe("diff");
    expect(attrs["data-state"]).toBe("open");
    expect(attrs.tabindex).toBe("-1");
  });

  it("reports closed state without dialog semantics", () => {
    const attrs = stackedHostAttrs({
      key: "status",
      open: false,
      label: "Status view",
    });
    expect(attrs["data-state"]).toBe("closed");
    expect(attrs.role).not.toBe("dialog");
  });
});

describe("mode coverage", () => {
  it("the stacked plan is keyed off one width mode only", () => {
    // Sanity: the mode ladder still carries 'stacked' last — the stacked
    // machinery reads it from widthMode.current.
    expect(WIDTH_MODES[WIDTH_MODES.length - 1]).toBe("stacked");
  });
});
