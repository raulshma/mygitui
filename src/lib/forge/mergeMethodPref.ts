/**
 * Per-repo last-used merge-method pref (E2, plan §9.4).
 *
 * The merge dialog remembers the method that last succeeded per repo root
 * and pre-selects it next time. Storage follows the forge prefs
 * conventions (see `remotePrefs.ts`): `mygitui.*` key prefix keyed by repo
 * root (stable across launches; repo ids are per-session), parse-validated
 * reads, all storage access swallowed (privacy modes / SSR / tests may
 * have no localStorage).
 *
 * Initial choice when nothing was remembered yet ("squash-if-allowed, else
 * merge"): squash — the repo-friendly default — unless the PR is
 * explicitly conflicting, where no method can succeed anyway and a plain
 * merge commit is the least surprising pre-selection. Repo-level
 * squash/merge/rebase allowances are not part of `PrInfo`; a not-allowed
 * method surfaces as gh's verbatim rejection in the dialog (with retry),
 * and the surviving memory then wins from the next merge on.
 */

import type { PrMergeMethod } from "$lib/ipc/client";

/** Storage key prefix; the repo root is appended verbatim. */
export const MERGE_METHOD_PREFIX = "mygitui.forge.merge-method.";

const MERGE_METHODS: readonly PrMergeMethod[] = ["merge", "squash", "rebase"];

/** Parse-and-drop: only the three gh methods survive. */
export function parseMergeMethod(raw: string | null): PrMergeMethod | null {
  if (raw === null) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    return MERGE_METHODS.includes(parsed as PrMergeMethod)
      ? (parsed as PrMergeMethod)
      : null;
  } catch {
    return null;
  }
}

/** The remembered method for a repo root (`null` = never merged there). */
export function loadMergeMethod(root: string): PrMergeMethod | null {
  if (root === "") return null;
  try {
    return parseMergeMethod(
      globalThis.localStorage?.getItem(MERGE_METHOD_PREFIX + root) ?? null,
    );
  } catch {
    return null;
  }
}

/** Persists the method for a repo root (`null` clears it). */
export function saveMergeMethod(
  root: string,
  method: PrMergeMethod | null,
): void {
  if (root === "") return;
  try {
    if (method === null) {
      globalThis.localStorage?.removeItem(MERGE_METHOD_PREFIX + root);
    } else if (MERGE_METHODS.includes(method)) {
      globalThis.localStorage?.setItem(
        MERGE_METHOD_PREFIX + root,
        JSON.stringify(method),
      );
    }
  } catch {
    // Storage full/unavailable: the choice still applies this session.
  }
}

/**
 * The dialog's initial method when nothing was remembered: squash when the
 * PR is mergeable ("squash-if-allowed"), a plain merge commit otherwise
 * (see the module doc for why `mergeable` is the only available signal).
 */
export function initialMergeMethod(pr: { mergeable: string }): PrMergeMethod {
  return pr.mergeable === "MERGEABLE" ? "squash" : "merge";
}
