/**
 * Dynamic window title store — `${repo}[*] — mygitui`.
 *
 * Follows the tab store's active tab: its backend-provided name plus a
 * ` *` dirty marker while the working tree has changes (`tab.status.entries`
 * — the same source RepoView counts changes from). App.svelte wires ONE
 * `$effect` here: every reactive read (tabs, activeId, status) re-runs it
 * and the result is pushed to the native window via
 * `getCurrentWindow().setTitle`. The computed string is mirrored into the
 * reactive `windowTitle.current` so the custom title bar
 * (`TitleBar.svelte`) can display the same title the OS chrome used to.
 *
 * Popout windows are skipped: their title is set by the Rust `open_popout`
 * command from the caller's string (the query contract carries no title),
 * and this module's tab-store read would clobber it with the bare app
 * name — the popout's custom title bar therefore reads the native title
 * instead.
 *
 * Guarded with the shared `isTauri()` check (see `$lib/entry/dragdrop`):
 * outside a Tauri webview (plain browser / e2e / vitest) the call no-ops.
 * Permission failures (`core:window:allow-set-title` absent from the
 * capability set) degrade to a one-time console warning instead of
 * crashing or spamming.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauri } from "$lib/entry/dragdrop";
import { tabStore } from "$lib/stores/tabs.svelte";

/** Fallback + suffix (matches the static tauri.conf.json window title). */
const APP_NAME = "mygitui";

let lastTitle: string | null = null;
let warned = false;

/** Reactive mirror of the title `syncWindowTitle` derived last. */
const titleState = $state<{ current: string }>({ current: APP_NAME });

/** The title derived from the active tab, for display (read reactively). */
export const windowTitle = {
  get current(): string {
    return titleState.current;
  },
};

/** True when this webview is a panel popout (see `$lib/layout/popout`). */
function inPopoutWindow(): boolean {
  try {
    return (
      typeof location !== "undefined" &&
      new URLSearchParams(location.search).has("panel")
    );
  } catch {
    return false;
  }
}

/**
 * Recomputes the title from the active tab and pushes it when changed.
 * Call from an `$effect` (the reactive reads make the effect re-run).
 */
export function syncWindowTitle(): void {
  if (inPopoutWindow()) return; // popouts keep their Rust-provided title
  const tab = tabStore.active;
  const dirty = (tab?.status?.entries.length ?? 0) > 0;
  const title = tab
    ? `${tab.name}${dirty ? " *" : ""} — ${APP_NAME}`
    : APP_NAME;
  if (title === lastTitle) return; // unchanged → skip the IPC round-trip
  lastTitle = title;
  titleState.current = title;
  if (!isTauri()) return; // browser / e2e: no native window to title
  try {
    getCurrentWindow()
      .setTitle(title)
      .catch((err: unknown) => {
        if (!warned) {
          warned = true;
          console.warn("[mygitui] window title sync failed:", err);
        }
      });
  } catch (err) {
    // getCurrentWindow throws outside a real window context (defensive).
    if (!warned) {
      warned = true;
      console.warn("[mygitui] window title sync failed:", err);
    }
  }
}
