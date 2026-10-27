/**
 * Panel popouts (M4 F1, extended in M9): "Pop out diff" / "Pop out history"
 * / "File history" / "Commit detail" open a separate Tauri webview window
 * rendering just that panel.
 *
 * Query contract (the new window's own URL):
 *
 *   ?panel=diff|history|filehistory|commitdetail|workflowrun|actionrun
 *   &repo=<repoId>[&path=<repo-relative path>][&sha=<commit sha>]
 *   [&run=<run id>][&actionRun=<action run id>]
 *
 * `path` is required for `filehistory`; `sha` for `commitdetail`; `run`
 * (a positive integer) for `workflowrun`; `actionRun` (the backend's string
 * run id, e.g. `act-1f`) for `actionrun`.
 * `App.svelte` reads it once at startup and renders the matching view alone
 * (each wires its own stores). Outside Tauri (browser / tests) opening a
 * popout toasts instead of throwing.
 *
 * NOTE: windows are created by the Rust `open_popout` command, not the core
 * `create-webview-window` one. On Windows, WebView2 rejects a controller
 * whose browser args differ from the environment already running on the
 * same user data folder (0x8007139F), and the main window is configured
 * with `additionalBrowserArgs` that the JS API cannot pass — so the Rust
 * side copies them from tauri.conf.json. The capability file must also
 * cover the `popout-*` labels or IPC inside the window is denied.
 */

import { isTauri } from "$lib/entry/dragdrop";
import { openPopout } from "$lib/ipc/client";
import { toast } from "$lib/toast";

/** Panels that support popout windows. */
export type PopoutPanel =
  | "diff"
  | "history"
  | "filehistory"
  | "commitdetail"
  | "workflowrun"
  | "actionrun";

/** Options beyond the panel + repo (M9: file history path). */
export interface PopoutOptions {
  /** Repo-relative path (required for `filehistory`). */
  path?: string;
  /** Full commit sha (required for `commitdetail`). */
  sha?: string;
  /** GitHub Actions run id (required for `workflowrun`). */
  runId?: number;
  /** Custom action run id string (required for `actionrun`). */
  actionRunId?: string;
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
  if (options.sha !== undefined && options.sha !== "") {
    query += `&sha=${encodeURIComponent(options.sha)}`;
  }
  if (
    options.runId !== undefined &&
    Number.isInteger(options.runId) &&
    options.runId > 0
  ) {
    query += `&run=${options.runId}`;
  }
  if (options.actionRunId !== undefined && options.actionRunId !== "") {
    query += `&actionRun=${encodeURIComponent(options.actionRunId)}`;
  }
  return query;
}

export interface ParsedPopout {
  panel: PopoutPanel;
  repoId: string;
  path: string | null;
  sha: string | null;
  runId: number | null;
  actionRunId: string | null;
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
    (panel !== "diff" &&
      panel !== "history" &&
      panel !== "filehistory" &&
      panel !== "commitdetail" &&
      panel !== "workflowrun" &&
      panel !== "actionrun") ||
    !repoId
  ) {
    return null;
  }
  if (panel === "filehistory" && !params.get("path")) return null;
  if (panel === "commitdetail" && !params.get("sha")) return null;
  const rawRun = params.get("run");
  const runId = rawRun !== null && /^\d+$/.test(rawRun) ? Number(rawRun) : null;
  if (panel === "workflowrun" && (runId === null || runId <= 0)) return null;
  const actionRunId = params.get("actionRun");
  if (panel === "actionrun" && !actionRunId) return null;
  return {
    panel,
    repoId,
    path: params.get("path"),
    sha: params.get("sha"),
    runId,
    actionRunId,
  };
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
  const shaKey =
    options.sha !== undefined && options.sha !== ""
      ? `-${options.sha.replace(/[^a-zA-Z0-9-/:_]/g, "").slice(0, 12)}`
      : "";
  const runKey =
    options.runId !== undefined && Number.isInteger(options.runId)
      ? `-run${options.runId}`
      : "";
  const actionKey =
    options.actionRunId !== undefined && options.actionRunId !== ""
      ? `-${options.actionRunId.replace(/[^a-zA-Z0-9-/:_]/g, "").slice(0, 24)}`
      : "";
  return `popout-${panel}-${safe || "repo"}${pathKey}${shaKey}${runKey}${actionKey}`;
}

/**
 * Opens (or focuses) the popout window for a panel + repo via the Rust
 * `open_popout` command (which reuses the main window's browser args — see
 * the module note). Non-Tauri environments get an explanatory toast;
 * Tauri failures toast the backend error instead of throwing.
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
  if (panel === "commitdetail" && !options.sha) {
    toast("Commit detail needs a sha", { kind: "error" });
    return;
  }
  if (
    panel === "workflowrun" &&
    (options.runId === undefined || !Number.isInteger(options.runId))
  ) {
    toast("Run details need a run id", { kind: "error" });
    return;
  }
  if (panel === "actionrun" && !options.actionRunId) {
    toast("Action run logs need a run id", { kind: "error" });
    return;
  }
  const label = popoutLabel(panel, repoId, options);
  try {
    await openPopout({
      label,
      query: popoutQueryString(panel, repoId, options),
      title,
    });
  } catch (err) {
    toast(
      `Popout failed: ${err instanceof Error ? err.message : String(err)}`,
      { kind: "error" },
    );
  }
}
