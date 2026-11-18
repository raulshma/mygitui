/**
 * Tests for the `.gitattributes` model (panels/attributesModel.ts): parse /
 * serialize round-trips, gitignore-class pattern matching, attribute
 * evaluation across rules, and the LFS / lockable toggles.
 */
import { describe, expect, it } from "vitest";
import {
  attributeMentionFor,
  matchesGitAttributesPattern,
  parseGitAttributes,
  ruleIsLfs,
  serializeGitAttributes,
  setLfsRule,
  setLockableRule,
  upsertRule,
} from "./attributesModel";

describe("parseGitAttributes", () => {
  it("parses rules with plain, valued, unset and unspecified attributes", () => {
    const lines = parseGitAttributes("*.png filter=lfs diff -text !merge");
    expect(lines).toEqual([
      { kind: "rule", pattern: "*.png", attrs: ["filter=lfs", "diff", "-text", "!merge"] },
    ]);
  });

  it("preserves comments and blank lines as raw entries", () => {
    const lines = parseGitAttributes("# images\n\n*.jpg binary");
    expect(lines).toEqual([
      { kind: "raw", text: "# images" },
      { kind: "raw", text: "" },
      { kind: "rule", pattern: "*.jpg", attrs: ["binary"] },
    ]);
  });

  it("supports quoted patterns containing spaces", () => {
    const lines = parseGitAttributes('"my docs/*.md" text');
    expect(lines).toEqual([{ kind: "rule", pattern: "my docs/*.md", attrs: ["text"] }]);
  });

  it("treats the first token as the pattern, even a -prefixed one", () => {
    // gitattributes grammar: the head token is always the pattern (so
    // `-binary` alone is a rule matching a file literally named "-binary").
    expect(parseGitAttributes("-binary")).toEqual([
      { kind: "rule", pattern: "-binary", attrs: [] },
    ]);
  });

  it("keeps an empty text as one raw line", () => {
    expect(parseGitAttributes("")).toEqual([{ kind: "raw", text: "" }]);
  });
});

describe("serializeGitAttributes (round-trip)", () => {
  it("round-trips untouched text verbatim", () => {
    const text = "# header\n\n*.png filter=lfs\n/assets/** -filter=lfs\n";
    expect(serializeGitAttributes(parseGitAttributes(text))).toBe(text);
  });

  it("re-serializes edited rules canonically and quotes spaced patterns", () => {
    const lines = parseGitAttributes('"my docs/*.md" text');
    const serialized = serializeGitAttributes(lines);
    expect(serialized).toBe('"my docs/*.md" text');
    expect(serializeGitAttributes(parseGitAttributes(serializeGitAttributes(lines)))).toBe(
      serialized,
    );
  });
});

describe("matchesGitAttributesPattern", () => {
  it("flat patterns match the basename at any depth", () => {
    expect(matchesGitAttributesPattern("*.log", "a/b/c/error.log")).toBe(true);
    expect(matchesGitAttributesPattern("*.log", "error.log")).toBe(true);
    expect(matchesGitAttributesPattern("*.log", "a/b/log.txt")).toBe(false);
  });

  it("anchored patterns match from the root only", () => {
    expect(matchesGitAttributesPattern("/assets/*.psd", "assets/x.psd")).toBe(true);
    expect(matchesGitAttributesPattern("/assets/*.psd", "deep/assets/x.psd")).toBe(false);
  });

  it("a trailing slash matches everything inside the directory", () => {
    expect(matchesGitAttributesPattern("assets/", "assets/a/b.bin")).toBe(true);
    expect(matchesGitAttributesPattern("assets/", "other/a.bin")).toBe(false);
  });

  it("handles ** crossing directories and ? single chars", () => {
    expect(matchesGitAttributesPattern("assets/**/*.bin", "assets/a/b.bin")).toBe(true);
    expect(matchesGitAttributesPattern("a?c", "abc")).toBe(true);
    expect(matchesGitAttributesPattern("a?c", "a/c")).toBe(false);
  });
});

describe("attribute evaluation", () => {
  const lines = parseGitAttributes(
    [
      "* text=auto",
      "*.png filter=lfs",
      "vendor/** -text",
      "*.png -filter=lfs", // later rule overrides
      "final.psd filter=lfs",
    ].join("\n"),
  );

  it("later matching rules override earlier ones", () => {
    expect(attributeMentionFor(lines, "x.png", "filter")).toEqual({ state: "value", value: "lfs" });
    // "*.png -filter=lfs" wins for pngs; final.psd is not a png.
    expect(attributeMentionFor(lines, "final.psd", "filter")).toEqual({
      state: "value",
      value: "lfs",
    });
  });

  it("distinguishes set / unset / unspecified / value / missing", () => {
    expect(attributeMentionFor(lines, "vendor/lib.c", "text")).toEqual({ state: "unset" });
    expect(attributeMentionFor(lines, "readme.md", "text")).toEqual({ state: "value", value: "auto" });
    expect(attributeMentionFor(lines, "readme.md", "filter")).toBeNull();
    const withBang = parseGitAttributes("*.bin !diff");
    expect(attributeMentionFor(withBang, "a.bin", "diff")).toEqual({ state: "unspecified" });
  });

  it("ruleIsLfs reads the filter mention", () => {
    const rules = parseGitAttributes("a.psd filter=lfs\nb.psd -filter=lfs\nc.psd text");
    const [, b, c] = rules;
    expect(ruleIsLfs(rules[0] as { attrs: string[] })).toBe(true);
    expect(ruleIsLfs(b as { attrs: string[] })).toBe(false);
    expect(ruleIsLfs(c as { attrs: string[] })).toBe(false);
  });
});

describe("edit ops (upsert / lfs / lockable)", () => {
  it("upsertRule replaces the attrs of the same pattern and appends new ones", () => {
    const base = parseGitAttributes("*.png filter=lfs diff\n*.jpg binary");
    const updated = upsertRule(base, "*.png", ["filter=lfs"]);
    expect(serializeGitAttributes(updated)).toBe("*.png filter=lfs\n*.jpg binary");
    const appended = upsertRule(updated, "*.psd", ["filter=lfs"]);
    expect(serializeGitAttributes(appended)).toBe("*.png filter=lfs\n*.jpg binary\n*.psd filter=lfs");
    // Other rules and raws are untouched.
    const withRaw = parseGitAttributes("# note\n*.png a b");
    expect(serializeGitAttributes(upsertRule(withRaw, "*.png", ["c"]))).toBe("# note\n*.png c");
  });

  it("setLfsRule adds filter=lfs / -filter=lfs preserving other attrs", () => {
    const base = parseGitAttributes("*.png diff");
    const on = serializeGitAttributes(setLfsRule(base, "*.png", true));
    expect(on).toBe("*.png diff filter=lfs");
    const off = serializeGitAttributes(setLfsRule(setLfsRule(base, "*.png", true), "*.png", false));
    expect(off).toBe("*.png diff -filter=lfs");
  });

  it("setLfsRule replaces an existing filter mention in place", () => {
    const base = parseGitAttributes("*.png filter=lfs");
    expect(serializeGitAttributes(setLfsRule(base, "*.png", false))).toBe("*.png -filter=lfs");
  });

  it("setLockableRule toggles lockable, keeping the rule's other attrs", () => {
    const base = parseGitAttributes("secrets/* filter=lfs");
    const locked = serializeGitAttributes(setLockableRule(base, "secrets/*", true));
    expect(locked).toBe("secrets/* filter=lfs lockable");
    const unlocked = serializeGitAttributes(setLockableRule(base, "secrets/*", false));
    expect(unlocked).toBe("secrets/* filter=lfs -lockable");
  });
});
