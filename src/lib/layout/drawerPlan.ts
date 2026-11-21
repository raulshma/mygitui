/**
 * Per-width-mode drawer plan (plan §10 responsive matrix) — the pure half
 * of the workspace drawer machinery.
 *
 * The matrix converts three workspace panels into non-modal overlay
 * drawers below the wide tier ("collapse, don't hide", §3 principle 4):
 *
 * | Panel  | Wide | Medium              | Narrow              | Stacked |
 * | ------ | ---- | ------------------- | ------------------- | ------- |
 * | FILES  | dock | drawer from left    | drawer from left    | dock    |
 * | STATUS | dock | dock, narrower      | drawer from right   | dock    |
 * | HISTORY| dock | drawer from right   | drawer from right   | dock    |
 *
 * (`drawerSide` encodes exactly this table; Stacked keeps the docked
 * behavior until the stacked fallback lands.)
 *
 * The reactive half — the per-mode open/closed state, persisted through
 * `splitPrefs` — lives in `workspaceDrawers.svelte.ts`. The rendering
 * plumbing lives in `SplitContainer`/`TabGroup`: a drawer is the SAME
 * keep-alive host element the docked pane uses, repositioned with CSS
 * (`position: absolute` against the workspace frame), so a docked↔drawer
 * mode flip never remounts a panel (§10 mechanics: switching modes never
 * loses editor/scroll state).
 */

import type { PanelId } from "./layoutModel";
import { stackedHostAttrs, stackedPaneClass } from "./stackedPlan";
import type { WidthMode } from "./widthMode.svelte";

/** The panels the matrix can drawerize. */
export type DrawerPanel = "files" | "status" | "history";

/** Which window edge a drawer anchors to (the activity rail is "left"). */
export type DrawerSide = "left" | "right";

/** Registry order for the drawer-capable panels (also the guard's priority). */
export const DRAWER_PANELS: readonly DrawerPanel[] = [
  "files",
  "status",
  "history",
];

/**
 * splitPrefs keys carrying the per-mode open/closed state (`modes`
 * dimension of the `mygitui.split.<key>` v2 envelope). Global — drawer
 * placement is shell behavior, not per-repo state, like every other split
 * pref.
 */
export const DRAWER_PREF_KEY: Record<DrawerPanel, string> = {
  files: "overlay.files",
  status: "overlay.status",
  history: "overlay.history",
};

/** The §10 matrix as a function: the drawer edge at `mode`, or `null` when the panel stays docked. */
export function drawerSide(
  panel: DrawerPanel,
  mode: WidthMode,
): DrawerSide | null {
  switch (panel) {
    case "files":
      return mode === "medium" || mode === "narrow" ? "left" : null;
    case "status":
      return mode === "narrow" ? "right" : null;
    case "history":
      return mode === "medium" || mode === "narrow" ? "right" : null;
  }
}

/**
 * Open-state fallback for modes the user never toggled (fresh installs and
 * first entry into a mode): HISTORY drawers start open — the medium tier is
 * the first collapse step and a closed-by-default right drawer would read
 * as history silently vanishing; in Narrow it keeps the commit-detail flow
 * alive across a resize instead of hiding the review surface the user was
 * reading. Files/Status drawers always start closed (their rail items
 * carry the visible toggle affordance + badge; Narrow hands the editor the
 * reclaimed width).
 */
export function drawerFallbackOpen(panel: DrawerPanel, mode: WidthMode): boolean {
  return panel === "history" && (mode === "medium" || mode === "narrow");
}

/** Narrows a PanelId to the drawer-capable subset. */
export function isDrawerPanel(panel: PanelId): panel is DrawerPanel {
  return (DRAWER_PANELS as readonly string[]).includes(panel);
}

/** One drawerized panel: its rail/drawer edge. */
export interface DrawerizedPanel {
  panel: DrawerPanel;
  side: DrawerSide;
}

/**
 * Which of `visible`'s panels render as overlay drawers at `mode`.
 *
 * Guard: the workspace must never run out of docked content — a panel
 * stays docked when drawerizing it (together with the higher-priority
 * drawers already planned) would leave nothing but drawers. Priority
 * follows {@link DRAWER_PANELS} order (files → status → history).
 */
export function planDrawerPanels(
  mode: WidthMode,
  visible: readonly PanelId[],
): DrawerizedPanel[] {
  const remaining = new Set(visible);
  const out: DrawerizedPanel[] = [];
  for (const panel of DRAWER_PANELS) {
    const side = drawerSide(panel, mode);
    if (side === null || !remaining.has(panel)) continue;
    if (remaining.size <= 1) break; // would empty the workspace
    remaining.delete(panel);
    out.push({ panel, side });
  }
  return out;
}

/**
 * One overlay host binding — the per-instance state + handlers a renderer
 * (SplitContainer pane or TabGroup panel host) needs to present a
 * keep-alive host as a non-modal drawer (§10 Medium/Narrow) OR as a
 * full-screen stacked view (§10 Stacked — `stacked: true`, no edge).
 * `key` is the layout node id for panes (SplitContainer) or the PanelId
 * for tab-group panel hosts.
 */
export interface DrawerBinding {
  key: string;
  /**
   * The drawer's window edge. `undefined` for stacked bindings — a
   * full-screen route has no edge (see {@link overlayPaneClass}).
   */
  side?: DrawerSide;
  open: boolean;
  /** Dialog accessible name ("Files panel", …). */
  label: string;
  /** Esc (scoped) + Tab trap handler, bound to the host element. */
  onKeydown: (event: KeyboardEvent) => void;
  /** Anchors stray presses on the drawer surface back to the panel. */
  onPointerdown: (event: PointerEvent) => void;
  /**
   * §10 Stacked (<800): the host is a full-screen route, not an edge
   * drawer — positioned `inset-0` over the workspace with visibility-only
   * state (see `stackedPlan.stackedPaneClass`); no Esc/edge semantics.
   */
  stacked?: boolean;
}

/**
 * The drawer chrome classes — mirrors `OverlayDrawer`'s contract
 * (slide 120ms in / 100ms out, `visibility` + transform while closed,
 * `shadow-overlay`), positioned against the workspace frame instead of the
 * window edge:
 * `absolute` within `.main-area` (below the repo header), skipping the
 * 40px activity rail on the left (`left-10`). `position: absolute` (not
 * `fixed`) is what keeps the chrome rows visible and lets the host escape
 * its pane's `overflow: hidden` clipping — its containing block is the
 * positioned `.main-area`, and none of the intervening static panes clip
 * or contain it (no transforms on that chain).
 */
export function drawerPaneClass(side: DrawerSide, open: boolean): string {
  const edge =
    side === "left"
      ? "left-10 border-r border-border"
      : "right-0 border-l border-border";
  // Motion grammar (app.css): enter ≤150ms, exit ≤100ms — the class is
  // recomputed when `open` flips, so the duration always matches direction.
  const duration = open ? "duration-[120ms]" : "duration-[100ms]";
  const state = open
    ? "visible translate-x-0"
    : side === "left"
      ? "invisible -translate-x-full"
      : "invisible translate-x-full";
  return `absolute inset-y-0 z-40 flex w-80 flex-col overflow-hidden bg-card shadow-overlay outline-none transition-[transform,visibility] ${duration} ease-out ${edge} ${state}`;
}

/** Descriptive dialog name for a drawer panel (i18n: English-only). */
export function drawerPanelLabel(panel: DrawerPanel): string {
  switch (panel) {
    case "files":
      return "Files panel";
    case "status":
      return "Status panel";
    case "history":
      return "History panel";
  }
}

/**
 * Element attributes presenting a keep-alive host as a non-modal drawer
 * dialog — mirrors `OverlayDrawer`'s surface (role/aria/data markers +
 * the scoped Esc/Tab-trap and surface-press handlers). Spread onto the
 * host element by SplitContainer / TabGroup.
 */
export function drawerHostAttrs(binding: DrawerBinding): Record<string, unknown> {
  return {
    role: "dialog",
    "aria-modal": "false",
    "aria-label": binding.label,
    tabindex: "-1",
    "data-overlay-drawer": binding.key,
    "data-side": binding.side,
    "data-state": binding.open ? "open" : "closed",
    onkeydown: binding.onKeydown,
    onpointerdown: binding.onPointerdown,
  };
}

/**
 * Host chrome dispatch — the ONE place renderers consult for how an
 * overlay host presents: a stacked binding renders as a full-screen route
 * (`stackedPlan` classes/attrs), anything else as an edge drawer.
 */
export function overlayPaneClass(binding: DrawerBinding): string {
  return binding.stacked
    ? stackedPaneClass(binding.open)
    : drawerPaneClass(binding.side ?? "left", binding.open);
}

/** Host attribute dispatch (see {@link overlayPaneClass}). */
export function overlayHostAttrs(
  binding: DrawerBinding,
): Record<string, unknown> {
  return binding.stacked ? stackedHostAttrs(binding) : drawerHostAttrs(binding);
}

