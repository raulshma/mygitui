/**
 * Unit tests for the AI diff budgeter: stat summaries, generated-path
 * detection, per-file slicing, and the full budgeted rendering
 * (fair slices across files, explicit truncation/omission notes, hard
 * total cap).
 */

import { describe, expect, it } from "vitest";
import type { DiffHunk, FileDiff } from "$lib/ipc/types";
import {
  budgetedDiffText,
  isGeneratedPath,
  MAX_DIFF_CHARS,
  renderFileSlice,
  statSummaryText,
} from "./diffBudget";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function hunk(lines: Array<[string, string]>, oldStart = 1, newStart = 1): DiffHunk {
  return {
    old_start: oldStart,
    new_start: newStart,
    lines: lines.map(([origin, text], i) => ({
      old_no: origin === "+" ? null : i + 1,
      new_no: origin === "-" ? null : i + 1,
      origin,
      text,
      highlights: [],
    })),
  };
}

function file(path: string, hunkLines: Array<[string, string]>, extras?: Partial<FileDiff>): FileDiff {
  const hunks = [hunk(hunkLines)];
  return {
    path,
    old_path: null,
    binary: false,
    is_image: false,
    additions: hunkLines.filter(([o]) => o === "+").length,
    deletions: hunkLines.filter(([o]) => o === "-").length,
    hunks,
    ...extras,
  };
}

const A = file("src/a.ts", [
  [" ", "const a = 1;"],
  ["-", "const b = 2;"],
  ["+", "const b = 3;"],
]);

/** A file with `n` changed lines of ~40 chars each. */
function bigFile(path: string, n: number): FileDiff {
  return file(
    path,
    Array.from({ length: n }, (_, i) => (i % 2 === 0 ? ["+", `const generated${i} = "some reasonably long line";`] : ["-", `const removed${i} = "another fairly long line here";`]) as [string, string]),
  );
}

// ---------------------------------------------------------------------------
// isGeneratedPath
// ---------------------------------------------------------------------------

describe("isGeneratedPath", () => {
  it("flags lockfiles and vendored/minified paths", () => {
    expect(isGeneratedPath("package-lock.json")).toBe(true);
    expect(isGeneratedPath("web/yarn.lock")).toBe(true);
    expect(isGeneratedPath("Cargo.lock")).toBe(true);
    expect(isGeneratedPath("go.sum")).toBe(true);
    expect(isGeneratedPath("app/dist/bundle.js")).toBe(true);
    expect(isGeneratedPath("vendor/lib.rs")).toBe(true);
    expect(isGeneratedPath("ui/app.min.js")).toBe(true);
    expect(isGeneratedPath("ui/app.min.css")).toBe(true);
    expect(isGeneratedPath("ui/app.js.map")).toBe(true);
  });

  it("leaves real source alone", () => {
    expect(isGeneratedPath("src/a.ts")).toBe(false);
    expect(isGeneratedPath("src/distribute.rs")).toBe(false); // no bare 'dist' match
    expect(isGeneratedPath("Cargo.toml")).toBe(false);
    expect(isGeneratedPath("src/minified.ts")).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// statSummaryText
// ---------------------------------------------------------------------------

describe("statSummaryText", () => {
  it("lists every file with +/- counts", () => {
    const text = statSummaryText([A, file("b.ts", [["+", "x"]])]);
    expect(text).toContain("Files changed (2):");
    expect(text).toContain("- src/a.ts (+1/-1)");
    expect(text).toContain("- b.ts (+1/-0)");
  });

  it("marks binary files and omits nothing else", () => {
    const text = statSummaryText([file("img.png", [], { binary: true })]);
    expect(text).toContain("- img.png (binary) (+0/-0)");
  });

  it("collapses beyond maxEntries with an explicit count", () => {
    const files = Array.from({ length: 107 }, (_, i) => file(`f${i}.ts`, [["+", "x"]]));
    const text = statSummaryText(files, 50);
    expect(text).toContain("Files changed (107):");
    expect(text).toContain("- f49.ts");
    expect(text).not.toContain("- f50.ts");
    expect(text).toContain("… and 57 more files (not listed)");
  });
});

// ---------------------------------------------------------------------------
// renderFileSlice
// ---------------------------------------------------------------------------

describe("renderFileSlice", () => {
  it("renders the whole file when it fits", () => {
    const slice = renderFileSlice(A, 1000);
    expect(slice).toContain("--- a/src/a.ts");
    expect(slice).toContain("+++ b/src/a.ts");
    expect(slice).toContain("@@ -1 +1 @@");
    expect(slice).toContain("-const b = 2;");
    expect(slice).toContain("+const b = 3;");
    expect(slice).not.toContain("more changed lines");
  });

  it("cuts mid-file with a changed-line note (context excluded from the count)", () => {
    const big = bigFile("big.ts", 100); // ~4k chars of content
    const slice = renderFileSlice(big, 600);
    expect(slice.length).toBeLessThanOrEqual(700);
    expect(slice).toContain("--- a/big.ts");
    expect(slice).toMatch(/\[\+\d+ more changed lines in this file\]/);
    // Context lines never count as omitted.
    expect(slice).not.toMatch(/\[0 more changed lines/);
  });

  it("always includes the file headers even under a tiny budget", () => {
    const slice = renderFileSlice(A, 30);
    expect(slice.startsWith("--- a/src/a.ts\n+++ b/src/a.ts")).toBe(true);
  });

  it("uses old_path for renames", () => {
    const renamed = { ...A, path: "new.ts", old_path: "old.ts" };
    expect(renderFileSlice(renamed, 1000)).toContain("--- a/old.ts");
  });
});

// ---------------------------------------------------------------------------
// budgetedDiffText
// ---------------------------------------------------------------------------

describe("budgetedDiffText", () => {
  it("renders small inputs as stats + full diff, no budget noise", () => {
    const text = budgetedDiffText([A]);
    expect(text).toContain("Files changed (1):");
    expect(text).toContain("- src/a.ts (+1/-1)");
    expect(text).toContain("+++ b/src/a.ts");
    expect(text).toContain("+const b = 3;");
    // Nothing truncated or omitted → no meta notes, no section header.
    expect(text).not.toContain("more changed lines");
    expect(text).not.toContain("Content omitted");
    expect(text).not.toContain("Diff content (");
  });

  it("gives every file a slice when many files share the budget", () => {
    const files = [
      bigFile("one.ts", 200), // ~8k chars each — alone each would exceed a fair share
      bigFile("two.ts", 200),
      bigFile("three.ts", 200),
      bigFile("four.ts", 200),
      bigFile("five.ts", 200),
    ];
    const text = budgetedDiffText(files, { maxChars: 12_000, perFileCap: 6_000 });
    // Every file is represented...
    for (const p of ["one.ts", "two.ts", "three.ts", "four.ts", "five.ts"]) {
      expect(text).toContain(`+++ b/${p}`);
    }
    // ...each is trimmed to its fair share...
    expect(text.match(/more changed lines in this file\]/g)?.length).toBe(5);
    // ...nothing is fully omitted...
    expect(text).not.toContain("Content omitted");
    // ...and the total stays under the budget plus notes.
    expect(text.length).toBeLessThan(12_000 + 1_000);
  });

  it("caps a single huge file at perFileCap", () => {
    const text = budgetedDiffText([bigFile("huge.ts", 1_000)], { perFileCap: 2_000 });
    expect(text).toContain("[+");
    const diffPortion = text.slice(text.indexOf("+++ b/huge.ts"));
    expect(diffPortion.length).toBeLessThan(2_300);
  });

  it("excludes generated content but keeps its stat line", () => {
    const text = budgetedDiffText([A, bigFile("package-lock.json", 500)]);
    expect(text).toContain("- package-lock.json");
    expect(text).toContain("+++ b/src/a.ts");
    expect(text).not.toContain("+++ b/package-lock.json");
    expect(text).toContain("[Content omitted for 1 file (stats above): package-lock.json]");
  });

  it("lists omitted files by path, capped with an 'and K more' tail", () => {
    const files = [
      bigFile("src/real.ts", 10),
      ...Array.from({ length: 30 }, (_, i) => bigFile(`gen/dist/f${i}.js`, 10)),
    ];
    const text = budgetedDiffText(files, { maxChars: 3_000 });
    expect(text).toContain("[Content omitted for 30 files");
    // The note names the first 20 omitted paths, then collapses.
    const note = text.slice(text.indexOf("[Content omitted"));
    expect(note).toContain("gen/dist/f0.js");
    expect(note).toContain("gen/dist/f19.js");
    expect(note).not.toContain("gen/dist/f20.js");
    expect(note).toContain("and 10 more]");
  });

  it("omits later files when the budget runs out (they stay in the stats)", () => {
    const files = [bigFile("a.ts", 400), bigFile("b.ts", 400), bigFile("c.ts", 400)];
    const text = budgetedDiffText(files, { maxChars: 1_200, perFileCap: 6_000 });
    expect(text).toContain("+++ b/a.ts");
    expect(text).toContain("Content omitted");
    expect(text).toContain("- c.ts"); // stat line survives
  });

  it("skips binary files from content while listing them in the stats", () => {
    const text = budgetedDiffText([A, file("img.png", [], { binary: true })]);
    expect(text).toContain("- img.png (binary)");
    expect(text).not.toContain("+++ b/img.png");
    expect(text).not.toContain("Content omitted for 1"); // binaries aren't "omitted", they're just not content
  });

  it("respects the default ~128k-context budget (400k chars)", () => {
    // 200 files × ~13k chars of content ≈ 2.7MB — far past the default.
    const files = Array.from({ length: 200 }, (_, i) => bigFile(`f${i}.ts`, 300));
    const text = budgetedDiffText(files);
    expect(text.length).toBeLessThanOrEqual(MAX_DIFF_CHARS + 3_000);
    expect(text).toContain("Content omitted");
    // Every file is still named somewhere (stats or omission note).
    expect(text).toContain("- f0.ts");
    expect(text).toContain("f199.ts");
  });

  it("handles an empty file list without crashing", () => {
    expect(budgetedDiffText([])).toBe("Files changed (0):");
  });
});
