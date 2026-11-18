/**
 * Pure `.gitattributes` model (batch B item 40) — parse, serialize, match,
 * and edit the file's rule lines without touching git.
 *
 * Line grammar (gitattributes(5), simplified where noted):
 *   `pattern attr attr=value -attr !attr` — whitespace-separated; a pattern
 *   containing whitespace must be double-quoted. `attr` sets, `-attr`
 *   unsets, `!attr` marks unspecified, `attr=value` assigns (quoted values
 *   keep their quotes out of the value). `#` comments and blank lines are
 *   preserved as raw lines so a round-trip edit does not rewrite the file.
 *
 * Pattern matching follows the gitignore semantics class (same as
 * gitattributes): a pattern containing a `/` (before its trailing position)
 * anchors at the repo root; otherwise it matches the basename at any depth.
 * `*` crosses nothing but `**`, `?` is one non-slash char, `[...]` classes
 * pass through. This is deliberately the same dialect family as the
 * ignore-pattern helpers (`gitignore.ts` builds candidate patterns; here we
 * evaluate existing ones).
 */

/** One parsed line: a rule, or a preserved comment/blank line. */
export type AttributesLine =
  | { kind: "rule"; pattern: string; attrs: string[] }
  | { kind: "raw"; text: string };

/** A quoted (or plain) token from the line head. */
function splitToken(line: string, offset: number): { token: string; end: number } | null {
  if (line[offset] === '"') {
    const close = line.indexOf('"', offset + 1);
    if (close === -1) return null; // unterminated quote: treat rest as token
    return { token: line.slice(offset + 1, close), end: close + 1 };
  }
  let end = offset;
  while (end < line.length && !/\s/.test(line[end]!)) end++;
  return { token: line.slice(offset, end), end };
}

/** Parses `.gitattributes` text into lines (rules + preserved raws). */
export function parseGitAttributes(text: string): AttributesLine[] {
  const lines: AttributesLine[] = [];
  for (const rawLine of text.split("\n")) {
    const trimmed = rawLine.trim();
    if (trimmed === "" || trimmed.startsWith("#")) {
      lines.push({ kind: "raw", text: rawLine });
      continue;
    }
    const head = splitToken(trimmed, 0);
    if (head === null || head.token === "") {
      lines.push({ kind: "raw", text: rawLine });
      continue;
    }
    const attrs: string[] = [];
    let cursor = head.end;
    while (cursor < trimmed.length) {
      if (/\s/.test(trimmed[cursor]!)) {
        cursor++;
        continue;
      }
      const next = splitToken(trimmed, cursor);
      if (next === null || next.token === "") break;
      attrs.push(next.token);
      cursor = next.end;
    }
    lines.push({ kind: "rule", pattern: head.token, attrs });
  }
  return lines;
}

/** Serializes lines back to file text (rules re-rendered canonically). */
export function serializeGitAttributes(lines: readonly AttributesLine[]): string {
  return lines
    .map((line) => {
      if (line.kind === "raw") return line.text;
      const pattern = /\s/.test(line.pattern) ? `"${line.pattern}"` : line.pattern;
      return [pattern, ...line.attrs].join(" ");
    })
    .join("\n");
}

/** Splits a path into its directory part and basename (forward slashes). */
function basenameOf(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash === -1 ? path : path.slice(slash + 1);
}

/** Converts a glob pattern into a anchored-to-its-scope regex body. */
function patternToRegexBody(pattern: string): string {
  let out = "";
  for (let i = 0; i < pattern.length; i++) {
    const ch = pattern[i]!;
    if (ch === "*") {
      if (pattern[i + 1] === "*") {
        out += ".*";
        i++;
        continue;
      }
      out += "[^/]*";
      continue;
    }
    if (ch === "?") {
      out += "[^/]";
      continue;
    }
    if (ch === "[") {
      const close = pattern.indexOf("]", i + 1);
      if (close !== -1) {
        out += pattern.slice(i, close + 1);
        i = close;
        continue;
      }
    }
    out += ch.replace(/[.+^${}()|\\]/g, "\\$&");
  }
  return out;
}

/**
 * Gitignore-class match: patterns with an inner `/` anchor at the root
 * (a trailing `/` means "everything inside this directory"); flat patterns
 * match the basename at any depth.
 */
export function matchesGitAttributesPattern(pattern: string, path: string): boolean {
  const p = pattern.trim();
  const normalized = path.replace(/\\/g, "/").replace(/^\.\//, "");
  if (p === "" || normalized === "") return false;
  const dirOnly = p.endsWith("/");
  const body = (dirOnly ? p.slice(0, -1) : p).replace(/^\/+/, "");
  if (body === "") return false;
  const anchored = body.includes("/");
  const regexBody = patternToRegexBody(body);
  if (anchored) {
    // `/src/` (dir) matches everything below it; `/src/tmp/*.log` matches
    // the exact path (or anything below, with `**`/trailing `*`). The
    // leading `/` is the anchor marker, not a literal path segment.
    const full = dirOnly ? new RegExp(`^${regexBody}(/.*)?$`) : new RegExp(`^${regexBody}$`);
    return full.test(normalized);
  }
  const base = new RegExp(`^${regexBody}$`);
  if (!dirOnly) return base.test(basenameOf(normalized));
  // Flat dir pattern (`assets/`): a directory named `body` at any depth —
  // everything below it matches.
  const segment = new RegExp(`(?:^|/)${regexBody}/`);
  return segment.test(normalized);
}

/** How a rule speaks about one attribute. */
export type AttrMention =
  | { state: "set" }
  | { state: "unset" }
  | { state: "unspecified" }
  | { state: "value"; value: string };

/** Reads one rule's mention of `attr` (`attr` / `-attr` / `!attr` / `a=v`). */
export function ruleAttrMention(rule: { attrs: readonly string[] }, attr: string): AttrMention | null {
  for (const token of rule.attrs) {
    if (token === attr) return { state: "set" };
    if (token === `-${attr}`) return { state: "unset" };
    if (token === `!${attr}`) return { state: "unspecified" };
    if (token.startsWith(`${attr}=`)) {
      return { state: "value", value: token.slice(attr.length + 1) };
    }
  }
  return null;
}

/**
 * The effective mention of `attr` for `path` across the parsed rules:
 * later rules override earlier ones (gitattributes evaluation order);
 * `null` when no matching rule mentions the attribute at all.
 */
export function attributeMentionFor(
  lines: readonly AttributesLine[],
  path: string,
  attr: string,
): AttrMention | null {
  let found: AttrMention | null = null;
  for (const line of lines) {
    if (line.kind !== "rule") continue;
    if (!matchesGitAttributesPattern(line.pattern, path)) continue;
    const mention = ruleAttrMention(line, attr);
    if (mention !== null) found = mention;
  }
  return found;
}

/** True when the rule turns `filter=lfs` on for its pattern. */
export function ruleIsLfs(rule: { attrs: readonly string[] }): boolean {
  const mention = ruleAttrMention(rule, "filter");
  return mention?.state === "value" && mention.value === "lfs";
}

/**
 * Upserts the rule for `pattern`: the first rule with that exact pattern
 * gets its attribute tokens replaced by `attrs` (order kept); with no
 * existing rule the new one appends at the end. Other rules are untouched.
 */
export function upsertRule(
  lines: readonly AttributesLine[],
  pattern: string,
  attrs: readonly string[],
): AttributesLine[] {
  const index = lines.findIndex(
    (line) => line.kind === "rule" && line.pattern === pattern,
  );
  if (index === -1) {
    return [...lines, { kind: "rule", pattern, attrs: [...attrs] }];
  }
  return lines.map((line, i) =>
    i === index && line.kind === "rule" ? { ...line, attrs: [...attrs] } : line,
  );
}

/**
 * Toggles `filter=lfs` on a pattern: on = the rule keeps its other tokens
 * with `filter=lfs` set; off = `filter` becomes `-filter=lfs` (an explicit
 * unset — a pattern later in the file would otherwise re-enable it). A
 * missing rule is created, matching the `git lfs track` / `untrack` file
 * effect.
 */
export function setLfsRule(
  lines: readonly AttributesLine[],
  pattern: string,
  enabled: boolean,
): AttributesLine[] {
  const existing = lines.find((line) => line.kind === "rule" && line.pattern === pattern);
  const kept =
    existing !== undefined && existing.kind === "rule"
      ? existing.attrs.filter((token) => !/^!?filter(=|$)/.test(token))
      : [];
  const attrs = enabled
    ? [...kept, "filter=lfs"]
    : [...kept, "-filter=lfs"];
  return upsertRule(lines, pattern, attrs);
}

/**
 * Toggles `lockable` on a pattern the same way (`git lfs lock` needs the
 * attribute; the lock/unlock commands themselves are backend-side).
 */
export function setLockableRule(
  lines: readonly AttributesLine[],
  pattern: string,
  enabled: boolean,
): AttributesLine[] {
  const existing = lines.find((line) => line.kind === "rule" && line.pattern === pattern);
  const kept =
    existing !== undefined && existing.kind === "rule"
      ? existing.attrs.filter((token) => !/^!?lockable$/.test(token))
      : [];
  const attrs = enabled ? [...kept, "lockable"] : [...kept, "-lockable"];
  return upsertRule(lines, pattern, attrs);
}
