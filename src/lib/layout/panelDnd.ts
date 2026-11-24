/**
 * Drag and drop primitives for layout panel tabs and groups.
 * Provides in-memory state tracking and standard HTML5 DragEvent helpers.
 */

import type { PanelId } from "./layoutModel";

export const PANEL_DRAG_MIME = "application/x-mygitui-panel";

export interface PanelDragInfo {
  panel: PanelId;
  sourceGroupId: string;
}

let inFlightDrag: PanelDragInfo | null = null;

export function setInFlightDrag(info: PanelDragInfo | null): void {
  inFlightDrag = info;
}

export function getInFlightDrag(): PanelDragInfo | null {
  return inFlightDrag;
}

export function getDragPanelInfo(event: DragEvent): PanelDragInfo | null {
  if (inFlightDrag) return inFlightDrag;
  try {
    const raw = event.dataTransfer?.getData(PANEL_DRAG_MIME);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed.panel === "string" && typeof parsed.sourceGroupId === "string") {
        return parsed as PanelDragInfo;
      }
    }
  } catch {
    // Ignore JSON parse errors
  }
  return null;
}
