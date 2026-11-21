/**
 * Per-repo GitHub remote override prefs (E1, plan §9.1).
 *
 * The GitHub flow resolves a repo's owner/name from one configured remote —
 * `origin` by default. Forks and mirrors sometimes need a different remote
 * (`upstream`); this store persists that choice per repo root, following the
 * history-prefs storage conventions: `mygitui.*` key prefix keyed by repo
 * root (stable across launches; repo ids are per-session), parse-validated
 * reads, all storage access swallowed (privacy modes / SSR / tests may have
 * no localStorage).
 *
 * `null` = no override (the backend default `origin` applies).
 */

/** Storage key prefix; the repo root is appended verbatim. */
export const FORGE_REMOTE_PREFIX = "mygitui.forge.remote.";

/** Remote names are short git identifiers; longer values are corrupt. */
const MAX_REMOTE_NAME = 100;

/** Parses a persisted override (null-safe, drops malformed values). */
export function parseForgeRemote(raw: string | null): string | null {
  if (raw === null || raw === "") return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "string") return null;
    const name = parsed.trim();
    // Git remote names cannot start with "-" or contain whitespace/control
    // chars; anything suspicious re-defaults to no override.
    if (
      name === "" ||
      name.length > MAX_REMOTE_NAME ||
      name.startsWith("-") ||
      /\s/.test(name)
    ) {
      return null;
    }
    return name;
  } catch {
    return null;
  }
}

/** Reads the persisted override for a repo root (`null` = default `origin`). */
export function loadForgeRemote(root: string): string | null {
  if (root === "") return null;
  try {
    return parseForgeRemote(
      globalThis.localStorage?.getItem(FORGE_REMOTE_PREFIX + root) ?? null,
    );
  } catch {
    return null;
  }
}

/** Persists the override for a repo root (`null` clears it). */
export function saveForgeRemote(root: string, remote: string | null): void {
  if (root === "") return;
  try {
    if (remote === null) {
      globalThis.localStorage?.removeItem(FORGE_REMOTE_PREFIX + root);
    } else {
      const name = remote.trim();
      if (name === "" || name.startsWith("-") || /\s/.test(name)) return;
      globalThis.localStorage?.setItem(
        FORGE_REMOTE_PREFIX + root,
        JSON.stringify(name),
      );
    }
  } catch {
    // Storage full/unavailable: the choice still applies this session.
  }
}

/**
 * Host of a git remote URL for display ("github.com", "ghe.corp"): any
 * scheme form (`https://…`), scp-like (`git@host:path`) and plain URLs all
 * reduce to the authority's host token (lowercased, `www.` stripped, port
 * dropped). `null` when nothing host-like is found.
 */
export function hostOfRemoteUrl(url: string): string | null {
  const trimmed = url.trim();
  let authority: string;
  const scheme = trimmed.indexOf("://");
  if (scheme !== -1) {
    const rest = trimmed.slice(scheme + 3);
    const slash = rest.indexOf("/");
    authority = slash === -1 ? rest : rest.slice(0, slash);
  } else {
    const colon = trimmed.indexOf(":");
    if (colon === -1) return null;
    authority = trimmed.slice(0, colon);
  }
  const withoutUser = authority.includes("@")
    ? authority.slice(authority.lastIndexOf("@") + 1)
    : authority;
  const token = withoutUser
    .trim()
    .split(":")[0]!
    .trim()
    .toLowerCase();
  if (token === "" || /\s/.test(token)) return null;
  return token.startsWith("www.") ? token.slice(4) : token;
}
