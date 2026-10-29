/**
 * Leading icons for palette commands (UI review P2 #12): with 40+ Git
 * commands, an icon per row makes scanning much faster. Panels reuse the
 * exact activity-bar glyphs (`panelIconData`); everything else maps by
 * command id — exact matches first, then the id prefix as a fallback.
 */
import type { IconifyIcon } from "@iconify/types";
import iconFolderOpen from "@ktibow/iconset-material-symbols/folder-open";
import iconFolder from "@ktibow/iconset-material-symbols/folder";
import iconClone from "@ktibow/iconset-material-symbols/cloud-download";
import iconRefresh from "@ktibow/iconset-material-symbols/refresh";
import iconAltRoute from "@ktibow/iconset-material-symbols/alt-route";
import iconClose from "@ktibow/iconset-material-symbols/close";
import iconStageAll from "@ktibow/iconset-material-symbols/done-all";
import iconUnstageAll from "@ktibow/iconset-material-symbols/remove-done";
import iconStash from "@ktibow/iconset-material-symbols/inventory";
import iconBranches from "@ktibow/iconset-material-symbols/account-tree";
import iconPull from "@ktibow/iconset-material-symbols/arrow-downward";
import iconPush from "@ktibow/iconset-material-symbols/arrow-upward";
import iconClean from "@ktibow/iconset-material-symbols/cleaning-services";
import iconHistory from "@ktibow/iconset-material-symbols/history";
import iconCompare from "@ktibow/iconset-material-symbols/compare-arrows";
import iconConflicts from "@ktibow/iconset-material-symbols/fork-right";
import iconBackup from "@ktibow/iconset-material-symbols/backup";
import iconRestartAlt from "@ktibow/iconset-material-symbols/restart-alt";
import iconSearch from "@ktibow/iconset-material-symbols/search";
import iconBolt from "@ktibow/iconset-material-symbols/bolt";
import iconDarkMode from "@ktibow/iconset-material-symbols/dark-mode";
import iconPalette from "@ktibow/iconset-material-symbols/palette";
import { panelIconData, PANEL_META, type PanelId } from "$lib/layout/layoutModel";
import type { Command } from "./commands";

const BY_ID: Record<string, IconifyIcon> = {
  "repo.open": iconFolderOpen,
  "repo.open-folder": iconFolder,
  "repo.clone": iconClone,
  "repo.refresh": iconRefresh,
  "repo.refresh-all": iconRefresh,
  "repo.close-tab": iconClose,
  "status.stage-all": iconStageAll,
  "status.unstage-all": iconUnstageAll,
  "status.stash": iconStash,
  "status.stash-pop": iconStash,
  "branches.create": iconBranches,
  "branches.switch": iconBranches,
  "branches.copy-current": iconBranches,
  "branches.fetch": iconClone,
  "branches.fetch-prune": iconClone,
  "branches.pull": iconPull,
  "branches.push": iconPush,
  "branches.clean-merged": iconClean,
  "bisect.start": iconAltRoute,
  "history.focus-filter": iconSearch,
  "history.clear-filter": iconClose,
  "history.refresh": iconRefresh,
  "history.toggle-blame": iconHistory,
  "diff.toggle-mode": iconCompare,
  "diff.toggle-wrap": iconCompare,
  "conflicts.open": iconConflicts,
  "conflicts.recheck": iconRefresh,
  "safety.checkpoint-now": iconBackup,
  "safety.gc": iconClean,
  "safety.reload-undo": iconRestartAlt,
  "app.quick-switcher": iconSearch,
  "app.palette": iconBolt,
  "app.theme-toggle": iconDarkMode,
  "app.theme-system": iconDarkMode,
  "app.theme-seed-set": iconPalette,
  "app.theme-seed-reset": iconRestartAlt,
  "app.keybinds-reset": iconRestartAlt,
};

const BY_PREFIX: Array<[string, IconifyIcon]> = [
  ["repo.", iconFolder],
  ["tab.", iconAltRoute],
  ["status.", iconStageAll],
  ["branches.", iconBranches],
  ["bisect.", iconAltRoute],
  ["history.", iconHistory],
  ["diff.", iconCompare],
  ["conflicts.", iconConflicts],
  ["safety.", iconBackup],
  ["app.", iconBolt],
];

/** Leading icon for a command row (`null` = render without one). */
export function commandIcon(command: Command): IconifyIcon | null {
  const focusPanel = "panel.focus-" as const;
  if (command.id.startsWith(focusPanel)) {
    const panel = command.id.slice(focusPanel.length) as PanelId;
    return PANEL_META[panel] ? panelIconData(panel) : null;
  }
  return (
    BY_ID[command.id] ??
    BY_PREFIX.find(([prefix]) => command.id.startsWith(prefix))?.[1] ??
    null
  );
}
