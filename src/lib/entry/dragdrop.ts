/**
 * Drag-and-drop repository opening (Tauri webview file drops).
 *
 * Wraps `getCurrentWebview().onDragDropEvent` behind a tiny guarded facade so
 * the app (and tests) work identically outside of a Tauri webview: when the
 * Tauri runtime is not present, nothing is registered and a no-op unlisten is
 * returned. This module never throws.
 */

import { getCurrentWebview } from "@tauri-apps/api/webview";

/** True when running inside a Tauri webview (i.e. the real app). */
export function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    "__TAURI_INTERNALS__" in window &&
    Boolean((window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
  );
}

/**
 * Invokes `onDrop` with the file/folder paths whenever the user drops items
 * on the window. Resolves with an unlisten function that is always safe to
 * call (and, outside Tauri, does nothing).
 */
export function listenDragDrop(
  onDrop: (paths: string[]) => void,
): Promise<() => void> {
  const noop = () => {};

  if (!isTauri()) {
    // Browser / test environment: no Tauri drag-drop events exist.
    return Promise.resolve(noop);
  }

  try {
    return getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "drop") {
          onDrop(event.payload.paths);
        }
      })
      .catch(() => noop);
  } catch {
    return Promise.resolve(noop);
  }
}
