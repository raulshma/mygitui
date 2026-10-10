/**
 * Global context-menu store (M9 F1) — one shared menu instance, opened by
 * any surface (commit rows, status files, branches, reflog entries) with a
 * position and an item list. `ContextMenu.svelte` renders it; this module
 * owns the state (runes, like the other stores).
 *
 * Items are plain data so callers build menus inline; `run` receives no
 * arguments (closures capture context). Separators are `{ separator: true }`.
 */

import type { IconifyIcon } from "@iconify/types";

export interface MenuItem {
  id: string;
  label: string;
  /** Optional leading icon (Iconify icon data, drawn at 16px). */
  icon?: IconifyIcon;
  /** Renders in the danger color (destructive actions). */
  danger?: boolean;
  disabled?: boolean;
  run: () => void;
}

export interface MenuSeparator {
  separator: true;
}

export type MenuEntry = MenuItem | MenuSeparator;

interface ContextMenuState {
  open: boolean;
  x: number;
  y: number;
  entries: MenuEntry[];
}

let state = $state<ContextMenuState>({
  open: false,
  x: 0,
  y: 0,
  entries: [],
});

/** Flips an entry to a separator check in templates. */
export function isSeparator(entry: MenuEntry): entry is MenuSeparator {
  return "separator" in entry;
}

export const contextMenu = {
  get open(): boolean {
    return state.open;
  },
  get x(): number {
    return state.x;
  },
  get y(): number {
    return state.y;
  },
  get entries(): MenuEntry[] {
    return state.entries;
  },

  /** Opens the menu at (x, y) with `entries` (closing any open instance). */
  show(x: number, y: number, entries: MenuEntry[]): void {
    if (entries.length === 0) return;
    state.x = x;
    state.y = y;
    state.entries = entries;
    state.open = true;
  },

  /** Closes without running anything. */
  hide(): void {
    state.open = false;
    state.entries = [];
  },

  /** Repositions the open menu (viewport clamping). */
  move(x: number, y: number): void {
    if (!state.open) return;
    state.x = x;
    state.y = y;
  },
};

/** Convenience for `oncontextmenu` handlers: opens and prevents default. */
export function showMenuAt(
  event: MouseEvent,
  entries: MenuEntry[],
): void {
  event.preventDefault();
  event.stopPropagation();
  contextMenu.show(event.clientX, event.clientY, entries);
}
