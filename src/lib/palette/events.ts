/**
 * Typed document-level event bus (M9 F1) — the contract between palette
 * commands, context menus, and the owning panels.
 *
 * Palette commands and context-menu actions dispatch here; panels subscribe
 * with {@link onUiEvent} in `$effect`s. Everything is a DOM CustomEvent on
 * `document` (so popout windows keep their own bus) with typed payloads.
 *
 * Pure data + thin DOM wrappers, unit-testable like the rest of `palette/`.
 */

/** All bus event names with their payload types. */
export interface UiEventMap {
  /** Focus a panel stack tab: `{ panel, repoId }`. */
  "focus-panel": { panel: string; repoId: string | null };
  /** Focus the history filter input. */
  "history-focus-filter": undefined;
  /** Clear the history filter. */
  "history-clear-filter": undefined;
  /** Restart the history stream. */
  "history-refresh": undefined;
  /** Toggle the blame view. */
  "history-toggle-blame": undefined;
  /** Select a commit in history: `{ sha, repoId? }`. */
  "history-select-commit": { sha: string; repoId?: string };
  /** Toggle split/unified diff mode. */
  "diff-toggle-mode": undefined;
  /** Toggle diff line wrapping (persisted; every mounted viewer flips). */
  "diff-toggle-wrap": undefined;
  /** Open the conflict editor. */
  "open-conflicts": undefined;
  /** Re-scan merge conflicts. */
  "conflicts-recheck": undefined;
  /** Focus the branch-create form. */
  "branches-focus-create": undefined;
  /** Focus the branch switch/search affordance. */
  "branches-focus-switch": undefined;
  /** Open the merged-branch cleanup wizard (BranchPanel). */
  "branches-cleanup": undefined;
  /** Open the bisect start form (BisectBanner). */
  "bisect-open-start": undefined;
  /** Open file history for a path: `{ path, repoId? }`. */
  "open-file-history": { path: string; repoId?: string };
  /** Request a commit action from anywhere: `{ sha, action }`. */
  "commit-action": { sha: string; action: "cherry-pick" | "revert" | "bookmark" };
  /** Open the AI settings dialog (M12 AiHealthChip / anywhere). */
  "open-ai-settings": undefined;
  /** M12: open the accent-seed picker (App owns the dialog). */
  "open-seed-dialog": undefined;
}

export type UiEventName = keyof UiEventMap;

/** Dispatch a typed UI event on `document`. */
export function emitUiEvent<K extends UiEventName>(
  name: K,
  ...detail: UiEventMap[K] extends undefined ? [] : [UiEventMap[K]]
): void {
  document.dispatchEvent(new CustomEvent(name, { detail: detail[0] }));
}

/** Subscribe to a typed UI event; returns the unlisten function. */
export function onUiEvent<K extends UiEventName>(
  name: K,
  cb: (detail: UiEventMap[K]) => void,
): () => void {
  const handler = (event: Event): void => {
    cb((event as CustomEvent<UiEventMap[K]>).detail);
  };
  document.addEventListener(name, handler);
  return () => document.removeEventListener(name, handler);
}
