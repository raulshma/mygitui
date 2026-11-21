/**
 * The §10 Stacked fallback (<800px) — the pure half of the stacked
 * machinery (the reactive active-view store lives in
 * `workspaceStacked.svelte.ts`).
 *
 * Below the supported floor (800 = the Tauri min window width) the
 * workspace stops splitting: it shows ONE full-screen region at a time,
 * selected through a bottom tab bar that replaces the vertical activity
 * rail. A "full-screen route" is the SAME keep-alive host element the
 * docked pane renders — repositioned with `position: absolute` exactly
 * like the Medium/Narrow overlay drawers (see `drawerPlan.ts`), so mode
 * flips swap classes only and panels never remount (§10 mechanics).
 *
 * Bar inventory (matrix: "bottom tab bar (5 items + more)"): five primary
 * views — Files, Editor, Status, History, GitHub — plus a "More" overflow
 * for every remaining rail destination (branches, remotes, stashes, …,
 * mirroring the ActivityBar's inventory). Every panel is reachable, so §3
 * principle 4 (collapse, don't hide) holds below the floor too.
 */

import { panelLabel, type PanelId } from "./layoutModel";

/**
 * The bottom tab bar's five primary views, in bar order. `diff` is the
 * workspace's center editor (the matrix's "Editor/diff" row), `forge` the
 * GitHub panel ("GitHub panel / run view").
 */
export const STACKED_PRIMARY_VIEWS: readonly PanelId[] = [
  "files",
  "diff",
  "status",
  "history",
  "forge",
];

/** The fresh-install stacked view (nothing persisted yet): the editor. */
export const STACKED_DEFAULT_VIEW: PanelId = "diff";

/**
 * Bar-label overrides. The matrix names the editor view "Editor"; every
 * other view keeps its panel registry label.
 */
const STACKED_VIEW_LABELS: Partial<Record<PanelId, string>> = {
  diff: "Editor",
};

/** The bottom bar / More menu label for a stacked view. */
export function stackedViewLabel(panel: PanelId): string {
  return STACKED_VIEW_LABELS[panel] ?? panelLabel(panel);
}

/** True for the five primary bar items (everything else lives in More). */
export function isStackedPrimaryView(panel: PanelId): boolean {
  return (STACKED_PRIMARY_VIEWS as readonly string[]).includes(panel);
}

/**
 * The "More" overflow contents: every rail-reachable panel that is not a
 * primary view, in the given order (the ActivityBar's panel list, which is
 * `TREE_PANELS` minus the center editor and the right-edge history).
 */
export function stackedMorePanels(panels: readonly PanelId[]): PanelId[] {
  return panels.filter((panel) => !isStackedPrimaryView(panel));
}

/**
 * The full-screen chrome classes for a keep-alive host — the stacked
 * counterpart of `drawerPlan.drawerPaneClass`: `position: absolute`
 * filling the host's containing block (the `.layout-area`, which sits
 * above the bottom bar) with `inset-0`. Visibility-driven like the
 * drawers (never `display: none`, which would kill the keep-alive
 * content), but without a slide transform: a full-screen route has no
 * edge to slide from, and a transform would turn the host into the
 * containing block for `fixed`-positioned panel UI (in-panel menus).
 */
export function stackedPaneClass(active: boolean): string {
  return `absolute inset-0 z-40 flex min-h-0 min-w-0 flex-col overflow-hidden outline-none [&>*]:min-h-0 [&>*]:flex-1 ${
    active ? "visible" : "invisible"
  }`;
}

/**
 * One stacked view binding — the per-instance state a renderer
 * (SplitContainer pane or TabGroup panel host) needs to present a
 * keep-alive host as a full-screen route. `key` mirrors
 * `DrawerBinding.key` (the layout node id for panes, the PanelId for
 * tab-group panel hosts); stacked bindings ride the same
 * `drawerPanes`/`drawerPanels` channels with `stacked: true`.
 */
export interface StackedBindingInfo {
  key: string;
  open: boolean;
  /** Accessible name for the full-screen region ("Editor view", …). */
  label: string;
}

/**
 * Element attributes presenting a keep-alive host as a full-screen stacked
 * view — a labeled region (the bar's items are toolbar buttons in the
 * ActivityBar idiom, so no tabpanel pairing), `data-stacked-view` for
 * targeting, `data-state` for tests/inspection. No dialog semantics: a
 * route is not an overlay, and there is no Esc-to-close (switching views
 * is the only navigation).
 */
export function stackedHostAttrs(
  binding: StackedBindingInfo,
): Record<string, unknown> {
  return {
    role: "group",
    "aria-label": binding.label,
    tabindex: "-1",
    "data-stacked-view": binding.key,
    "data-state": binding.open ? "open" : "closed",
  };
}
