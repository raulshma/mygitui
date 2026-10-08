/**
 * Panel popouts (M4 F1, v1 scope): "Pop out diff" / "Pop out history" open
 * a separate Tauri webview window rendering just that panel.
 *
 * Query contract (the new window's own URL):
 *
 *   ?panel=diff|history&repo=<repoId>
 *
 * `App.svelte` reads it once at startup and renders `PopoutDiff` or
 * `HistoryView` alone (each wires its own store). Outside Tauri (browser /
 * tests) opening a popout toasts instead of throwing.
 *
 * NOTE: creating a webview window needs the `core:webview:allow-create-
 * webview-window` capability; if the backend refuses, the failure is
 * toasted (the capability file lives outside this lane's edit scope).
 */

import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { isTauri } from "$lib/entry/dragdrop";
import { toast } from "$lib/toast";

/** Panels that support popout windows in v1. */
export type PopoutPanel = "diff" | "history";

/** The popout window URL query for a panel + repo. */
export function popoutQueryString(panel: PopoutPanel, repoId: string): string {
  return `?panel=${encodeURIComponent(panel)}&repo=${encodeURIComponent(repoId)}`;
}

/** Parses `location.search`; `null` when this window is not a popout. */
export function parsePopoutQuery(
  search: string,
): { panel: PopoutPanel; repoId: string } | null {
  let params: URLSearchParams;
  try {
    params = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  } catch {
    return null;
  }
  const panel = params.get("panel");
  const repoId = params.get("repo");
  if ((panel !== "diff" && panel !== "history") || !repoId) return null;
  return { panel, repoId };
}

/** WebviewWindow labels allow `a-zA-Z0-9-/:_`; everything else is dropped. */
function popoutLabel(panel: PopoutPanel, repoId: string): string {
  const safe = repoId.replace(/[^a-zA-Z0-9-/:_]/g, "").slice(0, 48);
  return `popout-${panel}-${safe || "repo"}`;
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
): Promise<void> {
  if (!isTauri()) {
    toast("Popouts require the desktop app");
    return;
  }
  const label = popoutLabel(panel, repoId);
  try {
    const existing = await WebviewWindow.getByLabel(label);
    if (existing) {
      await existing.setFocus();
      return;
    }
    const webview = new WebviewWindow(label, {
      url: popoutQueryString(panel, repoId),
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
