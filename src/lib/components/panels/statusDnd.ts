/**
 * Pure drop-decision logic for drag-and-drop staging (StatusPanel and the
 * Agent-mode changes list). Rows become draggable; the staged/unstaged
 * section headers (and their empty bodies) are the drop targets, so the
 * drop is just an accelerator for the existing stage/unstage buttons —
 * every keyboard and hover path is untouched.
 *
 * The MIME marker follows the TabStrip reorder idiom: drags carrying a
 * different type (OS file drops, tab reorders) are ignored by both sides.
 */

/** dataTransfer type marker: staging drop handlers ignore every other drag. */
export const STATUS_ROW_DRAG_TYPE = "application/x-mygitui-status-row";

/** Which side of the working copy a dragged row came from. */
export type DragOrigin = "staged" | "unstaged";

/** Which section the pointer is over. */
export type DropTarget = "staged" | "unstaged";

/** What a drop should do (the section pair decides; same-side is a no-op). */
export type DropAction = "stage" | "unstage" | "noop";

/**
 * The drop decision: a drag from the unstaged side onto the staged section
 * stages, staged → unstaged unstages, and any same-side (or unknown) pair
 * is a no-op — dropping a row back where it came from changes nothing.
 */
export function dropAction(origin: DragOrigin, target: DropTarget): DropAction {
  if (origin === "unstaged" && target === "staged") return "stage";
  if (origin === "staged" && target === "unstaged") return "unstage";
  return "noop";
}

/** True only for drags this app started (external file drops excluded). */
export function isStatusRowDrag(event: DragEvent): boolean {
  return event.dataTransfer?.types.includes(STATUS_ROW_DRAG_TYPE) ?? false;
}

/** Maps a status-panel section id ("staged" | "changes") to its side. */
export function sectionSide(sectionId: string): DropTarget {
  return sectionId === "staged" ? "staged" : "unstaged";
}
