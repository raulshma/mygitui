/**
 * Unit tests for the agentic-run tool policy: the wildcard matcher (a
 * faithful port of the opencode v2.3.2 matcher — cross-slash star,
 * single-char ?, space-star bare-command aliasing decided on the
 * TRANSFORMED pattern, literal text, backslash normalization), the
 * ordered rule ceiling evaluated the way the server does (LAST match
 * across the concatenated rules — including the opening `{*, *, ask}`
 * catch-all that out-votes the Build agent's `*` allow for unlisted
 * actions), and the app-side deny duplicate that guards attach-mode
 * servers.
 */

import { describe, expect, it } from "vitest";
import {
  AGENTIC_SHELL_ALLOWS,
  AGENTIC_SHELL_DENIES,
  buildToolPolicy,
  evaluateAsk,
  matchesPolicy,
  SENSITIVE_READ_GLOBS,
} from "./toolPolicy";

// ---------------------------------------------------------------------------
// matchesPolicy — the matcher semantics (opencode v2.3.2 Wildcard.match)
// ---------------------------------------------------------------------------

describe("matchesPolicy — star semantics", () => {
  it("star matches zero or more characters", () => {
    expect(matchesPolicy("id_rsa*", "id_rsa")).toBe(true);
    expect(matchesPolicy("id_rsa*", "id_rsa_ed25519")).toBe(true);
    expect(matchesPolicy("*.pem", "server.pem")).toBe(true);
  });

  it("star crosses directory separators (including /)", () => {
    expect(matchesPolicy("*.pem", "certs/deep/nested/server.pem")).toBe(true);
    expect(matchesPolicy("secrets/*", "secrets/prod/db.key")).toBe(true);
    expect(matchesPolicy("*credential*", "etc/cloud/credentials.json")).toBe(true);
  });

  it("star matches zero characters across a separator too", () => {
    expect(matchesPolicy("secrets/*", "secrets/")).toBe(true);
    expect(matchesPolicy("secrets/*", "secrets/keys")).toBe(true);
  });

  it("star does not match a missing prefix or suffix", () => {
    expect(matchesPolicy("*.pem", "pem")).toBe(false);
    expect(matchesPolicy("id_rsa*", "rsa")).toBe(false);
  });

  it("star matches empty strings", () => {
    expect(matchesPolicy("*", "")).toBe(true);
    expect(matchesPolicy("*", "anything/at/all")).toBe(true);
  });
});

describe("matchesPolicy — single-char ? semantics", () => {
  it("? matches exactly one character", () => {
    expect(matchesPolicy("id_rsa?", "id_rsa-")).toBe(true);
    expect(matchesPolicy(".env.?*", ".env.a")).toBe(true);
  });

  it("? refuses zero or multiple characters", () => {
    expect(matchesPolicy("id_rsa?", "id_rsa")).toBe(false);
    expect(matchesPolicy("id_rsa?", "id_rsa_ab")).toBe(false);
  });

  it("? is not a literal question mark", () => {
    expect(matchesPolicy("what?", "what?")).toBe(true); // ? matched the char
    expect(matchesPolicy("a?c", "abc")).toBe(true);
  });
});

describe("matchesPolicy — space-star bare-command aliasing", () => {
  it('"cmd *" also matches the bare command', () => {
    expect(matchesPolicy("git status *", "git status")).toBe(true);
    expect(matchesPolicy("git log *", "git log")).toBe(true);
    expect(matchesPolicy("ls *", "ls")).toBe(true);
  });

  it('"cmd *" matches arguments too', () => {
    expect(matchesPolicy("git status *", "git status -sb")).toBe(true);
    expect(matchesPolicy("git diff *", "git diff HEAD~1 -- src/a.ts")).toBe(true);
    expect(matchesPolicy("ls *", "ls -la /tmp")).toBe(true);
  });

  it("the bare alias preserves prefix wildcards", () => {
    // Bare form of "git l* *" is "git l*" — still wildcard-matched.
    expect(matchesPolicy("git l* *", "git log")).toBe(true);
  });

  it("only the command word is aliased, not partial words", () => {
    expect(matchesPolicy("ls *", "lsof")).toBe(false);
    expect(matchesPolicy("git status *", "git statusify")).toBe(false);
    expect(matchesPolicy("git branch *", "git branchx")).toBe(false);
  });

  it("a star without the preceding space does not alias the bare form", () => {
    // "git push*" intentionally matches bare `git push` via the star's
    // zero-char match, but a literal suffix like "git pusht *" must not
    // match "git pusht" unless the star allows it.
    expect(matchesPolicy("git push*", "git push")).toBe(true);
    expect(matchesPolicy("git push*", "git push --force")).toBe(true);
    expect(matchesPolicy("git push *", "git push")).toBe(true); // space-star alias
    expect(matchesPolicy("git pus*h", "git push")).toBe(true);
  });

  it("the alias is decided on the TRANSFORMED pattern (server order)", () => {
    // " **" transforms to " .*.*", which never ends with " .*" — so a
    // " **" suffix does NOT alias the bare command, even though a " *"
    // suffix does. Mirrors the server transforming `*`→`.*` first.
    expect(matchesPolicy("git status **", "git status")).toBe(false);
    expect(matchesPolicy("git status **", "git status -sb")).toBe(true);
    expect(matchesPolicy("ls **", "ls")).toBe(false);
    // A " ?*" suffix does NOT alias either: the `?` supplies a dot right
    // before the star's dot ("..*"), so the transformed tail is not " .*".
    expect(matchesPolicy("git status ?*", "git status")).toBe(false);
    expect(matchesPolicy("foo ? ?*", "foo x")).toBe(false);
    expect(matchesPolicy("foo ? ?*", "foo x y")).toBe(true); // full pattern
  });
});

describe("matchesPolicy — literal text and escaping", () => {
  it("literal patterns match exactly", () => {
    expect(matchesPolicy(".env", ".env")).toBe(true);
    expect(matchesPolicy(".env", ".env.local")).toBe(false);
    expect(matchesPolicy(".env.local", ".env.local")).toBe(true);
    expect(matchesPolicy(".env", ".Env")).toBe(false);
  });

  it("regex metacharacters stay literal", () => {
    expect(matchesPolicy("a+b.txt", "a+b.txt")).toBe(true);
    expect(matchesPolicy("a+b.txt", "aab.txt")).toBe(false);
    expect(matchesPolicy("file(1).txt", "file(1).txt")).toBe(true);
    expect(matchesPolicy("file(1).txt", "file1.txt")).toBe(false);
    expect(matchesPolicy("$var", "$var")).toBe(true);
  });

  it("patterns are anchored to the whole string", () => {
    expect(matchesPolicy("env", ".env")).toBe(false);
    expect(matchesPolicy(".env", "my.env")).toBe(false);
  });
});

describe("matchesPolicy — backslash normalization", () => {
  it("backslashes in the value normalize to forward slashes", () => {
    expect(matchesPolicy("secrets/*", "secrets\\prod\\db.key")).toBe(true);
    expect(matchesPolicy("*.pem", "certs\\server.pem")).toBe(true);
  });

  it("backslashes in the pattern normalize too", () => {
    expect(matchesPolicy("secrets\\*", "secrets/prod/db.key")).toBe(true);
    expect(matchesPolicy("secrets\\*", "secrets\\keys")).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// buildToolPolicy — the ordered ceiling, evaluated server-style
// ---------------------------------------------------------------------------

/**
 * Mirrors the server's rule pick (concat agent+session rules, keep the
 * LAST whose action AND resource both match; the action/resource
 * comparisons use the same wildcard matcher as the resource itself —
 * `xw(requestAction, rule.action) && xw(requestResource, rule.resource)`).
 */
function lastEffect(action: string, resource: string): string | null {
  let effect: string | null = null;
  for (const rule of buildToolPolicy()) {
    if (matchesPolicy(rule.action, action) && matchesPolicy(rule.resource, resource)) {
      effect = rule.effect;
    }
  }
  return effect;
}

describe("buildToolPolicy", () => {
  it("opens with a {*, *, ask} catch-all that out-votes the Build agent's allow", () => {
    const policy = buildToolPolicy();
    // Server-side the session rules are concatenated AFTER the agent's
    // rules (Build opens with `{*, *, allow}`) — the LAST match wins, so
    // our opening catch-all must be `ask` and must come before every
    // specific rule.
    expect(policy[0]).toEqual({ action: "*", resource: "*", effect: "ask" });
  });

  it("unlisted actions (MCP tools, todos, execute) evaluate to ask", () => {
    // Without the catch-all these would replay the Build agent's
    // `{*, *, allow}` baseline as allow.
    expect(lastEffect("todowrite", "*")).toBe("ask");
    expect(lastEffect("execute", "some/command")).toBe("ask");
    expect(lastEffect("mcp_fs_write_file", "/etc/hosts")).toBe("ask");
    expect(lastEffect("mcp_github_create_issue", "*")).toBe("ask");
    expect(lastEffect("experimental_image_generate", "*")).toBe("ask");
  });

  it("auto-allows plain reads, globs and greps", () => {
    expect(lastEffect("read", "src/lib/ai/opencode.ts")).toBe("allow");
    expect(lastEffect("read", "README.md")).toBe("allow");
    expect(lastEffect("glob", "src/**/*.ts")).toBe("allow");
    expect(lastEffect("grep", "TODO")).toBe("allow");
  });

  it("asks on sensitive reads (last ask rule wins over the broad allow)", () => {
    expect(lastEffect("read", ".env")).toBe("ask");
    expect(lastEffect("read", ".env.local")).toBe("ask");
    expect(lastEffect("read", ".env.production")).toBe("ask");
    expect(lastEffect("read", "deploy/production.env")).toBe("ask");
    expect(lastEffect("read", "/home/u/project/.env")).toBe("ask");
    expect(lastEffect("read", "sub/dir/.env.local")).toBe("ask");
    expect(lastEffect("read", "server.pem")).toBe("ask");
    expect(lastEffect("read", "certs/deep/server.pem")).toBe("ask");
    expect(lastEffect("read", "certs/vault.p12")).toBe("ask");
    expect(lastEffect("read", "backup.pfx")).toBe("ask");
    expect(lastEffect("read", "id_rsa")).toBe("ask");
    expect(lastEffect("read", "id_ed25519")).toBe("ask");
    expect(lastEffect("read", "id_ecdsa")).toBe("ask");
    expect(lastEffect("read", "id_dsa")).toBe("ask");
    expect(lastEffect("read", "deploy/id_ed25519")).toBe("ask");
    expect(lastEffect("read", "/home/u/.ssh/id_ed25519")).toBe("ask");
    expect(lastEffect("read", ".ssh/id_rsa")).toBe("ask");
    expect(lastEffect("read", "deploy/id_deploy_key")).toBe("allow"); // start-anchored id_rsa*
    expect(lastEffect("read", "config/credentials.json")).toBe("ask");
    expect(lastEffect("read", ".git-credentials")).toBe("ask");
    expect(lastEffect("read", "/home/u/.git-credentials")).toBe("ask");
    expect(lastEffect("read", ".netrc")).toBe("ask");
    expect(lastEffect("read", "/home/u/.netrc")).toBe("ask");
    expect(lastEffect("read", "secrets/prod/db.key")).toBe("ask");
    expect(lastEffect("read", ".envrc")).not.toBe("ask"); // only .env and .env.*
    // The documented exception stays readable without an ask.
    expect(lastEffect("read", ".env.example")).toBe("allow");
    expect(lastEffect("read", "ci/.env.example")).toBe("allow");
  });

  it("every SENSITIVE_READ_GLOBS entry carries an ask rule", () => {
    const policy = buildToolPolicy();
    for (const glob of SENSITIVE_READ_GLOBS) {
      expect(policy).toContainEqual({ action: "read", resource: glob, effect: "ask" });
    }
  });

  it("allows exactly the read-only shell allowlist (space-star rules)", () => {
    const policy = buildToolPolicy();
    for (const command of AGENTIC_SHELL_ALLOWS) {
      expect(policy).toContainEqual({ action: "shell", resource: command, effect: "allow" });
    }
    // Every allow is a space-star pattern (bare command matches included).
    for (const rule of policy) {
      if (rule.action === "shell" && rule.effect === "allow") {
        expect(AGENTIC_SHELL_ALLOWS).toContain(rule.resource);
        expect(rule.resource.endsWith(" *")).toBe(true);
      }
    }
  });

  it("asks as the shell fallback for everything else", () => {
    const policy = buildToolPolicy();
    expect(policy).toContainEqual({ action: "shell", resource: "*", effect: "ask" });
    const askIndex = policy.findIndex(
      (rule) => rule.action === "shell" && rule.resource === "*" && rule.effect === "ask",
    );
    const allowIndexes = AGENTIC_SHELL_ALLOWS.map((c) =>
      policy.findIndex((rule) => rule.action === "shell" && rule.resource === c),
    );
    // The broad ask must come BEFORE the allows: last match wins, so the
    // later allow rules beat the fallback where the command matches, and
    // the fallback covers everything the allowlist misses.
    for (const index of allowIndexes) expect(askIndex).toBeLessThan(index);
  });

  it("denies git push AFTER the ask fallback so the deny wins", () => {
    const policy = buildToolPolicy();
    for (const pattern of AGENTIC_SHELL_DENIES) {
      expect(policy).toContainEqual({ action: "shell", resource: pattern, effect: "deny" });
    }
    const askIndex = policy.findIndex((rule) => rule.action === "shell" && rule.effect === "ask");
    const denyIndex = policy.findIndex((rule) => rule.action === "shell" && rule.effect === "deny");
    expect(denyIndex).toBeGreaterThan(askIndex);

    // Effective direction: pushes are denied, allowlisted commands allowed,
    // everything else asks.
    expect(lastEffect("shell", "git push")).toBe("deny");
    expect(lastEffect("shell", "git push origin main")).toBe("deny");
    expect(lastEffect("shell", "git push --force-with-lease origin +main")).toBe("deny");
    expect(lastEffect("shell", "git push -f")).toBe("deny");
    expect(lastEffect("shell", "git status")).toBe("allow");
    expect(lastEffect("shell", "git status -sb")).toBe("allow");
    expect(lastEffect("shell", "git diff --staged")).toBe("allow");
    expect(lastEffect("shell", "git log --oneline -5")).toBe("allow");
    expect(lastEffect("shell", "ls")).toBe("allow");
    expect(lastEffect("shell", "git commit -m nope")).toBe("ask");
    expect(lastEffect("shell", "rm -rf /")).toBe("ask");
    expect(lastEffect("shell", "git checkout -- .")).toBe("ask");
  });

  it("git show is NOT allowlisted — committed blobs ask", () => {
    // `git show HEAD:.env` would dump committed secrets without an ask;
    // the allowlist covers only status/log/diff/blame/branch + ls.
    expect(lastEffect("shell", "git show HEAD:.env")).toBe("ask");
    expect(lastEffect("shell", "git show")).toBe("ask");
    expect(AGENTIC_SHELL_ALLOWS).not.toContain("git show *");
  });

  it("denies every shell command carrying a redirection (arbitrary writes)", () => {
    // The server's shell resource includes the redirect text and
    // redirects get no directory gate — without a deny these would be
    // ALLOWED via the allowlist (`git status *` matches the prefix).
    expect(lastEffect("shell", "git status > ~/.bashrc")).toBe("deny");
    expect(lastEffect("shell", "git diff > /tmp/x")).toBe("deny");
    expect(lastEffect("shell", "git log --format=%x41 > /tmp/exfil")).toBe("deny");
    expect(lastEffect("shell", "git log --format=%x41 >> ~/.ssh/authorized_keys")).toBe("deny");
    expect(lastEffect("shell", "ls > /tmp/listing")).toBe("deny");
    expect(lastEffect("shell", "git log < /dev/null")).toBe("deny");
    expect(lastEffect("shell", "git status")).toBe("allow"); // untouched
    expect(lastEffect("shell", "git diff --staged")).toBe("allow"); // untouched
  });

  it("edits ask; webfetch/websearch deny; the rest ask", () => {
    const policy = buildToolPolicy();
    expect(policy).toContainEqual({ action: "edit", resource: "*", effect: "ask" });
    expect(policy).toContainEqual({ action: "webfetch", resource: "*", effect: "deny" });
    expect(policy).toContainEqual({ action: "websearch", resource: "*", effect: "deny" });
    expect(policy).toContainEqual({ action: "external_directory", resource: "*", effect: "ask" });
    expect(policy).toContainEqual({ action: "subagent", resource: "*", effect: "ask" });
    expect(policy).toContainEqual({ action: "skill", resource: "*", effect: "ask" });
    expect(policy).toContainEqual({ action: "question", resource: "*", effect: "ask" });

    expect(lastEffect("edit", "src/a.ts")).toBe("ask");
    expect(lastEffect("webfetch", "https://example.com")).toBe("deny");
    expect(lastEffect("websearch", "anything")).toBe("deny");
    expect(lastEffect("external_directory", "/etc")).toBe("ask");
  });

  it("rule effects only use the three opencode effects", () => {
    for (const rule of buildToolPolicy()) {
      expect(["allow", "ask", "deny"]).toContain(rule.effect);
    }
  });
});

// ---------------------------------------------------------------------------
// evaluateAsk — the app-side deny duplicate
// ---------------------------------------------------------------------------

describe("evaluateAsk", () => {
  it("auto-rejects shell asks matching a deny pattern", () => {
    expect(evaluateAsk("shell", ["git push"])).toBe("auto-reject");
    expect(evaluateAsk("shell", ["git push origin main"])).toBe("auto-reject");
    expect(evaluateAsk("shell", ["git push --force origin +main"])).toBe("auto-reject");
    expect(evaluateAsk("shell", ["git push\t--force-with-lease"])).toBe("auto-reject");
    expect(evaluateAsk("shell", ["> ~/.bashrc"])).toBe("auto-reject");
  });

  it("auto-rejects the adversarial redirection commands", () => {
    expect(evaluateAsk("shell", ["git status > ~/.bashrc"])).toBe("auto-reject");
    expect(evaluateAsk("shell", ["git diff > /tmp/x"])).toBe("auto-reject");
    expect(evaluateAsk("shell", ["git log --format=%x41 >> ~/.ssh/authorized_keys"])).toBe(
      "auto-reject",
    );
    expect(evaluateAsk("shell", ["git log < /dev/null"])).toBe("auto-reject");
  });

  it("auto-rejects when ANY of the resources is denied", () => {
    expect(evaluateAsk("shell", ["git status", "git push origin main"])).toBe("auto-reject");
    expect(evaluateAsk("shell", ["git status -sb", "git diff > /tmp/x"])).toBe("auto-reject");
  });

  it("forwards non-denied shell asks", () => {
    expect(evaluateAsk("shell", ["git status"])).toBe("forward");
    expect(evaluateAsk("shell", ["npm test"])).toBe("forward");
    expect(evaluateAsk("shell", ["curl https://example.com"])).toBe("forward");
    expect(evaluateAsk("shell", [])).toBe("forward");
  });

  it("forwards non-shell actions even with deny-shaped resources", () => {
    expect(evaluateAsk("read", ["git push"])).toBe("forward");
    expect(evaluateAsk("edit", ["> file"])).toBe("forward");
    expect(evaluateAsk("mcp_server_tool", ["git push"])).toBe("forward");
  });

  it("backslash commands are normalized before the deny check", () => {
    expect(evaluateAsk("shell", ["git push\\origin main"])).toBe("auto-reject");
  });
});
