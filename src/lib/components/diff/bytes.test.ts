/**
 * Unit tests for byte-safe highlight handling (diff/bytes.ts).
 *
 * `DiffLine.highlights` are UTF-8 BYTE offsets; JS strings are UTF-16. These
 * tests pin the conversion (incl. 2/3/4-byte chars and surrogate pairs) and
 * the outward-clamping policy for offsets landing mid-character.
 */

import { describe, expect, it } from "vitest";
import {
  applyHighlights,
  byteToCharIndex,
  sliceByBytes,
  utf8Length,
  type TextSegment,
} from "$lib/components/diff/bytes";

function plain(text: string): TextSegment[] {
  return [{ text, hl: false }];
}

function join(segments: TextSegment[]): string {
  return segments.map((s) => s.text).join("");
}

// ASCII ---------------------------------------------------------------------

describe("utf8Length", () => {
  it("is the char count for pure ASCII", () => {
    expect(utf8Length("hello brave")).toBe(11);
    expect(utf8Length("")).toBe(0);
  });

  it("counts multi-byte code points correctly", () => {
    expect(utf8Length("é")).toBe(2); // U+00E9, 2 bytes
    expect(utf8Length("€")).toBe(3); // U+20AC, 3 bytes
    expect(utf8Length("中")).toBe(3); // U+4E2D, 3 bytes
    expect(utf8Length("\u{1F600}")).toBe(4); // emoji, surrogate pair in UTF-16
    expect(utf8Length("\u{1D11E}")).toBe(4); // musical symbol G clef
  });

  it("sums mixed strings", () => {
    // a(1) é(2) 中(3) 😀(4) b(1) = 11 bytes, 5 chars (6 UTF-16 units).
    expect(utf8Length("aé中\u{1F600}b")).toBe(11);
  });
});

// byteToCharIndex -----------------------------------------------------------

describe("byteToCharIndex", () => {
  it("maps ASCII offsets 1:1", () => {
    const text = "hello world";
    expect(byteToCharIndex(text, 6, "down")).toBe(6);
    expect(byteToCharIndex(text, 6, "up")).toBe(6);
    expect(byteToCharIndex(text, 11, "down")).toBe(11);
    expect(byteToCharIndex(text, 11, "up")).toBe(11);
  });

  it("maps offsets around 2-byte chars exactly on boundaries", () => {
    const text = "héllo"; // h=0, é=1..2, l=3, l=4, o=5
    expect(byteToCharIndex(text, 0, "down")).toBe(0);
    expect(byteToCharIndex(text, 1, "down")).toBe(1);
    expect(byteToCharIndex(text, 3, "down")).toBe(2); // byte 3 = 'l' at char 2
    expect(byteToCharIndex(text, 3, "up")).toBe(2);
  });

  it("clamps outward when the offset lands mid-character", () => {
    const text = "héllo"; // é occupies bytes 1-2
    expect(byteToCharIndex(text, 2, "down")).toBe(1); // mid-é → char start
    expect(byteToCharIndex(text, 2, "up")).toBe(2); // mid-é → char end
  });

  it("treats surrogate pairs as single 4-byte chars", () => {
    const text = "a\u{1F600}b"; // a=0, emoji=1..4, b=5 (chars: 0,1,3)
    expect(byteToCharIndex(text, 1, "down")).toBe(1);
    expect(byteToCharIndex(text, 5, "down")).toBe(3);
    expect(byteToCharIndex(text, 5, "up")).toBe(3);
    expect(byteToCharIndex(text, 3, "down")).toBe(1); // mid-emoji (low surrogate)
    expect(byteToCharIndex(text, 3, "up")).toBe(3);
  });

  it("clamps out-of-range offsets to the string bounds", () => {
    expect(byteToCharIndex("abc", -5, "down")).toBe(0);
    expect(byteToCharIndex("abc", -5, "up")).toBe(0);
    expect(byteToCharIndex("abc", 99, "down")).toBe(3);
    expect(byteToCharIndex("abc", 99, "up")).toBe(3);
    expect(byteToCharIndex("", 0, "down")).toBe(0);
  });
});

// sliceByBytes ---------------------------------------------------------------

describe("sliceByBytes", () => {
  it("slices ASCII like String.slice", () => {
    expect(sliceByBytes("hello brave", 6, 11)).toBe("brave");
  });

  it("slices ranges bounded by multi-byte chars", () => {
    const text = "héllo wörld"; // é=bytes1-2, ö=bytes8-9
    expect(sliceByBytes(text, 0, 6)).toBe("héllo");
    expect(sliceByBytes(text, 7, 11)).toBe("wör"); // bytes 7..10 = w, ö, r
    expect(sliceByBytes(text, 7, utf8Length(text))).toBe("wörld");
  });

  it("includes chars the range touches (outward clamp)", () => {
    const text = "aéz"; // a=0, é=1-2, z=3
    expect(sliceByBytes(text, 2, 3)).toBe("é"); // start mid-é → include é
    expect(sliceByBytes(text, 0, 2)).toBe("aé"); // end mid-é → include é
    expect(sliceByBytes(text, 1, 2)).toBe("é"); // exactly é
  });

  it("never splits a surrogate pair", () => {
    const text = "a\u{1F600}b";
    expect(sliceByBytes(text, 2, 3)).toBe("\u{1F600}"); // middle of the pair
    expect(sliceByBytes(text, 1, 5)).toBe("\u{1F600}");
  });

  it("returns empty for degenerate ranges", () => {
    expect(sliceByBytes("abc", 2, 2)).toBe("");
    expect(sliceByBytes("abc", 5, 9)).toBe("");
    expect(sliceByBytes("", 0, 1)).toBe("");
    expect(sliceByBytes("abc", -2, 1)).toBe("a");
  });

  it("handles reversed ranges by normalizing", () => {
    expect(sliceByBytes("hello", 3, 1)).toBe("el");
  });
});

// applyHighlights -----------------------------------------------------------

describe("applyHighlights", () => {
  it("returns one plain segment when there are no highlights", () => {
    expect(applyHighlights("hello", [])).toEqual(plain("hello"));
    expect(applyHighlights("hello", null)).toEqual(plain("hello"));
    expect(applyHighlights("hello", undefined)).toEqual(plain("hello"));
    expect(applyHighlights("", [[0, 1]])).toEqual([]);
    expect(applyHighlights("", [])).toEqual([]);
  });

  it("marks the engine's canonical case (Rust test parity)", () => {
    // word_highlights_mark_changed_words: "hello world" → "hello brave",
    // both sides highlight (6, 11).
    expect(applyHighlights("hello world", [[6, 11]])).toEqual([
      { text: "hello ", hl: false },
      { text: "world", hl: true },
    ]);
    expect(applyHighlights("hello brave", [[6, 11]])).toEqual([
      { text: "hello ", hl: false },
      { text: "brave", hl: true },
    ]);
  });

  it("supports multiple disjoint ranges", () => {
    expect(applyHighlights("a-b-c", [[1, 2], [3, 4]])).toEqual([
      { text: "a", hl: false },
      { text: "-", hl: true },
      { text: "b", hl: false },
      { text: "-", hl: true },
      { text: "c", hl: false },
    ]);
  });

  it("merges overlapping and adjacent ranges", () => {
    expect(applyHighlights("abcdef", [[1, 3], [2, 5]])).toEqual([
      { text: "a", hl: false },
      { text: "bcdef".slice(0, 4), hl: true }, // bcde
      { text: "f", hl: false },
    ]);
    expect(applyHighlights("abcdef", [[1, 3], [3, 4]])).toEqual([
      { text: "a", hl: false },
      { text: "bcd", hl: true },
      { text: "ef", hl: false },
    ]);
  });

  it("drops degenerate and out-of-bounds ranges", () => {
    expect(applyHighlights("abc", [[1, 1], [5, 9]])).toEqual(plain("abc"));
    expect(applyHighlights("abc", [[0, 99]])).toEqual([{ text: "abc", hl: true }]);
    expect(applyHighlights("abc", [[-3, 0]])).toEqual(plain("abc"));
  });

  it("normalizes reversed ranges", () => {
    expect(applyHighlights("abcd", [[3, 1]])).toEqual([
      { text: "a", hl: false },
      { text: "bc", hl: true },
      { text: "d", hl: false },
    ]);
  });

  it("handles leading and trailing highlights", () => {
    expect(applyHighlights("abcd", [[0, 1]])).toEqual([
      { text: "a", hl: true },
      { text: "bcd", hl: false },
    ]);
    expect(applyHighlights("abcd", [[3, 4]])).toEqual([
      { text: "abc", hl: false },
      { text: "d", hl: true },
    ]);
    expect(applyHighlights("abcd", [[0, 4]])).toEqual([{ text: "abcd", hl: true }]);
  });

  it("is byte-accurate on multibyte text", () => {
    // "héllo wörld": h=0 é=1-2 l=3 l=4 o=5 ' '=6 w=7 ö=8-9 r=10 l=11 d=12
    // Changed word "wörld" = bytes 7..13.
    expect(applyHighlights("héllo wörld", [[7, 13]])).toEqual([
      { text: "héllo ", hl: false },
      { text: "wörld", hl: true },
    ]);
    // Only ö changed: bytes 8..10 cover just ö (r starts at byte 10).
    expect(applyHighlights("héllo wörld", [[8, 10]])).toEqual([
      { text: "héllo w", hl: false },
      { text: "ö", hl: true },
      { text: "rld", hl: false },
    ]);
  });

  it("highlights CJK text where chars are 3 bytes each", () => {
    // 你=0-2 好=3-5 世=6-8 界=9-11; changed word 世界 = bytes 6..12.
    expect(applyHighlights("你好世界", [[6, 12]])).toEqual([
      { text: "你好", hl: false },
      { text: "世界", hl: true },
    ]);
  });

  it("clamps outward when a range boundary splits a character", () => {
    const text = "aéz"; // a=0, é=1-2, z=3
    expect(applyHighlights(text, [[2, 3]])).toEqual([
      { text: "a", hl: false },
      { text: "é", hl: true },
      { text: "z", hl: false },
    ]);
    expect(applyHighlights(text, [[0, 2]])).toEqual([
      { text: "aé", hl: true },
      { text: "z", hl: false },
    ]);
  });

  it("never splits surrogate pairs", () => {
    const text = "x\u{1F600}y"; // x=0, emoji=1-4, y=5
    expect(applyHighlights(text, [[1, 3]])).toEqual([
      { text: "x", hl: false },
      { text: "\u{1F600}", hl: true },
      { text: "y", hl: false },
    ]);
  });

  it("always reconstructs the input exactly", () => {
    const cases: Array<[string, [number, number][]]> = [
      ["héllo wörld", [[7, 13], [0, 1]]],
      ["aé中\u{1F600}b", [[1, 5], [2, 8], [10, 11]]],
      ["你好世界", [[3, 4], [6, 12]]],
    ];
    for (const [text, ranges] of cases) {
      expect(join(applyHighlights(text, ranges))).toBe(text);
    }
  });
});
