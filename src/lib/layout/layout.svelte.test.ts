/**
 * Unit tests for the layout store (`$lib/layout/layout.svelte`) with
 * injected in-memory storage and vitest fake timers (persistence debounce).
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  LayoutStore,
  OVERLAY_STORAGE_PREFIX,
  parseOverlay,
  PERSIST_DEBOUNCE_MS,
  PRESETS_STORAGE_KEY,
  type StorageLike,
} from "./layout.svelte";
import {
  DEFAULT_LAYOUT,
  findPanelNode,
  insertPanel,
  movePanel,
  removePanel,
  visiblePanels,
  type LayoutNode,
} from "./layoutModel";

/** In-memory storage (hermetic; records writes and removals). */
function memStorage(initial: Record<string, string> = {}): StorageLike & {
  dump(): Record<string, string>;
} {
  const data = { ...initial };
  return {
    getItem: (key) => data[key] ?? null,
    setItem: (key, value) => {
      data[key] = value;
    },
    removeItem: (key) => {
      delete data[key];
    },
    dump: () => data,
  };
}

const ROOT = "C:/repos/demo";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("LayoutStore resolution", () => {
  it("resolves the default preset with no storage and no overlay", () => {
    const store = new LayoutStore(memStorage());
    const resolved = store.active(ROOT);
    expect(resolved.presetId).toBe("default");
    expect(resolved.layout.main).toEqual(DEFAULT_LAYOUT.main);
    expect(resolved.layout.diffRatio).toBe(0.4);
    expect(resolved.layout.autoHideCommitBar).toBe(false);
    expect(resolved.hasOverlay).toBe(false);
    expect(resolved.overridden).toBe(false);
  });

  it("applyPreset switches the active preset (with its flags)", () => {
    const store = new LayoutStore(memStorage());
    store.applyPreset(ROOT, "zen");
    const resolved = store.active(ROOT);
    expect(resolved.presetId).toBe("zen");
    expect(visiblePanels(resolved.layout.main)).toEqual(["history"]);
    expect(resolved.layout.autoHideCommitBar).toBe(true);
    expect(resolved.hasOverlay).toBe(true);
  });

  it("unknown preset ids fall back to the default preset", () => {
    const store = new LayoutStore(memStorage());
    store.applyPreset(ROOT, "nope");
    expect(store.active(ROOT).presetId).toBe("default");
  });

  it("resolution precedence: override > preset > default", () => {
    const store = new LayoutStore(memStorage());
    // 1. default preset when no overlay exists.
    expect(store.active(ROOT).presetId).toBe("default");
    // 2. an applied preset replaces the default.
    store.applyPreset(ROOT, "zen");
    expect(visiblePanels(store.active(ROOT).layout.main)).toEqual(["history"]);
    // 3. a structural change snapshots an override on top of the preset
    //    choice (presetId is kept for display; the tree comes from the
    //    override).
    store.updateTree(ROOT, (tree) => {
      const id = findPanelNode(tree, "history")?.id;
      return id
        ? insertPanel(tree, "status", { kind: "tab", nodeId: id })
        : null;
    });
    const resolved = store.active(ROOT);
    expect(resolved.presetId).toBe("zen");
    expect(resolved.overridden).toBe(true);
    expect(visiblePanels(resolved.layout.main)).toEqual(["history", "status"]);
    // An unchanged/null mutator result never creates an override.
    const other = "C:/repos/other";
    store.updateTree(other, (tree) => tree);
    expect(store.active(other).overridden).toBe(false);
  });
});

describe("LayoutStore mutations", () => {
  it("setRatio while following a preset records ratios, not overrides", () => {
    const storage = memStorage();
    const store = new LayoutStore(storage);
    store.setRatio(ROOT, "split-main", 0.4);
    const resolved = store.active(ROOT);
    expect(resolved.overridden).toBe(false);
    expect(findSplitRatio(resolved.layout.main, "split-main")).toBe(0.4);
    // Out-of-range drags clamp.
    store.setRatio(ROOT, "split-main", 5);
    expect(
      findSplitRatio(store.active(ROOT).layout.main, "split-main"),
    ).toBe(0.85);
    // The "diff" pseudo-split resizes the diff pane.
    store.setRatio(ROOT, "diff", 0.7);
    expect(store.active(ROOT).layout.diffRatio).toBe(0.7);
  });

  it("setRatio with an override updates the override tree in place", () => {
    const store = new LayoutStore(memStorage());
    seedOverride(store);
    store.setRatio(ROOT, "split-main", 0.5);
    const resolved = store.active(ROOT);
    expect(findSplitRatio(resolved.layout.main, "split-main")).toBe(0.5);
    expect(resolved.overridden).toBe(true);
  });

  it("structural changes snapshot a validated override", () => {
    const store = new LayoutStore(memStorage());
    // Move status out of the tab group into its own group after history.
    store.movePanelTo(ROOT, "status", {
      kind: "after",
      nodeId: "leaf-history",
    });
    const resolved = store.active(ROOT);
    expect(resolved.overridden).toBe(true);
    expect(visiblePanels(resolved.layout.main)).toEqual([
      "branches",
      "remotes",
      "stashes",
      "worktrees",
      "reflog",
      "undo",
      "terminal",
      "history",
      "status",
    ]);
  });

  it("rejects invalid overrides from mutators", () => {
    const store = new LayoutStore(memStorage());
    // A tree failing validation must be dropped (root would be "bad" if
    // the override had been accepted).
    store.updateTree(ROOT, () => ({
      kind: "split",
      id: "bad",
      ratio: 0.99,
      a: { kind: "leaf", id: "x", panel: "status" },
      b: { kind: "leaf", id: "y", panel: "history" },
    }));
    expect(store.active(ROOT).layout.main.id).toBe("split-main");
    // Null results (guard refusals) are ignored too.
    store.updateTree(ROOT, () => null);
    expect(store.active(ROOT).layout.main.id).toBe("split-main");
  });

  it("setActiveTab persists per repo like ratios", () => {
    const store = new LayoutStore(memStorage());
    store.setActiveTab(ROOT, "tabs-left", 3);
    const group = findPanelNode(store.active(ROOT).layout.main, "undo");
    expect(group?.kind).toBe("tabs");
    expect(group && "active" in group ? group.active : -1).toBe(3);
  });

  it("addPanel appends to the first tab group; hidePanel removes it", () => {
    const store = new LayoutStore(memStorage());
    store.hidePanel(ROOT, "reflog");
    expect(visiblePanels(store.active(ROOT).layout.main)).not.toContain("reflog");
    store.addPanel(ROOT, "reflog", "tabs-left");
    expect(visiblePanels(store.active(ROOT).layout.main)).toContain("reflog");

    // Zen (no tab groups): addPanel wraps the tree in a new left column.
    store.applyPreset(ROOT, "zen");
    store.addPanel(ROOT, "status");
    expect(visiblePanels(store.active(ROOT).layout.main)).toEqual([
      "status",
      "history",
    ]);
  });

  it("hidePanel never empties the workspace", () => {
    const store = new LayoutStore(memStorage());
    store.applyPreset(ROOT, "zen");
    store.hidePanel(ROOT, "history");
    expect(visiblePanels(store.active(ROOT).layout.main)).toEqual(["history"]);
  });

  it("resetRepo drops the overlay entirely", () => {
    const store = new LayoutStore(memStorage());
    store.applyPreset(ROOT, "zen");
    store.setRatio(ROOT, "split-main", 0.4);
    store.resetRepo(ROOT);
    store.flush();
    const resolved = store.active(ROOT);
    expect(resolved.presetId).toBe("default");
    expect(resolved.hasOverlay).toBe(false);
  });

  it("saveAs snapshots the repo layout into a persisted, applied preset", () => {
    const storage = memStorage();
    const store = new LayoutStore(storage);
    store.applyPreset(ROOT, "review");
    store.setRatio(ROOT, "split-review", 0.42);
    const preset = store.saveAs(ROOT, "My Review");
    expect(preset?.name).toBe("My Review");
    // Now following the new preset, cleanly (no override).
    const resolved = store.active(ROOT);
    expect(resolved.presetId).toBe(preset?.id);
    expect(resolved.overridden).toBe(false);
    expect(findSplitRatio(resolved.layout.main, "split-review")).toBe(0.42);
    expect(resolved.layout.diffRatio).toBe(0.6);
    // Renaming to the same name mints a distinct id.
    const again = store.saveAs(ROOT, "my review");
    expect(again?.id).not.toBe(preset?.id);
    // Blank names are refused.
    expect(store.saveAs(ROOT, "   ")).toBeNull();
  });
});

describe("LayoutStore persistence", () => {
  it("debounces overlay writes (150ms) and removes empty overlays", () => {
    const storage = memStorage();
    const store = new LayoutStore(storage);
    store.applyPreset(ROOT, "zen");
    expect(storage.dump()[OVERLAY_STORAGE_PREFIX + ROOT]).toBeUndefined();
    vi.advanceTimersByTime(PERSIST_DEBOUNCE_MS);
    expect(storage.dump()[OVERLAY_STORAGE_PREFIX + ROOT]).toBe(
      JSON.stringify({ presetId: "zen" }),
    );
    store.resetRepo(ROOT);
    vi.advanceTimersByTime(PERSIST_DEBOUNCE_MS);
    expect(storage.dump()[OVERLAY_STORAGE_PREFIX + ROOT]).toBeUndefined();
  });

  it("debounces user preset writes", () => {
    const storage = memStorage();
    const store = new LayoutStore(storage);
    store.saveAs(ROOT, "Mine");
    expect(storage.dump()[PRESETS_STORAGE_KEY]).toBeUndefined();
    vi.advanceTimersByTime(PERSIST_DEBOUNCE_MS);
    const persisted = JSON.parse(storage.dump()[PRESETS_STORAGE_KEY]!) as Array<{
      id: string;
      name: string;
    }>;
    expect(persisted).toHaveLength(1);
    expect(persisted[0]!.name).toBe("Mine");
  });

  it("hydrates overlays + user presets from storage on ensureRepo", () => {
    const storage = memStorage({
      [PRESETS_STORAGE_KEY]: JSON.stringify([
        {
          id: "user-1",
          name: "User One",
          tree: { kind: "leaf", id: "l", panel: "history" },
        },
      ]),
      [OVERLAY_STORAGE_PREFIX + ROOT]: JSON.stringify({
        presetId: "user-1",
        ratios: { "split-x": 0.4 },
      }),
    });
    const store = new LayoutStore(storage);
    // `active()` is a pure read (safe inside $derived): before hydration it
    // resolves the default preset and must not write $state.
    expect(store.active(ROOT).presetId).toBe("default");
    // Hydration is the explicit ensureRepo hook (component $effect).
    store.ensureRepo(ROOT);
    const resolved = store.active(ROOT);
    expect(resolved.presetId).toBe("user-1");
    expect(resolved.presetName).toBe("User One");
    expect(store.listPresets().some((p) => p.id === "user-1")).toBe(true);
    // Mutating calls hydrate too, so a persisted ratio survives a drag.
    store.setRatio(ROOT, "split-x", 0.6);
    store.flush();
    const persisted = parseOverlay(storage.dump()[OVERLAY_STORAGE_PREFIX + ROOT]!);
    expect(persisted.ratios).toEqual({ "split-x": 0.6 });
    expect(persisted.presetId).toBe("user-1");
  });

  it("built-in presets shadow user presets with the same id", () => {
    const storage = memStorage({
      [PRESETS_STORAGE_KEY]: JSON.stringify([
        {
          id: "zen",
          name: "Fake Zen",
          tree: { kind: "leaf", id: "l", panel: "status" },
        },
      ]),
    });
    const store = new LayoutStore(storage);
    expect(store.presetById("zen")?.name).toBe("Zen");
  });

  it("tolerates corrupted storage", () => {
    const storage = memStorage({
      [PRESETS_STORAGE_KEY]: "{not json",
      [OVERLAY_STORAGE_PREFIX + ROOT]: "]]]",
    });
    const store = new LayoutStore(storage);
    expect(store.userPresets).toEqual([]);
    expect(store.active(ROOT).presetId).toBe("default");
  });

  it("works with null storage (memory only)", () => {
    const store = new LayoutStore(null);
    store.applyPreset(ROOT, "zen");
    expect(store.active(ROOT).presetId).toBe("zen");
    expect(() => store.flush()).not.toThrow();
  });
});

describe("parseOverlay", () => {
  it("keeps valid fields and drops malformed ones", () => {
    const overlay = parseOverlay(
      JSON.stringify({
        presetId: "zen",
        treeOverride: {
          kind: "leaf",
          id: "l",
          panel: "history",
        },
        ratios: { a: 0.5, b: "nope" },
        activeTabs: { t: 2, u: 1.5 },
        diffRatio: 0.55,
      }),
    );
    expect(overlay.presetId).toBe("zen");
    expect(overlay.treeOverride?.kind).toBe("leaf");
    expect(overlay.ratios).toEqual({ a: 0.5 });
    expect(overlay.activeTabs).toEqual({ t: 2 });
    expect(overlay.diffRatio).toBe(0.55);
  });

  it("rejects invalid override trees and junk input", () => {
    // An invalid panel id → tree fails to parse → dropped.
    expect(
      parseOverlay(
        JSON.stringify({
          treeOverride: { kind: "leaf", id: "l", panel: "bogus" },
        }),
      ).treeOverride,
    ).toBeUndefined();
    // A rule-breaking tree (duplicate panel) → dropped by validation.
    expect(
      parseOverlay(
        JSON.stringify({
          treeOverride: {
            kind: "tabs",
            id: "t",
            tabs: ["status", "status"],
            active: 0,
          },
        }),
      ).treeOverride,
    ).toBeUndefined();
    expect(parseOverlay("junk")).toEqual({});
    expect(parseOverlay(null)).toEqual({});
    expect(parseOverlay("42")).toEqual({});
    // A valid tree is kept.
    const ok = parseOverlay(
      JSON.stringify({
        treeOverride: {
          kind: "split",
          id: "s",
          ratio: 0.5,
          a: { kind: "leaf", id: "a", panel: "status" },
          b: { kind: "leaf", id: "b", panel: "history" },
        },
      }),
    );
    expect(ok.treeOverride).not.toBeUndefined();
  });
});

// -- helpers ------------------------------------------------------------------

function findSplitRatio(
  tree: LayoutNode | null,
  splitId: string,
): number | null {
  let current = tree;
  while (current?.kind === "split") {
    if (current.id === splitId) return current.ratio;
    current = current.a;
  }
  return null;
}

/** Applies a real structural override (hide one of two left panels). */
function seedOverride(store: LayoutStore): void {
  store.updateTree(ROOT, (tree) => {
    const group = findPanelNode(tree, "branches");
    if (group === null || group.kind !== "tabs") return null;
    return removePanel(tree, "branches");
  });
}
