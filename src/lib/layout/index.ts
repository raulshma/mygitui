/**
 * # `src/lib/layout` — the configurable workspace layout system (M4 F1)
 *
 * The repo workspace is a fixed vertical frame plus a configurable splitter
 * tree for the "main" area:
 *
 * ```
 * ┌──────────────────────────────────────────────┐
 * │ repo header (fixed)                          │
 * ├───────────────┬──────────────────────────────┤
 * │ main = LayoutNode tree (SplitContainer)      │
 * │  ┌──────────┬─────────────────────────────┐  │
 * │  │ tabs:    │ history                     │  │
 * │  │ status   │  (leaf)                     │  │
 * │  │ branches │                             │  │
 * │  │ …        │                             │  │
 * │  └──────────┴─────────────────────────────┘  │
 * ├──────────────────────────────────────────────┤
 * │ CommitBar (fixed; Zen auto-hides it)         │
 * ├──────────────────────────────────────────────┤
 * │ diff pane (fixed bottom; collapse + popout)  │
 * └──────────────────────────────────────────────┘
 * ```
 *
 * ## PanelId registry
 *
 * Every placeable panel is an id in `layoutModel.ts`'s `PanelId` union —
 * `status | branches | remotes | stashes | worktrees | reflog | undo |
 * history | diff` — with labels/icons in `PANEL_META`. The id → component
 * mapping itself lives in `RepoView.svelte` (a `{#snippet}` per panel,
 * passed to `SplitContainer` as a `renderPanel` factory), because snippets
 * close over per-repo props and can only be declared in a component.
 * Adding a panel = add the id + `PANEL_META` entry + one snippet + (for
 * addable panels) membership in `TREE_PANELS`.
 *
 * `diff` is the registry's one non-tree panel: it stays the fixed bottom
 * pane (collapse + popout) but shares the union so popout queries and
 * metadata stay uniform.
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
 *   the tree (draggable + keyboard-accessible dividers).
 * - `../components/layout/SplitPane.svelte` — generic two-pane splitter for
 *   panel-internal resize handles (same divider UX, local ratio state).
 * - `../components/layout/TabGroup.svelte` — tab-group leaf renderer
 *   (tablist + tab context menu + "+ Add panel").
 *
 * ## Persistence keys
 *
 * - `mygitui.layouts` — user presets (`LayoutPreset[]`).
 * - `mygitui.layouts.overlay.<repoRoot>` — per-repo overlay
 *   (`{ presetId?, treeOverride?, ratios?, activeTabs?, diffRatio? }`).
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
