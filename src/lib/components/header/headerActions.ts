/**
 * Data contract for the repo header's "More repository actions" menu
 * (RepoView owns the commands, RepoHeader renders them). Plain data —
 * like the context-menu entries — so the same list can render as menu
 * rows AND as pinned header buttons (`repoActionPins`), with the pin
 * affordance attached in one place.
 */
import type { IconifyIcon } from "@iconify/types";

/** One pinnable header command (a "More repository actions" entry). */
export interface HeaderAction {
  /** Stable pin id (persisted in `mygitui.repoActionPins`). */
  id: string;
  /** Menu-row text and the pinned button's tooltip. */
  label: string;
  /** Leading icon (Iconify icon data, drawn at 16px). */
  icon: IconifyIcon;
  /** The command (closures capture context, like MenuEntry.run). */
  run: () => void;
  /** Disabled in the menu and on the pinned button (why-tooltip allowed). */
  disabled?: boolean;
  /** Renders a separator before this entry in the menu (grouping). */
  gapBefore?: boolean;
}
