/**
 * # `src/lib/layout` — the configurable workspace layout system (M4 F1)
 *
 * The repo workspace is a fixed vertical frame plus a configurable splitter
 * tree for the "main" area:
 *
 * ```
 * ┌───────────────────────────────────────────────────────────┐
 * │ repo header (fixed)                                       │
 * ├────┬──────────────┬───────────────────────┬───────────────┤
 * │ A  │ primary      │ diff / commit editor  │ history       │
 * │ c  │ sidebar      │  (leaf)               │  (leaf,       │
 * │ t  │ (status,     │                       │  collapsible  │
 * │ i  │ branches, …) │                       │  to a rail)   │
 * │ v  │              │                       │               │
 * ├────┴──────────────┴───────────────────────┴───────────────┤
 * │ activity rail (left) and auto-hide tab (right edge)       │
 * └───────────────────────────────────────────────────────────┘
 * ```
 *
 * The commit box lives at the top of the status (changes) panel. The
 * VSCode-style activity bar stays visible and switches the primary-sidebar
 * panel. Collapsing either sidebar does not remove panels from the tree:
 * the pane is hidden with CSS (`collapsedIds`), its panels stay mounted
 * (drafts, selections, scroll survive) and the remaining panes grow into the
 * freed width. The right sidebar collapses to a vertical History rail;
 * the primary sidebar collapses to the activity bar itself.
 *
 * Panels are removed from the tree only when the user explicitly hides one
 * ("Hide" in a tab/panel menu); `showRightPanel` re-adds history for layouts
 * that predate the secondary sidebar.
 *
 * ## PanelId registry
 *
 * Every placeable panel is an id in `layoutModel.ts`'s `PanelId` union
 * (`status`, `branches`, `remotes`, `stashes`, `worktrees`, `health`,
 * `submodules`, `reflog`, `undo`, `history`, `diff`, `terminal`, `forge`,
 * `actions`, `stats`) with labels/icons in `PANEL_META`. The id → component
 * mapping itself lives in `RepoView.svelte` (a `{#snippet}` per panel,
 * passed to `SplitContainer` as a `renderPanel` factory), because snippets
 * close over per-repo props and can only be declared in a component.
 * Adding a panel = add the id + `PANEL_META` entry + one snippet + (for
 * addable panels) membership in `TREE_PANELS`.
 *
 * `diff` is the workspace's main editor pane (working-copy file diff,
 * commit detail, or compare result); it is a normal tree panel.
 *
 * ## Module map
 *
 * - `layoutModel.ts` — pure tree schema + operations (movePanel,
 *   removePanel, insertPanel, setRatio, validate…) + `DEFAULT_LAYOUT` +
 *   `DEFAULT_PRESETS`. Unit tested, no runes/DOM/storage.
 * - `layout.svelte.ts` — the `layouts` rune store: preset list (built-ins +
 *   user presets), per-repo overlays, resolution
 *   (`overlay > preset > default`), debounced persistence, `saveAs`/
 *   `resetRepo`.
 * - `popout.ts` — WebviewWindow popouts for `diff` / `history`
 *   (`?panel=<id>&repo=<id>` query contract, guarded outside Tauri).
 * - `splitPrefs.ts` — persisted ratios for panel-internal splitters
 *   (`SplitPane`: history list ↔ detail, detail meta ↔ diff) plus the
 *   per-width-mode open/closed state overlay drawers persist.
 * - `widthMode.svelte.ts` — the reactive window width modes (Wide/Medium/
 *   Narrow/Stacked, plan §4.2) mirrored onto `<html data-width-mode="…">`;
 *   the `wm-*` custom variants in `src/app.css` key off it.
 * - `drawerPlan.ts` — the pure §10 responsive-matrix drawer plan: which
 *   panel drawerizes at which width mode and from which edge, the
 *   fresh-install open fallbacks, the never-empty-the-workspace guard, and
 *   the shared host chrome/handler contract (dispatching to the stacked
 *   full-screen chrome for `stacked: true` bindings). Unit tested, no
 *   runes/DOM.
 * - `workspaceDrawers.svelte.ts` — the reactive per-mode drawer open/closed
 *   state, persisted through `splitPrefs` (`DRAWER_PREF_KEY`); consumed by
 *   `RepoView` (which owns toggles, focus return and the §10 rail/toggle
 *   affordances).
 * - `stackedPlan.ts` — the pure §10 Stacked (<800px) fallback plan: the
 *   bottom tab bar's inventory (5 primary views + More), display labels,
 *   and the full-screen keep-alive host chrome. Unit tested, no
 *   runes/DOM.
 * - `workspaceStacked.svelte.ts` — the reactive active stacked view
 *   (hydrated once per session, persisted through
 *   `mygitui.stacked.view`); consumed by `RepoView` (which owns view
 *   routing and host bindings).
 * - `containerWidth.svelte.ts` — ResizeObserver width tracking for
 *   structure-switching layouts (§4.2 "a panel adapts to its width").
 * - `../components/layout/SplitContainer.svelte` — recursive renderer for
 *   the tree (draggable + keyboard-accessible dividers, collapsed-pane
 *   hiding via `collapsedIds`, §10 overlay-drawer panes via `drawerPanes`).
 * - `../components/layout/ActivityBar.svelte` — VSCode-style icon rail that
 *   switches the primary sidebar and toggles it without unmounting.
 * - `../components/layout/SplitPane.svelte` — generic two-pane splitter for
 *   panel-internal resize handles (same divider UX, local ratio state).
 * - `../components/layout/TabGroup.svelte` — tab-group leaf renderer
 *   (tablist + tab context menu + "+ Add panel"; the activity-controlled
 *   group shows a compact title row instead of a tab strip).
 *
 * ## Persistence keys
 *
 * - `mygitui.layouts` — user presets (`LayoutPreset[]`).
 * - `mygitui.layouts.overlay.<repoRoot>` — per-repo overlay
 *   (`{ presetId?, treeOverride?, ratios?, activeTabs?, diffRatio?,
 *   leftCollapsed?, rightCollapsed? }`).
 * - `mygitui.split.<name>` — panel-internal splitter ratios + per-width-mode
 *   open/closed state (global prefs; see `splitPrefs.ts`).
 *
 * All writes go through a 150 ms debounce; storage is injectable for tests.
 */
export * from "./layoutModel";
export * from "./layout.svelte";
export * from "./splitPrefs";
export {
  openPanelPopout,
  parsePopoutQuery,
  popoutQueryString,
  type PopoutPanel,
} from "./popout";
