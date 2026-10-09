/**
 * First-launch argv + deep-link entry (M12).
 *
 * Two open paths converge on the same `openPath` callback (App.svelte passes
 * its `openRepoTab` — the same function the single-instance `cli-args`
 * listener uses):
 *
 *   1. First-launch argv — `cliArgsInitial` serves the args captured by the
 *      backend at process start (`mygitui <path>`); the FIRST path that
 *      opens successfully wins (later candidates are fallbacks, not extra
 *      tabs).
 *   2. Deep links — `mygitui://open?path=<abs>` via the tauri deep-link
 *      plugin's `onOpenUrl` (URL-decoded; every URL's path opens).
 *
 * Everything is wrapped in try/catch and no-ops outside a Tauri webview:
 * the module never throws, and a failed open (missing folder, bad URL) is
 * swallowed after reporting through the normal open-path error handling.
 *
 * A module-level `opened` set guards double-opens of the same path per
 * session (deep links can repeat; `openTab` itself dedupes by root, but the
 * guard also keeps duplicate argv/deep-link races quiet).
 */

import { isTauri } from "$lib/entry/dragdrop";
import { cliArgsInitial } from "$lib/ipc/client";

/** Paths already opened (or being opened) this session — dedupe guard. */
const opened = new Set<string>();

/** Opens a session-guarded path through `openPath`; false when rejected. */
async function openOnce(
  path: string,
  openPath: (p: string) => Promise<void> | void,
): Promise<boolean> {
  if (path === "" || opened.has(path)) return false;
  opened.add(path);
  try {
    await openPath(path);
    return true;
  } catch (err) {
    // Let the owner's open-path handling surface the failure (App.svelte
    // toasts it), then release the guard so a corrected retry can pass.
    opened.delete(path);
    throw err;
  }
}

/**
 * Parses a deep-link URL (`mygitui://open?path=<abs>`) into the path, or
 * `null` for anything else (wrong scheme, missing/blank path, bad URL).
 */
export function pathFromDeepLink(url: string): string | null {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "mygitui:") return null;
    // searchParams decodes percent-encoding (URL-encoded Windows paths).
    const path = parsed.searchParams.get("path");
    return path !== null && path.trim() !== "" ? path : null;
  } catch {
    return null;
  }
}

/**
 * Installs the first-launch argv consumer and (inside Tauri) the deep-link
 * listener. Call once from App.svelte's `onMount`, next to the existing
 * `cli-args` (second-launch) listener.
 */
export async function initDeepLinks(
  openPath: (p: string) => Promise<void> | void,
): Promise<void> {
  // (a) First-launch argv: open the first path that opens successfully.
  try {
    const args = await cliArgsInitial();
    for (const arg of args) {
      try {
        if (await openOnce(arg, openPath)) break;
      } catch {
        // Unopenable candidate — try the next one.
      }
    }
  } catch {
    // cli args are best-effort (non-Tauri resolves [] anyway).
  }

  // (b) Deep links: mygitui://open?path=<abs>.
  if (!isTauri()) return;
  try {
    const { onOpenUrl } = await import("@tauri-apps/plugin-deep-link");
    await onOpenUrl((urls) => {
      for (const url of urls) {
        const path = pathFromDeepLink(url);
        if (path === null) continue;
        void openOnce(path, openPath).catch(() => {
          // Surface via the owner's error path only; never crash the handler.
        });
      }
    });
  } catch {
    // Plugin missing / registration failed — deep links stay off.
  }
}
