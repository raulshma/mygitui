/**
 * Blame query preferences (session-level, shared by every mounted
 * BlameView): the extended `repo_blame` toggles beyond the revision. The
 * flags survive blame open/close within one app session (component state
 * would reset every time the blame tab closes); nothing is persisted to
 * localStorage. Lives in a `.svelte.ts` module because `$state` only
 * compiles there.
 */

export const blameQueryPrefs = $state({
  /** `-w` parity: attribute through whitespace-only commits. */
  ignoreWhitespace: false,
  /** `--first-parent` parity: follow only the first-parent chain. */
  firstParent: false,
  /** Honor `.git-blame-ignore-revs` at the workdir root. */
  ignoreRevs: false,
});
