/**
 * Markdown path detection, dependency-free so UI shells (FileWorkspace,
 * FileEditor) can import it without pulling the markdown-it preview chunk.
 */

const MD_EXTENSIONS = new Set(["md", "markdown", "mdown", "mkd"]);

/** True when the workdir path looks like a markdown file. */
export function isMarkdownPath(path: string): boolean {
  const base = path.split(/[\\/]/).pop() ?? path;
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return false;
  return MD_EXTENSIONS.has(base.slice(dot + 1).toLowerCase());
}
