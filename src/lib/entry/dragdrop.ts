/**
 * Drag-and-drop repository opening (webview file drops).
 *
 * The webview's own HTML5 drag-and-drop handles file drops: `dragover`
 * admits file drags onto the window, `drop` extracts the paths and hands
 * them to `onDrop`. This is also what makes in-page HTML5 drags (the repo
 * tab strip's drag-to-reorder) work at all — Tauri's native drag-drop
 * handler (`dragDropEnabled: true`, the default) replaces the webview's
 * drag-drop handling and those events never reach the page, so the app
 * disables it on every window (tauri.conf.json + `popout.rs`) and owns
 * file drops here instead.
 *
 * Path extraction per backend: Linux/WebKitGTK delivers dropped files as
 * `file://` URIs in `text/uri-list`; Windows/WebView2 exposes a non-
 * standard `path` on the dropped `File` objects. Anything else (text
 * drags, in-page tab drags, platforms without paths) yields nothing.
 *
 * The module never throws, and works identically outside of a Tauri
 * webview (browser / e2e tests drive the same HTML5 events).
 */

/** True when running inside a Tauri webview (i.e. the real app). */
export function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    "__TAURI_INTERNALS__" in window &&
    Boolean((window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
  );
}

/** True when a drag carries files from outside the app (not our own drags). */
function isFileDrag(dataTransfer: DataTransfer | null): boolean {
  return dataTransfer?.types.includes("Files") ?? false;
}

/**
 * Repository paths carried by a file drop, in drop order, deduplicated.
 * Empty for non-file drags and platforms that expose no paths.
 */
export function pathsFromDataTransfer(dataTransfer: DataTransfer | null): string[] {
  if (!dataTransfer) return [];
  const paths: string[] = [];
  try {
    // Linux/WebKitGTK: dropped files arrive as `file:///…` URIs, one per
    // line, `#`-prefixed comments allowed.
    for (const line of dataTransfer.getData("text/uri-list").split("\n")) {
      const uri = line.trim();
      if (uri === "" || uri.startsWith("#")) continue;
      try {
        const url = new URL(uri);
        if (url.protocol !== "file:") continue;
        const path = decodeURIComponent(url.pathname);
        if (path !== "") paths.push(path);
      } catch {
        // Not a URI — skip the line.
      }
    }
    // Windows/WebView2: the dropped File carries its real path (non-standard).
    for (const file of dataTransfer.files) {
      const path = (file as File & { path?: unknown }).path;
      if (typeof path === "string" && path !== "") paths.push(path);
    }
  } catch {
    // DataTransfer access can throw on detached events: treat as no paths.
    return [];
  }
  return [...new Set(paths)];
}

/**
 * Invokes `onDrop` with the file/folder paths whenever the user drops items
 * on the window. Resolves with an unlisten function that is always safe to
 * call (and, outside a DOM window, does nothing).
 */
export function listenDragDrop(
  onDrop: (paths: string[]) => void,
): Promise<() => void> {
  const noop = () => {};

  if (typeof window === "undefined") {
    return Promise.resolve(noop);
  }

  try {
    const onDragOver = (event: DragEvent): void => {
      // Admit file drags so `drop` fires; leave every other drag (the tab
      // strip's reorder drags, text selections) untouched.
      if (!isFileDrag(event.dataTransfer) || event.defaultPrevented) return;
      event.preventDefault();
    };
    const onDropEvent = (event: DragEvent): void => {
      if (!isFileDrag(event.dataTransfer) || event.defaultPrevented) return;
      event.preventDefault(); // the browser would otherwise open the file
      const paths = pathsFromDataTransfer(event.dataTransfer);
      if (paths.length > 0) onDrop(paths);
    };
    window.addEventListener("dragover", onDragOver);
    window.addEventListener("drop", onDropEvent);
    return Promise.resolve(() => {
      window.removeEventListener("dragover", onDragOver);
      window.removeEventListener("drop", onDropEvent);
    });
  } catch {
    return Promise.resolve(noop);
  }
}
