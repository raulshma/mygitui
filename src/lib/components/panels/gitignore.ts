/**
 * Pure logic for the gitignore quick-add (StatusPanel "Ignore" hover
 * action). No runes, no DOM, no IPC: given a changed path, produce the
 * candidate `.gitignore` patterns the user can pick from.
 *
 * Pattern semantics:
 *   - exact: the path verbatim (backslashes normalized to forward slashes,
 *     which is what `.gitignore` matching requires).
 *   - ext:   `*.ext` by the file's extension — ignores every file with that
 *            extension anywhere in the repo; empty when the file has none.
 *   - dir:   the file's parent directory with a trailing slash — ignores
 *            everything inside it; empty for root-level files (ignoring the
 *            repo root itself would be meaningless).
 */

export interface IgnorePatternOptions {
  /** The path itself, e.g. `src/tmp/scratch.log`. */
  exact: string;
  /** `*.log` — every file with this extension. Empty when extension-less. */
  ext: string;
  /** `src/tmp/` — the parent directory. Empty for root-level files. */
  dir: string;
}

/**
 * Candidate ignore patterns for one changed path. Never throws; a weird
 * input (empty path) yields empty strings for every option.
 */
export function patternFor(path: string): IgnorePatternOptions {
  const normalized = path.replaceAll("\\", "/").replace(/^\.\//, "").trim();
  if (normalized === "") {
    return { exact: "", ext: "", dir: "" };
  }
  const slash = normalized.lastIndexOf("/");
  const base = normalized.slice(slash + 1);
  const dir = slash === -1 ? "" : normalized.slice(0, slash + 1);

  const dot = base.lastIndexOf(".");
  // A leading dot is a dotfile name (`.gitignore`), not an extension marker.
  const ext = dot > 0 ? `*${base.slice(dot)}` : "";

  return { exact: normalized, ext, dir };
}
