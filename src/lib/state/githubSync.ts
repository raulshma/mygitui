/**
 * Mirrors the persisted GitHub switches into the backend's spawn gate.
 *
 * The frontend IPC client is the primary enforcement point (it rejects a
 * gated command before `invoke`), so this sync is defense-in-depth: the
 * Rust runner refuses to launch `gh` while its process-global gate is off,
 * even if some future call site bypasses the client.
 *
 * Called once at startup (before the pollers start) and from an `$effect`
 * that tracks the settings. No-op outside Tauri.
 */

import { isTauri } from "$lib/entry/dragdrop";
import { githubConfigure } from "$lib/ipc/client";
import { appSettings } from "$lib/state/appSettings.svelte";

/** The switch payload (camelCase; the client maps it to snake_case args). */
export function githubSwitches(): {
  enabled: boolean;
  pullRequests: boolean;
  actions: boolean;
  issues: boolean;
  notifications: boolean;
  statusChips: boolean;
} {
  return {
    enabled: appSettings.githubEnabled,
    pullRequests: appSettings.githubPullRequests,
    actions: appSettings.githubActions,
    issues: appSettings.githubIssues,
    notifications: appSettings.githubNotifications,
    statusChips: appSettings.githubStatusChips,
  };
}

/** Last payload sent — the settings effect fires on every store write. */
let lastPayload = "";

/** Sends the current switches when they changed (idempotent, never throws). */
export function syncGithubSettings(): void {
  if (!isTauri()) return;
  const switches = githubSwitches();
  const payload = JSON.stringify(switches);
  if (payload === lastPayload) return;
  lastPayload = payload;
  void githubConfigure(switches).catch((err) => {
    // Keep the next settings change retrying.
    lastPayload = "";
    console.warn("[github] settings sync failed:", err);
  });
}
