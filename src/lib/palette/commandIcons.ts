/**
 * Leading icons for palette commands (UI review P2 #12): with 40+ Git
 * commands, an icon per row makes scanning much faster. Panels reuse the
 * exact activity-bar glyphs (`panelIconData`); everything else maps by
 * command id — exact matches first, then the id prefix as a fallback.
 *
 * Icon data is resolved lazily through the reactive `appIcon` resolver
 * (semantic names, see `icons/sets/mapping.json`), so palette rows follow
 * the active icon library. Callers must invoke this per render — the
 * palette calls it inside its `{#each}` template — not cache the result.
 */
import type { IconifyIcon } from "@iconify/types";
import { appIcon } from "$lib/components/icons/library.svelte";
import { panelIconData, PANEL_META, type PanelId } from "$lib/layout/layoutModel";
import type { Command } from "./commands";

/** Command id → semantic icon name (see `icons/sets/mapping.json`). */
const BY_ID: Record<string, string> = {
  "repo.open": "folder-open",
  "repo.open-folder": "folder",
  "repo.clone": "cloud-download",
  "repo.refresh": "refresh",
  "repo.refresh-all": "refresh",
  "repo.close-tab": "close",
  "status.stage-all": "done-all",
  "status.unstage-all": "remove-done",
  "status.stash": "inventory",
  "status.stash-pop": "inventory",
  "branches.create": "account-tree",
  "branches.switch": "account-tree",
  "branches.copy-current": "account-tree",
  "branches.fetch": "cloud-download",
  "branches.fetch-prune": "cloud-download",
  "branches.pull": "arrow-downward",
  "branches.push": "arrow-upward",
  "branches.clean-merged": "cleaning-services",
  "bisect.start": "alt-route",
  "history.focus-filter": "search",
  "history.clear-filter": "close",
  "history.refresh": "refresh",
  "history.toggle-blame": "history",
  "diff.toggle-mode": "compare-arrows",
  "diff.toggle-wrap": "compare-arrows",
  "conflicts.open": "fork-right",
  "conflicts.recheck": "refresh",
  "safety.checkpoint-now": "backup",
  "safety.gc": "cleaning-services",
  "safety.reload-undo": "restart-alt",
  "app.quick-switcher": "search",
  "app.palette": "bolt",
  "app.theme-toggle": "dark-mode",
  "app.theme-system": "dark-mode",
  "app.theme-seed-set": "palette",
  "app.theme-seed-reset": "restart-alt",
  "app.keybinds-reset": "restart-alt",
};

/** Command id prefix → semantic icon name (first match wins). */
const BY_PREFIX: Array<[string, string]> = [
  ["repo.", "folder"],
  ["tab.", "alt-route"],
  ["status.", "done-all"],
  ["branches.", "account-tree"],
  ["bisect.", "alt-route"],
  ["history.", "history"],
  ["diff.", "compare-arrows"],
  ["conflicts.", "fork-right"],
  ["safety.", "backup"],
  ["app.", "bolt"],
];

/** Leading icon for a command row (`null` = render without one). */
export function commandIcon(command: Command): IconifyIcon | null {
  const focusPanel = "panel.focus-" as const;
  if (command.id.startsWith(focusPanel)) {
    const panel = command.id.slice(focusPanel.length) as PanelId;
    return PANEL_META[panel] ? panelIconData(panel) : null;
  }
  const name =
    BY_ID[command.id] ??
    BY_PREFIX.find(([prefix]) => command.id.startsWith(prefix))?.[1];
  return name === undefined ? null : appIcon(name);
}
