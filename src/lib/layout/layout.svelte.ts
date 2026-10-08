/**
 * Layout store (Svelte 5 runes) — presets, per-repo overlays and the
 * resolved active layout for the workspace.
 *
 * Persistence:
 * - `mygitui.layouts` — user-created presets (built-in DEFAULT_PRESETS live
 *   in code and always win id collisions, so upgrades can reshape them).
 * - `mygitui.layouts.overlay.<repoRoot>` — per-repo overlay:
 *   `{ presetId?, treeOverride?, ratios?, activeTabs?, diffRatio? }`.
 *
 * Active layout resolution: overlay.treeOverride > preset(+ratios/
 * activeTabs/diffRatio overlays) > the "default" preset > DEFAULT_LAYOUT.
 * Splitter drags only write `ratios`/`diffRatio` while the repo follows a
 * preset; any structural change (move/hide/add) snapshots the resolved tree
 * into `treeOverride`, detaching the repo from the preset until reset.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Writes are persisted on a 150 ms debounce (per storage key); storage is
 * injectable for tests (`StorageLike`, same shape as autofetch uses).
 */

import {
  DEFAULT_LAYOUT,
  DEFAULT_PRESETS,
  DEFAULT_PRESET_ID,
  RATIO_MIN,
  applyRatios,
  cloneTree,
  freshId,
  insertPanel,
  movePanel,
  nodeById,
  parseTree,
  presetToLayout,
  removePanel,
  setActiveTab as modelSetActiveTab,
  setRatio as modelSetRatio,
  validateTree,
  visiblePanels,
  type LayoutNode,
  type LayoutPreset,
  type MoveTarget,
  type PanelId,
  type WorkspaceLayout,
} from "./layoutModel";

/** localStorage key holding user presets (`LayoutPreset[]`). */
export const PRESETS_STORAGE_KEY = "mygitui.layouts";
/** Prefix for per-repo overlay keys: prefix + repo root path. */
export const OVERLAY_STORAGE_PREFIX = "mygitui.layouts.overlay.";
/** Debounce window for persisted writes. */
export const PERSIST_DEBOUNCE_MS = 150;

/** Minimal storage surface this store needs (subset of DOM `Storage`). */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Per-repo overlay persisted under `OVERLAY_STORAGE_PREFIX + repoRoot`. */
export interface RepoOverlay {
  /** Preset the repo follows (absent = the default preset). */
  presetId?: string;
  /** Structural snapshot; wins over the preset tree entirely. */
  treeOverride?: LayoutNode;
  /** Split-id → ratio adjustments applied over the preset tree. */
  ratios?: Record<string, number>;
  /** Tab-group id → active index adjustments. */
  activeTabs?: Record<string, number>;
  /** Diff pane fraction override. */
  diffRatio?: number;
}

/** The resolved layout a workspace renders. */
export interface ResolvedLayout {
  /** Effective preset id (what the preset select shows). */
  presetId: string;
  /** Effective preset name. */
  presetName: string;
  /** The resolved workspace layout (fresh objects; safe to mutate-local). */
  layout: WorkspaceLayout;
  /** True when a structural override detached this repo from its preset. */
  overridden: boolean;
  /** True when a per-repo overlay exists at all. */
  hasOverlay: boolean;
}

/** Parses a persisted overlay, dropping malformed fields. */
export function parseOverlay(raw: string | null): RepoOverlay {
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (typeof parsed !== "object" || parsed === null) return {};
  const v = parsed as Record<string, unknown>;
  const overlay: RepoOverlay = {};
  if (typeof v.presetId === "string") overlay.presetId = v.presetId;
  if (v.treeOverride !== undefined && v.treeOverride !== null) {
    const tree = parseTree(v.treeOverride);
    if (tree) overlay.treeOverride = tree;
  }
  if (typeof v.ratios === "object" && v.ratios !== null) {
    const ratios: Record<string, number> = {};
    for (const [key, value] of Object.entries(v.ratios)) {
      if (typeof value === "number" && Number.isFinite(value)) ratios[key] = value;
    }
    if (Object.keys(ratios).length > 0) overlay.ratios = ratios;
  }
  if (typeof v.activeTabs === "object" && v.activeTabs !== null) {
    const activeTabs: Record<string, number> = {};
    for (const [key, value] of Object.entries(v.activeTabs)) {
      if (typeof value === "number" && Number.isInteger(value)) activeTabs[key] = value;
    }
    if (Object.keys(activeTabs).length > 0) overlay.activeTabs = activeTabs;
  }
  if (typeof v.diffRatio === "number" && Number.isFinite(v.diffRatio)) {
    overlay.diffRatio = v.diffRatio;
  }
  return overlay;
}

export class LayoutStore {
  /** User-created presets (persisted; built-ins come from layoutModel). */
  userPresets: LayoutPreset[] = $state([]);
  /** Per-repo overlays keyed by repo root (lazily hydrated from storage). */
  overlays: Record<string, RepoOverlay> = $state({});

  readonly #storage: StorageLike | null;
  /** Roots whose overlay was hydrated (absent in storage = empty overlay). */
  readonly #hydrated = new Set<string>();
  #presetsTimer: ReturnType<typeof setTimeout> | null = null;
  readonly #overlayTimers = new Map<string, ReturnType<typeof setTimeout>>();

  constructor(storage: StorageLike | null = defaultStorage()) {
    this.#storage = storage;
    this.userPresets = this.#loadUserPresets();
  }

  #loadUserPresets(): LayoutPreset[] {
    let raw: string | null = null;
    try {
      raw = this.#storage?.getItem(PRESETS_STORAGE_KEY) ?? null;
    } catch {
      return [];
    }
    if (!raw) return [];
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      return [];
    }
    if (!Array.isArray(parsed)) return [];
    const presets: LayoutPreset[] = [];
    for (const entry of parsed) {
      if (typeof entry !== "object" || entry === null) continue;
      const v = entry as Record<string, unknown>;
      if (typeof v.id !== "string" || typeof v.name !== "string") continue;
      const tree = parseTree(v.tree);
      if (!tree) continue;
      presets.push({
        id: v.id,
        name: v.name,
        tree,
        diffRatio:
          typeof v.diffRatio === "number" && Number.isFinite(v.diffRatio)
            ? v.diffRatio
            : undefined,
        autoHideCommitBar: v.autoHideCommitBar === true,
      });
    }
    return presets;
  }

  // -- presets ---------------------------------------------------------------

  /**
   * Built-in + user presets, in order (built-ins first). User presets with
   * a built-in id are skipped — built-ins always win id collisions so
   * upgrades can reshape them (stale persisted copies must not shadow).
   */
  listPresets(): LayoutPreset[] {
    const out: LayoutPreset[] = DEFAULT_PRESETS.map((p) => ({ ...p }));
    const builtIn = new Set(DEFAULT_PRESETS.map((p) => p.id));
    for (const user of this.userPresets) {
      if (builtIn.has(user.id)) continue;
      out.push(user);
    }
    return out;
  }

  /** Finds a preset by id across built-ins and user presets. */
  presetById(id: string): LayoutPreset | null {
    return this.listPresets().find((p) => p.id === id) ?? null;
  }

  /**
   * Snapshots the repo's current resolved layout as a new named user
   * preset, persists it (debounced) and re-points the repo's overlay at it.
   */
  saveAs(repoRoot: string, name: string): LayoutPreset | null {
    const trimmed = name.trim();
    if (trimmed === "") return null;
    this.#ensureOverlay(repoRoot);
    const resolved = this.active(repoRoot);
    const base = `user-${trimmed.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;
    let id = base;
    for (let i = 2; this.presetById(id) !== null; i++) id = `${base}-${i}`;
    const preset: LayoutPreset = {
      id,
      name: trimmed,
      tree: cloneTree(resolved.layout.main),
      diffRatio: resolved.layout.diffRatio,
      autoHideCommitBar: resolved.layout.autoHideCommitBar,
    };
    this.userPresets = [...this.userPresets, preset];
    this.#schedulePresetsPersist();
    // Follow the new preset (clears any override — the snapshot IS it).
    this.#setOverlay(repoRoot, { presetId: id });
    return preset;
  }

  // -- overlay + resolution --------------------------------------------------

  /**
   * Hydrates (once) the overlay for a repo root from storage. MUST NOT be
   * called from inside a `$derived` (it writes `$state`) — components call
   * it from a `$effect` (see `ensureRepo`); mutating methods call it
   * directly (they run in event handlers, never during derivation).
   */
  #ensureOverlay(repoRoot: string): RepoOverlay {
    if (!this.#hydrated.has(repoRoot)) {
      this.#hydrated.add(repoRoot);
      if (this.overlays[repoRoot] === undefined) {
        let raw: string | null = null;
        try {
          raw = this.#storage?.getItem(OVERLAY_STORAGE_PREFIX + repoRoot) ?? null;
        } catch {
          raw = null;
        }
        this.overlays[repoRoot] = parseOverlay(raw);
      }
    }
    return this.overlays[repoRoot] ?? {};
  }

  /**
   * Component-facing hydration hook: call from a `$effect` keyed on the
   * repo root so persisted overlays load before/with the first layout
   * reads. No-op after the first call per root.
   */
  ensureRepo(repoRoot: string): void {
    this.#ensureOverlay(repoRoot);
  }

  /** Replaces a repo's overlay and schedules its persisted write. */
  #setOverlay(repoRoot: string, overlay: RepoOverlay): void {
    this.overlays[repoRoot] = overlay;
    const timer = this.#overlayTimers.get(repoRoot);
    if (timer) clearTimeout(timer);
    this.#overlayTimers.set(
      repoRoot,
      setTimeout(() => {
        this.#overlayTimers.delete(repoRoot);
        this.#persistOverlay(repoRoot);
      }, PERSIST_DEBOUNCE_MS),
    );
  }

  #persistOverlay(repoRoot: string): void {
    if (!this.#storage) return;
    const overlay = this.overlays[repoRoot];
    const key = OVERLAY_STORAGE_PREFIX + repoRoot;
    try {
      if (!overlay || Object.keys(overlay).length === 0) {
        this.#storage.removeItem(key);
      } else {
        this.#storage.setItem(key, JSON.stringify(overlay));
      }
    } catch {
      // Storage full/unavailable: keep the in-memory state usable.
    }
  }

  #schedulePresetsPersist(): void {
    if (this.#presetsTimer) clearTimeout(this.#presetsTimer);
    this.#presetsTimer = setTimeout(() => {
      this.#presetsTimer = null;
      this.#persistPresets();
    }, PERSIST_DEBOUNCE_MS);
  }

  #persistPresets(): void {
    if (!this.#storage) return;
    try {
      this.#storage.setItem(PRESETS_STORAGE_KEY, JSON.stringify(this.userPresets));
    } catch {
      // Ignore.
    }
  }

  /** Immediate synchronous flush of all pending writes (test/teardown aid). */
  flush(): void {
    if (this.#presetsTimer) {
      clearTimeout(this.#presetsTimer);
      this.#presetsTimer = null;
      this.#persistPresets();
    }
    for (const [repoRoot, timer] of [...this.#overlayTimers]) {
      clearTimeout(timer);
      this.#overlayTimers.delete(repoRoot);
      this.#persistOverlay(repoRoot);
    }
  }

  /**
   * The active layout for a repo: overlay > preset > default. Pure read —
   * safe to call inside `$derived` (hydration happens via `ensureRepo` in
   * a component `$effect`, or in any mutating call). Reads are reactive.
   */
  active(repoRoot: string): ResolvedLayout {
    const overlay = this.overlays[repoRoot] ?? {};
    const preset =
      (overlay.presetId ? this.presetById(overlay.presetId) : null) ??
      this.presetById(DEFAULT_PRESET_ID);
    const fallback = preset ?? {
      id: DEFAULT_PRESET_ID,
      name: "Default",
      tree: DEFAULT_LAYOUT.main,
      diffRatio: DEFAULT_LAYOUT.diffRatio,
      autoHideCommitBar: DEFAULT_LAYOUT.autoHideCommitBar,
    };

    if (overlay.treeOverride) {
      return {
        presetId: overlay.presetId ?? fallback.id,
        presetName: this.presetById(overlay.presetId ?? fallback.id)?.name ?? "Custom",
        layout: {
          main: overlay.treeOverride,
          diffRatio: overlay.diffRatio ?? fallback.diffRatio ?? 0.4,
          autoHideCommitBar: fallback.autoHideCommitBar ?? false,
        },
        overridden: true,
        hasOverlay: true,
      };
    }

    const base = presetToLayout(fallback);
    let adjusted = applyRatios(base.main, overlay.ratios ?? {});
    if (overlay.activeTabs) {
      for (const [tabsId, index] of Object.entries(overlay.activeTabs)) {
        adjusted = modelSetActiveTab(adjusted, tabsId, index);
      }
    }
    return {
      presetId: fallback.id,
      presetName: fallback.name,
      layout: {
        main: adjusted,
        diffRatio: overlay.diffRatio ?? base.diffRatio,
        autoHideCommitBar: base.autoHideCommitBar,
      },
      overridden: false,
      hasOverlay: Object.keys(overlay).length > 0,
    };
  }

  /** Follows a preset: clears any override/ratios, sets `presetId`. */
  applyPreset(repoRoot: string, presetId: string): void {
    this.#ensureOverlay(repoRoot); // hydrate before replacing
    const preset = this.presetById(presetId);
    this.#setOverlay(repoRoot, preset ? { presetId } : {});
  }

  /**
   * Structural tree change (move/hide/add): mutator receives the resolved
   * tree; a falsy/unchanged result is ignored, an invalid result (failed
   * validation) is dropped.
   */
  updateTree(
    repoRoot: string,
    mutator: (tree: LayoutNode) => LayoutNode | null,
  ): void {
    this.#ensureOverlay(repoRoot);
    const resolved = this.active(repoRoot);
    const next = mutator(resolved.layout.main);
    if (!next || next === resolved.layout.main) return;
    if (validateTree(next).length > 0) return;
    this.#setOverlay(repoRoot, { ...this.#ensureOverlay(repoRoot), treeOverride: next });
  }

  /**
   * Splitter drag: while following a preset only the `ratios` map records
   * the drag (the preset stays authoritative); with an override the tree is
   * updated in place. Also accepts `diff` as the split id to resize the
   * diff pane.
   */
  setRatio(repoRoot: string, splitId: string, ratio: number): void {
    const overlay = this.#ensureOverlay(repoRoot);
    if (splitId === "diff") {
      const safe = Math.min(0.95, Math.max(0.05, ratio));
      this.#setOverlay(repoRoot, { ...overlay, diffRatio: safe });
      return;
    }
    if (overlay.treeOverride) {
      this.#setOverlay(repoRoot, {
        ...overlay,
        treeOverride: modelSetRatio(overlay.treeOverride, splitId, ratio),
      });
      return;
    }
    this.#setOverlay(repoRoot, {
      ...overlay,
      ratios: { ...(overlay.ratios ?? {}), [splitId]: ratio },
    });
  }

  /** Tab activation inside a group (kept per repo like ratios). */
  setActiveTab(repoRoot: string, tabsId: string, index: number): void {
    const overlay = this.#ensureOverlay(repoRoot);
    if (overlay.treeOverride) {
      this.#setOverlay(repoRoot, {
        ...overlay,
        treeOverride: modelSetActiveTab(overlay.treeOverride, tabsId, index),
      });
      return;
    }
    this.#setOverlay(repoRoot, {
      ...overlay,
      activeTabs: { ...(overlay.activeTabs ?? {}), [tabsId]: index },
    });
  }

  /** Structural move of a visible panel to a target position. */
  movePanelTo(repoRoot: string, panel: PanelId, target: MoveTarget): void {
    this.updateTree(repoRoot, (tree) => movePanel(tree, panel, target));
  }

  /** Hides a panel (removed from the tree; re-addable via "Add panel"). */
  hidePanel(repoRoot: string, panel: PanelId): void {
    this.updateTree(repoRoot, (tree) => {
      // Never empty the workspace: keep at least one panel.
      if (visiblePanels(tree).length <= 1) return null;
      return removePanel(tree, panel);
    });
  }

  /**
   * Adds a hidden panel: into the tab group at `tabsId` when given, else as
   * a new left column wrapping the whole tree (covers group-less trees like
   * Zen).
   */
  addPanel(repoRoot: string, panel: PanelId, tabsId?: string): void {
    this.updateTree(repoRoot, (tree) => {
      if (tabsId && nodeById(tree, tabsId)) {
        return insertPanel(tree, panel, { kind: "tab", nodeId: tabsId });
      }
      const id = freshId(tree, "split");
      return {
        kind: "split",
        id,
        ratio: RATIO_MIN,
        a: { kind: "tabs", id: freshId(tree, "tabs"), tabs: [panel], active: 0 },
        b: tree,
      };
    });
  }

  /** Drops the repo's overlay (back to the plain preset/default). */
  resetRepo(repoRoot: string): void {
    this.#setOverlay(repoRoot, {});
  }
}

/** The application-wide layout store. */
export const layouts = new LayoutStore();
