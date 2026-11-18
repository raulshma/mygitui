/**
 * Pure derived state for the RepoPanel "Hooks" section (batch B item 39).
 *
 * The backend's `hooks_list` reports kind/present/executable only, and
 * there is no generic git-dir file read/write command (`repo_read_file`
 * reaches workdir files only, and `.git/hooks` lives outside the workdir).
 * The section is therefore frontend-only: it derives each hook's absolute
 * file path (`<root>/.git/hooks/<kind>` — valid for local repos; remote
 * tabs have no local path) and the affordances that path allows —
 * "Open in editor" through the opener plugin's `openPath` (the OS default
 * handler) and a copy-path action. No new backend surface.
 */

import type { HookInfo } from "$lib/ipc/types";

/** Absolute path of one hook file inside a repo root. */
export function hookPathFor(root: string, kind: string): string {
  const base = root.replace(/[\\/]+$/, "");
  return `${base}/.git/hooks/${kind}`;
}

/** One rendered hook row: the HookInfo fields plus the derived affordances. */
export interface HookRow {
  kind: string;
  present: boolean;
  executable: boolean;
  /** `<root>/.git/hooks/<kind>`; "" when no local root is known. */
  path: string;
  /** "Open in editor" is possible (file exists and a local root exists). */
  canOpen: boolean;
  /** Copy-path is possible (a local root exists). */
  canCopy: boolean;
}

/** Builds the section's rows (empty root: paths blank, actions off). */
export function hookRowsFor(
  hooks: readonly HookInfo[],
  root: string | null,
): HookRow[] {
  return hooks.map((hook) => ({
    kind: hook.kind,
    present: hook.present,
    executable: hook.executable,
    path: root === null ? "" : hookPathFor(root, hook.kind),
    canOpen: hook.present && root !== null,
    canCopy: root !== null,
  }));
}

/** One-line summary for the section header ("3 of 8 installed · 2 runnable"). */
export function hooksSummary(hooks: readonly HookInfo[]): string {
  const present = hooks.filter((hook) => hook.present).length;
  const runnable = hooks.filter((hook) => hook.present && hook.executable).length;
  const total = hooks.length;
  if (total === 0) return "No hooks reported.";
  const parts = [`${present} of ${total} installed`];
  if (runnable > 0) parts.push(`${runnable} runnable`);
  return `${parts.join(" · ")}.`;
}
