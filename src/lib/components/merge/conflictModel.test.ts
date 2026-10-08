/**
 * Unit tests for the pure conflict model (`conflictModel.ts`): parsing the
 * D1 path-labeled marker format (and libgit2's label variants), every
 * resolution combination, malformed marker degradation, and multibyte
 * round-trips at the UTF-8 boundary.
 */

import { describe, expect, it } from "vitest";
import {
  applyResolutions,
  countConflicts,
  decodeUtf8,
  effectiveContent,
  encodeUtf8,
  parseConflicts,
  unresolvedConflicts,
  type ConflictSegment,
  type ResolutionChoice,
  type Segment,
} from "./conflictModel";

const PATH = "src/a.ts";

/** D1-exact preview (`render_conflict_markers`): path labels, `\n` endings. */
function d1Preview(ours: string, theirs: string, path = PATH): string {
  const terminated = (s: string): string => (s.length > 0 && !s.endsWith("\n") ? `${s}\n` : s);
  return `<<<<<<< ${path}\n${terminated(ours)}=======\n${terminated(theirs)}>>>>>>> ${path}\n`;
}

/** Conflict segments only, for direct assertions. */
function conflictsOf(segments: Segment[]): ConflictSegment[] {
  return segments.filter((s): s is ConflictSegment => s.type === "conflict");
}

describe("parseConflicts", () => {
  it("parses the exact D1 marker format into line/conflict segments", () => {
    const text = `header\n${d1Preview("our line", "their line")}footer\n`;
    const segments = parseConflicts(text, PATH);

    expect(segments).toHaveLength(3);
    // Boundary newlines live between segments (restored by join), not inside them.
    expect(segments[0]).toEqual({ type: "line", text: "header" });

    const conflict = segments[1]!;
    assertConflict(conflict);
    expect(conflict.ours).toBe("our line");
    expect(conflict.theirs).toBe("their line");
    expect(conflict.oursLabel).toBe(PATH);
    expect(conflict.theirsLabel).toBe(PATH);
    expect(conflict.pathLabeled).toBe(true);
    expect(conflict.raw).toBe(d1Preview("our line", "their line").slice(0, -1));

    expect(segments[2]).toEqual({ type: "line", text: "footer\n" });
  });

  it("round-trips unresolved segments byte-identically", () => {
    const text = d1Preview("α ours\nsecond", "theirs");
    const serialized = applyResolutions(parseConflicts(text, PATH), new Map(), new Map());
    expect(serialized).toBe(text);
  });

  it("flags pathLabeled=false for libgit2's own label variants and still parses", () => {
    const text = [
      "<<<<<<< HEAD",
      "ours",
      "=======",
      "theirs",
      ">>>>>>> feature/x",
      "",
    ].join("\n");
    const [conflict] = conflictsOf(parseConflicts(text, PATH));
    assertConflict(conflict);
    expect(conflict.oursLabel).toBe("HEAD");
    expect(conflict.theirsLabel).toBe("feature/x");
    expect(conflict.pathLabeled).toBe(false);
  });

  it("parses several conflicts in one file, keeping interleaved lines", () => {
    const text = [
      `<<<<<<< ${PATH}`,
      "a-ours",
      "=======",
      "a-theirs",
      `>>>>>>> ${PATH}`,
      "middle",
      `<<<<<<< ${PATH}`,
      "b-ours",
      "=======",
      "b-theirs",
      `>>>>>>> ${PATH}`,
    ].join("\n");
    const segments = parseConflicts(text, PATH);
    expect(countConflicts(segments)).toBe(2);
    expect(segments[1]).toEqual({ type: "line", text: "middle" });
    const [first, second] = conflictsOf(segments);
    expect(first?.ours).toBe("a-ours");
    expect(second?.theirs).toBe("b-theirs");
  });

  it("keeps empty ours/theirs sections and multi-line sections", () => {
    const text = d1Preview("", "t1\nt2");
    const [conflict] = conflictsOf(parseConflicts(text, PATH));
    assertConflict(conflict);
    expect(conflict.ours).toBe("");
    expect(conflict.theirs).toBe("t1\nt2");
  });

  it("treats a trailing newline after the last marker as a final empty line segment", () => {
    const segments = parseConflicts(d1Preview("o", "t"), PATH);
    expect(segments).toHaveLength(2);
    expect(segments[1]).toEqual({ type: "line", text: "" });
  });

  it("returns a single line segment for marker-free text", () => {
    expect(parseConflicts("just\ntext\n", PATH)).toEqual([
      { type: "line", text: "just\ntext\n" },
    ]);
    expect(parseConflicts("", PATH)).toEqual([{ type: "line", text: "" }]);
  });
});

describe("parseConflicts malformed input (defensive, never throws)", () => {
  it("unmatched ======= outside a conflict is literal", () => {
    const segments = parseConflicts("a\n=======\nb\n", PATH);
    expect(segments).toEqual([{ type: "line", text: "a\n=======\nb\n" }]);
    expect(countConflicts(segments)).toBe(0);
  });

  it("opening marker with no separator before EOF degrades the marker line to literal", () => {
    const text = `<<<<<<< ${PATH}\nnever closed\n`;
    const segments = parseConflicts(text, PATH);
    expect(countConflicts(segments)).toBe(0);
    expect(segments).toEqual([{ type: "line", text }]);
  });

  it("separator without closing marker before EOF degrades the opener to literal", () => {
    const text = `<<<<<<< ${PATH}\nours\n=======\nno end anywhere\n`;
    const segments = parseConflicts(text, PATH);
    expect(countConflicts(segments)).toBe(0);
    expect(segments).toEqual([{ type: "line", text }]);
  });

  it("a dangling opener absorbs following markers into one block instead of throwing", () => {
    // The end-marker scan legitimately reaches the next block's `>>>>>>>`;
    // the guarantee is only "never throws, nothing is lost".
    const text = `<<<<<<< ${PATH}\nnever closed\n=======\n${d1Preview("o", "t")}`;
    const segments = parseConflicts(text, PATH);
    expect(countConflicts(segments)).toBe(1);
    const [conflict] = conflictsOf(segments);
    assertConflict(conflict);
    // Unresolved round-trip is still byte-identical.
    expect(applyResolutions(segments, new Map(), new Map())).toBe(text);
  });

  it("loose >>>>>>> marker line is literal", () => {
    const segments = parseConflicts(`>>>>>>> ${PATH}\n`, PATH);
    expect(segments).toEqual([{ type: "line", text: `>>>>>>> ${PATH}\n` }]);
    expect(countConflicts(segments)).toBe(0);
  });

  it("nested-looking <<<<<<< inside a section is handled without throwing", () => {
    // A second opener before the separator: the first separator/end still
    // closes the block; the nested line lands in the ours section.
    const text = [
      `<<<<<<< ${PATH}`,
      "<<<<<<< nested",
      "ours",
      "=======",
      "theirs",
      `>>>>>>> ${PATH}`,
    ].join("\n");
    const [conflict] = conflictsOf(parseConflicts(text, PATH));
    assertConflict(conflict);
    expect(conflict.ours).toBe("<<<<<<< nested\nours");
  });

  it("handles CRLF line endings without losing the \\r", () => {
    const text = [
      `<<<<<<< ${PATH}\r`,
      "ours\r",
      "=======\r",
      "theirs\r",
      `>>>>>>> ${PATH}\r`,
    ].join("\n");
    const [conflict] = conflictsOf(parseConflicts(text, PATH));
    assertConflict(conflict);
    expect(conflict.ours).toBe("ours\r");
    expect(conflict.raw).toBe(text);
  });
});

describe("applyResolutions", () => {
  const twoConflicts = (): Segment[] =>
    parseConflicts(
      [
        `<<<<<<< ${PATH}`,
        "o1",
        "=======",
        "t1",
        `>>>>>>> ${PATH}`,
        "mid",
        `<<<<<<< ${PATH}`,
        "o2",
        "=======",
        "t2",
        `>>>>>>> ${PATH}`,
      ].join("\n"),
      PATH,
    );

  it("chooses ours", () => {
    const out = applyResolutions(twoConflicts(), new Map([[0, "ours" as ResolutionChoice]]), new Map());
    expect(out).toBe("o1\nmid\n<<<<<<< src/a.ts\no2\n=======\nt2\n>>>>>>> src/a.ts");
  });

  it("chooses theirs", () => {
    const out = applyResolutions(twoConflicts(), new Map([[1, "theirs" as ResolutionChoice]]), new Map());
    expect(out).toBe("<<<<<<< src/a.ts\no1\n=======\nt1\n>>>>>>> src/a.ts\nmid\nt2");
  });

  it("uses custom text (boundary newline comes from the join, not the content)", () => {
    const out = applyResolutions(
      twoConflicts(),
      new Map([[0, "custom" as ResolutionChoice]]),
      new Map([[0, "hand merged"]]),
    );
    expect(out.startsWith("hand merged\nmid\n")).toBe(true);
  });

  it("custom choice with missing custom text falls back to ours", () => {
    const out = applyResolutions(twoConflicts(), new Map([[0, "custom" as ResolutionChoice]]), new Map());
    expect(out.startsWith("o1\n")).toBe(true);
  });

  it("custom text with no explicit choice wins over the buttons (documented precedence)", () => {
    const out = applyResolutions(
      twoConflicts(),
      new Map<number, ResolutionChoice>([[1, "theirs"]]),
      new Map<number, string>([[0, "edited"]]),
    );
    expect(out.startsWith("edited\n")).toBe(true);
    expect(out.endsWith("t2")).toBe(true);
  });

  it("resolves every combination at once", () => {
    const out = applyResolutions(
      twoConflicts(),
      new Map<number, ResolutionChoice>([
        [0, "ours"],
        [1, "theirs"],
      ]),
      new Map<number, string>(),
    );
    expect(out).toBe("o1\nmid\nt2");
  });

  it("keeps marker text for conflicts left unresolved (mixed with resolved)", () => {
    const segments = twoConflicts();
    const out = applyResolutions(segments, new Map(), new Map());
    expect(countConflicts(parseConflicts(out, PATH))).toBe(2);
  });

  it("preserves multiline and multibyte custom content", () => {
    const segments = parseConflicts(d1Preview("o", "t"), PATH);
    const out = applyResolutions(
      segments,
      new Map(),
      new Map([[0, "你好\nwörld ✓"]]),
    );
    expect(out).toBe("你好\nwörld ✓\n");
  });

  it("empty custom text resolves the conflict to an empty section newline", () => {
    const segments = parseConflicts(d1Preview("o", "t"), PATH);
    const out = applyResolutions(segments, new Map(), new Map([[0, ""]]));
    expect(out).toBe("\n");
  });
});

describe("countConflicts / unresolvedConflicts / effectiveContent", () => {
  const segments = (): Segment[] =>
    parseConflicts(
      [
        `<<<<<<< ${PATH}`,
        "o1",
        "=======",
        "t1",
        `>>>>>>> ${PATH}`,
        `<<<<<<< ${PATH}`,
        "o2",
        "=======",
        "t2",
        `>>>>>>> ${PATH}`,
      ].join("\n"),
      PATH,
    );

  it("counts conflicts only", () => {
    expect(countConflicts(segments())).toBe(2);
    expect(countConflicts(parseConflicts("plain", PATH))).toBe(0);
  });

  it("lists unresolved indices, shrinking as choices land", () => {
    const segs = segments();
    const choices = new Map<number, ResolutionChoice>();
    const customs = new Map<number, string>();
    expect(unresolvedConflicts(segs, choices, customs)).toEqual([0, 1]);
    choices.set(0, "theirs");
    expect(unresolvedConflicts(segs, choices, customs)).toEqual([1]);
    customs.set(1, "edit");
    expect(unresolvedConflicts(segs, choices, customs)).toEqual([]);
  });

  it("effectiveContent: custom wins, then the chosen side, then ours", () => {
    const [conflict] = conflictsOf(segments());
    expect(conflict).toBeDefined();
    const c = conflict!;
    expect(effectiveContent(c, new Map(), new Map(), 0)).toBe("o1");
    expect(effectiveContent(c, new Map([[0, "theirs"]]), new Map(), 0)).toBe("t1");
    expect(effectiveContent(c, new Map([[0, "ours"]]), new Map([[0, "edit"]]), 0)).toBe("edit");
  });
});

describe("UTF-8 boundary", () => {
  it("decodes multibyte bytes exactly (emojis, CJK, combining marks)", () => {
    const text = "<<<<<<< ünïcode ✓\n中文 🎉\n=======\nñ\n>>>>>>> ünïcode ✓\n";
    const bytes = encodeUtf8(text);
    // (No `instanceof Uint8Array` here: jsdom hands out cross-realm arrays.)
    expect(Array.from(bytes.slice(0, 3))).toEqual([0x3c, 0x3c, 0x3c]);
    const round = parseConflicts(decodeUtf8(Array.from(bytes)), "ünïcode ✓");
    const [conflict] = conflictsOf(round);
    assertConflict(conflict);
    expect(conflict.ours).toBe("中文 🎉");
    expect(conflict.oursLabel).toBe("ünïcode ✓");
    expect(decodeUtf8(encodeUtf8(text))).toBe(text);
  });

  it("decodeUtf8 accepts both number[] and Uint8Array", () => {
    const bytes = encodeUtf8("héllo");
    expect(decodeUtf8(Array.from(bytes))).toBe("héllo");
    expect(decodeUtf8(bytes)).toBe("héllo");
  });

  it("full pipeline: parse → resolve → encode round-trips multibyte content", () => {
    const original = d1Preview("моя строка", "彼の行");
    const segments = parseConflicts(decodeUtf8(encodeUtf8(original)), PATH);
    const out = applyResolutions(segments, new Map([[0, "theirs" as ResolutionChoice]]), new Map());
    expect(decodeUtf8(encodeUtf8(out))).toBe("彼の行\n");
  });
});

function assertConflict(value: Segment | undefined): asserts value is ConflictSegment {
  expect(value?.type).toBe("conflict");
}
