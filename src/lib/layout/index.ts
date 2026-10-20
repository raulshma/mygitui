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
 *   (`SplitPane`: history list ↔ detail, detail meta ↔ diff).
 * - `../components/layout/SplitContainer.svelte` — recursive renderer for
 *   the tree (draggable + keyboard-accessible dividers, collapsed-pane
 *   hiding via `collapsedIds`).
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
 * - `mygitui.split.<name>` — panel-internal splitter ratios (global prefs;
 *   see `splitPrefs.ts`).
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
