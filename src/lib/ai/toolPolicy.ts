/**
 * Agentic-run tool policy (ADR 0001) — pure, no store, no IO.
 *
 * The single-shot AI features deliberately neuter the opencode backend
 * (`V2_GENERATION_PERMISSIONS` in `opencode.ts` + auto-rejecting every
 * permission ask). Agentic runs instead hand the agent a strict policy.
 * On the server, session rules are concatenated AFTER the agent's own
 * rules (the default "Build" agent opens with
 * `{action:"*",resource:"*",effect:"allow"}`) and the winning rule is the
 * LAST match across the whole concatenated list — so this policy OPENS
 * with a `{*, *, ask}` catch-all: it out-votes Build's catch-all for
 * every action our specific rules don't name (MCP tools as
 * `<server>_<tool>`, `todowrite`, `execute`, …), which would otherwise
 * replay as allow. Our specific rules later in the list win over that
 * catch-all; anything matched by neither defaults to ask server-side too.
 *
 * The approved ceiling:
 *   - `read` / `glob` / `grep` auto-allowed, except `read` on sensitive
 *     files (env files, keys, certificates, credential stores, secrets/)
 *     which asks — globs come in workspace-relative AND absolute-path
 *     variants, because reads outside the workspace resolve to absolute
 *     paths that start-anchored globs never match;
 *   - `shell` allowed for a small read-only allowlist (git status/log/
 *     diff/blame/branch, ls — the space-star form also matches the bare
 *     command), every other command asks, and `git push` plus ANY shell
 *     command carrying a redirection (`>` / `<`, `>>` included) is denied
 *     outright — the server's shell resource includes the redirect text
 *     and redirects get no directory gate, so `git status > ~/.bashrc`
 *     would otherwise be an arbitrary file write;
 *   - `edit` asks (mutations are the user's call);
 *   - `webfetch` / `websearch` denied (the agentic surface is the repo,
 *     not the network);
 *   - `external_directory` / `subagent` / `skill` / `question` ask.
 *
 * Accepted tradeoff: `git log *` and `git diff *` stay auto-allowed, so
 * `git log -p -- .env` can print COMMITTED secrets without an ask (the
 * name-class globs below cover the usual secret filenames only). `git
 * show *` was dropped from the allowlist for exactly this reason — it
 * moved to the ask fallback. See ADR 0001.
 *
 * {@link evaluateAsk} duplicates the deny list app-side so attach-mode
 * users behind a looser server config still cannot push or redirect.
 *
 * Pattern semantics — a faithful port of opencode v2.3.2's matcher
 * (`packages/core/src/util/wildcard.ts`, verified against the shipped
 * binary): normalize backslashes to `/`, escape regex metacharacters,
 * THEN transform `*` → `.*` and `?` → `.`, and only afterwards alias the
 * bare command when the TRANSFORMED pattern ends with `" .*"` (replaced
 * with `" ( .*)?"`). The transform order matters: a raw `" **"` becomes
 * `" .*.*"` and never aliases the bare command, while a `" ?*"` suffix
 * gains its dot from the `?` transform and does. Anchored to the whole
 * string; `*` crosses `/`; case-sensitive (linux).
 */

/** One opencode session permission rule (v2 `session.create` shape). */
export interface SessionPermissionRule {
  /** Tool action: read, edit, glob, grep, shell, … (or `<server>_<tool>`). */
  action: string;
  /** Resource pattern the action applies to (path, shell command, `*`). */
  resource: string;
  /** `allow` auto-runs, `ask` forwards to the user, `deny` refuses. */
  effect: "allow" | "ask" | "deny";
}

/**
 * `read` targets that ask instead of auto-running. `secrets/*` rather than
 * a double-star form: under the documented matcher a `*` already crosses
 * `/`, so `secrets/*` matches every depth below `secrets/` — while `**`
 * relies on globstar special-casing the documented semantics never
 * promised. The slash-prefixed variants (`DIR/…` forms) cover reads
 * OUTSIDE the workspace: such paths arrive absolute, and start-anchored
 * globs (`.env`, `id_rsa*`, …) can never match them, while the
 * leading-`*` globs only match by accident. (`*.env` already covers
 * absolute `…/path/.env` — its star crosses `/` — so there is no separate
 * `DIR/.env` entry.)
 */
export const SENSITIVE_READ_GLOBS: readonly string[] = [
  // Env files: the workspace's `.env`/`.env.*`, any `name.env` (absolute
  // `.env` paths included), and nested/absolute `.env.<name>` variants.
  ".env",
  ".env.*",
  "*.env",
  "*/.env.*",
  // Keys and certificates.
  "*.pem",
  "*.key",
  "*.p12",
  "*.pfx",
  "id_rsa*",
  "id_ed25519*",
  "id_ecdsa*",
  "id_dsa*",
  "*/id_rsa*",
  "*/id_ed25519*",
  "*/id_ecdsa*",
  "*/id_dsa*",
  "*/.ssh/id_*",
  // Credential stores (`*credential*` already spans nested/absolute
  // paths; the explicit entries keep the class documented on their own).
  "*credential*",
  ".git-credentials",
  "*/.git-credentials",
  ".netrc",
  "*/.netrc",
  // Everything below `secrets/`, at any depth.
  "secrets/*",
];

/** Read-only shell commands the agent may run without asking. */
export const AGENTIC_SHELL_ALLOWS: readonly string[] = [
  "git status *",
  "git log *",
  "git diff *",
  "git blame *",
  "git branch *",
  "ls *",
];

/**
 * Shell commands denied outright (app policy AND
 * {@link evaluateAsk}'s app-side duplicate). `git push*` has no space
 * before the star on purpose: it matches the bare `git push` plus every
 * argument form, force flags (`--force`, `-f`) and force refspecs
 * (`+main`) included. `*>*` / `*<*` deny every redirection (`>>`
 * included) — without them the allowlist would allow arbitrary file
 * writes like `git status > ~/.bashrc`, because the server's shell
 * resource carries the redirect text and redirects get no directory gate.
 */
export const AGENTIC_SHELL_DENIES: readonly string[] = ["*>*", "*<*", "git push*"];

/** Reply message used when {@link evaluateAsk} auto-rejects a denied ask. */
export const AGENTIC_DENY_MESSAGE =
  "Blocked by mygitui's agentic tool policy: git push and shell redirections are denied.";

/**
 * The approved policy ceiling as an ordered rule list. Order matters
 * (the server keeps the LAST match across agent + session rules):
 *   - the `{*, *, ask}` catch-all comes FIRST so it wins only where no
 *     specific rule of ours matches (it must out-vote the Build agent's
 *     `{*, *, allow}` baseline for unlisted actions);
 *   - reads: broad allow, then the sensitive-file asks (ask wins where
 *     both match);
 *   - shell: the broad ask (fallback for every other command), the
 *     allowlist (allow beats the fallback where the command matches),
 *     and the denies LAST (push, redirections — they must outlast both);
 *   - everything else: one rule per action, no overlap to resolve.
 */
export function buildToolPolicy(): SessionPermissionRule[] {
  return [
    { action: "*", resource: "*", effect: "ask" },
    { action: "read", resource: "*", effect: "allow" },
    { action: "glob", resource: "*", effect: "allow" },
    { action: "grep", resource: "*", effect: "allow" },
    ...SENSITIVE_READ_GLOBS.map((resource) => ({
      action: "read",
      resource,
      effect: "ask",
    }) as SessionPermissionRule),
    // Documented example-env files stay readable: the Build agent's own
    // `{read, *.env.example, allow}` sits BEFORE our session rules, so
    // without this later allow our `.env.*` ask would win (last match).
    { action: "read", resource: "*.env.example", effect: "allow" },
    { action: "shell", resource: "*", effect: "ask" },
    ...AGENTIC_SHELL_ALLOWS.map((resource) => ({
      action: "shell",
      resource,
      effect: "allow",
    }) as SessionPermissionRule),
    ...AGENTIC_SHELL_DENIES.map((resource) => ({
      action: "shell",
      resource,
      effect: "deny",
    }) as SessionPermissionRule),
    { action: "edit", resource: "*", effect: "ask" },
    { action: "webfetch", resource: "*", effect: "deny" },
    { action: "websearch", resource: "*", effect: "deny" },
    { action: "external_directory", resource: "*", effect: "ask" },
    { action: "subagent", resource: "*", effect: "ask" },
    { action: "skill", resource: "*", effect: "ask" },
    { action: "question", resource: "*", effect: "ask" },
  ];
}

/** True when `pattern` matches `value` per opencode's policy semantics. */
export function matchesPolicy(pattern: string, value: string): boolean {
  // Faithful port of opencode v2.3.2's Wildcard.match: transform FIRST
  // (escape → `*`→`.*` → `?`→`.`), THEN alias the bare command when the
  // TRANSFORMED pattern ends with `" .*"` — never on the raw pattern.
  const normalizedValue = value.replace(/\\/g, "/");
  let transformed = pattern
    .replace(/\\/g, "/")
    .replace(/[.+^${}()|[\]\\]/g, "\\$&")
    .replace(/\*/g, ".*")
    .replace(/\?/g, ".");
  if (transformed.endsWith(" .*")) {
    transformed = transformed.slice(0, -3) + "( .*)?";
  }
  // Anchored, dot-matches-all, case-sensitive (the server adds "i" only
  // on case-insensitive filesystems — linux/macOS case-preserving here).
  return new RegExp(`^${transformed}$`, "s").test(normalizedValue);
}

/** What the app should do with a `permission.asked` event. */
export type AskVerdict = "forward" | "auto-reject";

/**
 * The app-side duplicate of the deny list: a shell ask whose command
 * matches a deny pattern (push, redirection) is auto-rejected WITHOUT
 * surfacing to the user — this protects attach-mode users whose server
 * carries a looser saved policy than the ceiling we inject. Everything
 * else forwards to the approval queue. Non-shell actions always forward
 * (the deny list is shell-shaped).
 */
export function evaluateAsk(action: string, resources: string[]): AskVerdict {
  if (action !== "shell") return "forward";
  for (const resource of resources) {
    for (const pattern of AGENTIC_SHELL_DENIES) {
      if (matchesPolicy(pattern, resource)) return "auto-reject";
    }
  }
  return "forward";
}
