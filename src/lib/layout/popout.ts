/**
 * Panel popouts (M4 F1, extended in M9): "Pop out diff" / "Pop out history"
 * / "File history" open a separate Tauri webview window rendering just that
 * panel.
 *
 * Query contract (the new window's own URL):
 *
 *   ?panel=diff|history|filehistory&repo=<repoId>[&path=<repo-relative path>]
 *
 * `path` is required for `filehistory`. `App.svelte` reads it once at
 * startup and renders the matching view alone (each wires its own stores).
 * Outside Tauri (browser / tests) opening a popout toasts instead of
 * throwing.
 *
 * NOTE: creating a webview window needs the `core:webview:allow-create-
 * webview-window` capability; if the backend refuses, the failure is
 * toasted (the capability file lives outside this lane's edit scope).
 */

import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { isTauri } from "$lib/entry/dragdrop";
import { toast } from "$lib/toast";

/** Panels that support popout windows. */
export type PopoutPanel = "diff" | "history" | "filehistory";

/** Options beyond the panel + repo (M9: file history path). */
export interface PopoutOptions {
  /** Repo-relative path (required for `filehistory`). */
  path?: string;
}

/** The popout window URL query for a panel + repo. */
export function popoutQueryString(
  panel: PopoutPanel,
  repoId: string,
  options: PopoutOptions = {},
): string {
  let query = `?panel=${encodeURIComponent(panel)}&repo=${encodeURIComponent(repoId)}`;
  if (options.path !== undefined && options.path !== "") {
    query += `&path=${encodeURIComponent(options.path)}`;
  }
  return query;
}

export interface ParsedPopout {
  panel: PopoutPanel;
  repoId: string;
  path: string | null;
}

/** Parses `location.search`; `null` when this window is not a popout. */
export function parsePopoutQuery(search: string): ParsedPopout | null {
  let params: URLSearchParams;
  try {
    params = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  } catch {
    return null;
  }
  const panel = params.get("panel");
  const repoId = params.get("repo");
  if (
    (panel !== "diff" && panel !== "history" && panel !== "filehistory") ||
    !repoId
  ) {
    return null;
  }
  if (panel === "filehistory" && !params.get("path")) return null;
  return { panel, repoId, path: params.get("path") };
}

/** WebviewWindow labels allow `a-zA-Z0-9-/:_`; everything else is dropped. */
function popoutLabel(
  panel: PopoutPanel,
  repoId: string,
  options: PopoutOptions,
): string {
  const safe = repoId.replace(/[^a-zA-Z0-9-/:_]/g, "").slice(0, 48);
  const pathKey =
    options.path !== undefined && options.path !== ""
      ? `-${options.path.replace(/[^a-zA-Z0-9-/:_]/g, "").slice(0, 32)}`
      : "";
  return `popout-${panel}-${safe || "repo"}${pathKey}`;
}

/**
 * Opens (or focuses) the popout window for a panel + repo. Non-Tauri
 * environments get an explanatory toast; Tauri failures (e.g. a missing
 * capability) toast the backend error instead of throwing.
 */
export async function openPanelPopout(
  panel: PopoutPanel,
  repoId: string,
  title: string,
  options: PopoutOptions = {},
): Promise<void> {
  if (!isTauri()) {
    toast("Popouts require the desktop app");
    return;
  }
  if (panel === "filehistory" && !options.path) {
    toast("File history needs a path", { kind: "error" });
    return;
  }
  const label = popoutLabel(panel, repoId, options);
  try {
    const existing = await WebviewWindow.getByLabel(label);
    if (existing) {
      await existing.setFocus();
      return;
    }
    const webview = new WebviewWindow(label, {
      url: popoutQueryString(panel, repoId, options),
      title,
      width: 960,
      height: 680,
      minWidth: 420,
      minHeight: 300,
    });
    await new Promise<void>((resolve, reject) => {
      webview.once("tauri://created", () => resolve());
      webview.once("tauri://error", (event) => {
        const payload = (event as { payload?: unknown }).payload;
        reject(
          new Error(
            typeof payload === "string"
              ? payload
              : "the window could not be created",
          ),
        );
      });
    });
  } catch (err) {
    toast(
      `Popout failed: ${err instanceof Error ? err.message : String(err)}`,
      { kind: "error" },
    );
  }
}
