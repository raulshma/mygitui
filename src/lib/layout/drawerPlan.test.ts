/**
 * Unit tests for the §10 drawer plan (`$lib/layout/drawerPlan`): the
 * per-mode matrix (which panel drawerizes at which width mode and from
 * which edge), the fresh-install open-state fallbacks, the workspace
 * emptying guard in `planDrawerPanels`, and the shared host-chrome classes.
 */

import { describe, expect, it } from "vitest";
import {
  DRAWER_PANELS,
  DRAWER_PREF_KEY,
  drawerFallbackOpen,
  drawerPaneClass,
  drawerPanelLabel,
  drawerSide,
  isDrawerPanel,
  planDrawerPanels,
} from "./drawerPlan";
import { WIDTH_MODES } from "./widthMode.svelte";

describe("drawerSide (§10 matrix)", () => {
  it("drawerizes FILES from the left in Medium and Narrow", () => {
    expect(drawerSide("files", "wide")).toBeNull();
    expect(drawerSide("files", "medium")).toBe("left");
    expect(drawerSide("files", "narrow")).toBe("left");
    // Stacked keeps the docked behavior until the stacked fallback lands.
    expect(drawerSide("files", "stacked")).toBeNull();
  });

  it("keeps STATUS docked in Medium and drawerizes it from the right in Narrow", () => {
    expect(drawerSide("status", "wide")).toBeNull();
    expect(drawerSide("status", "medium")).toBeNull();
    expect(drawerSide("status", "narrow")).toBe("right");
    expect(drawerSide("status", "stacked")).toBeNull();
  });

  it("drawerizes HISTORY from the right in Medium and Narrow", () => {
    expect(drawerSide("history", "wide")).toBeNull();
    expect(drawerSide("history", "medium")).toBe("right");
    expect(drawerSide("history", "narrow")).toBe("right");
    expect(drawerSide("history", "stacked")).toBeNull();
  });

  it("covers every width mode for every drawer panel", () => {
    for (const panel of DRAWER_PANELS) {
      for (const mode of WIDTH_MODES) {
        const side = drawerSide(panel, mode);
        expect(side === null || side === "left" || side === "right").toBe(true);
      }
    }
  });
});

describe("drawerFallbackOpen (fresh-install defaults)", () => {
  it("starts the history drawer open in both drawer modes (the commit-detail flow stays alive)", () => {
    expect(drawerFallbackOpen("history", "medium")).toBe(true);
    expect(drawerFallbackOpen("history", "narrow")).toBe(true);
    expect(drawerFallbackOpen("history", "wide")).toBe(false);
    expect(drawerFallbackOpen("history", "stacked")).toBe(false);
  });

  it("starts the files/status drawers closed (rail toggle is the affordance)", () => {
    for (const panel of ["files", "status"] as const) {
      for (const mode of WIDTH_MODES) {
        expect(drawerFallbackOpen(panel, mode)).toBe(false);
      }
    }
  });
});

describe("DRAWER_PREF_KEY", () => {
  it("gives every drawer panel a unique namespaced key", () => {
    const keys = DRAWER_PANELS.map((panel) => DRAWER_PREF_KEY[panel]);
    expect(new Set(keys).size).toBe(DRAWER_PANELS.length);
    for (const key of keys) {
      expect(key).toMatch(/^overlay\./);
    }
  });
});

describe("isDrawerPanel", () => {
  it("accepts exactly the three drawer-capable panels", () => {
    expect(isDrawerPanel("files")).toBe(true);
    expect(isDrawerPanel("status")).toBe(true);
    expect(isDrawerPanel("history")).toBe(true);
    expect(isDrawerPanel("diff")).toBe(false);
    expect(isDrawerPanel("terminal")).toBe(false);
    expect(isDrawerPanel("forge")).toBe(false);
  });
});

describe("planDrawerPanels", () => {
  it("plans the default layout's drawers per mode", () => {
    const visible = ["files", "status", "history", "diff"] as const;
    expect(planDrawerPanels("wide", [...visible])).toEqual([]);
    expect(planDrawerPanels("medium", [...visible])).toEqual([
      { panel: "files", side: "left" },
      { panel: "history", side: "right" },
    ]);
    expect(planDrawerPanels("narrow", [...visible])).toEqual([
      { panel: "files", side: "left" },
      { panel: "status", side: "right" },
      { panel: "history", side: "right" },
    ]);
    expect(planDrawerPanels("stacked", [...visible])).toEqual([]);
  });

  it("skips panels absent from the tree", () => {
    expect(planDrawerPanels("medium", ["diff"])).toEqual([]);
    expect(planDrawerPanels("narrow", ["status", "diff"])).toEqual([
      { panel: "status", side: "right" },
    ]);
  });

  it("never drawerizes the last docked panel (workspace would empty)", () => {
    // Only files + editor: files drawerizes, the editor remains docked.
    expect(planDrawerPanels("medium", ["files", "diff"])).toEqual([
      { panel: "files", side: "left" },
    ]);
    // A files-only tree would be left with nothing: files stays docked.
    expect(planDrawerPanels("medium", ["files"])).toEqual([]);
    // Status wins its slot before history (registry order priority).
    expect(planDrawerPanels("narrow", ["status", "history"])).toEqual([
      { panel: "status", side: "right" },
    ]);
  });
});

describe("drawerPaneClass", () => {
  it("renders an open drawer visible at its edge", () => {
    const open = drawerPaneClass("left", true);
    expect(open).toContain("absolute");
    expect(open).toContain("left-10"); // skips the 40px activity rail
    expect(open).toContain("visible");
    expect(open).toContain("translate-x-0");
    expect(open).toContain("w-80");
    expect(drawerPaneClass("right", true)).toContain("right-0");
  });

  it("keeps a closed drawer mounted but invisible, slid off its edge", () => {
    const closedLeft = drawerPaneClass("left", false);
    expect(closedLeft).toContain("invisible");
    expect(closedLeft).toContain("-translate-x-full");
    // Visibility-driven (never `display: none`, which would kill the
    // keep-alive content): no standalone `hidden` class — `overflow-hidden`
    // in the chrome is fine.
    expect(closedLeft).not.toMatch(/(^|\s)hidden(\s|$)/);
    const closedRight = drawerPaneClass("right", false);
    expect(closedRight).toContain("invisible");
    expect(closedRight).toContain("translate-x-full");
  });
});

describe("drawerPanelLabel", () => {
  it("names every drawer dialog", () => {
    expect(drawerPanelLabel("files")).toBe("Files panel");
    expect(drawerPanelLabel("status")).toBe("Status panel");
    expect(drawerPanelLabel("history")).toBe("History panel");
  });
});
