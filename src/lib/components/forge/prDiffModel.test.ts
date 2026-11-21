/**
 * Unit tests for the PR-diff shaping (prDiffModel.ts, E2 §9.4): GitHub REST
 * unified patches (`forge_pr_files`) parse into the `FileDiff[]` the shared
 * DiffViewer renders — hunk headers with and without counts, line numbering,
 * no-newline markers, and patch-less (binary/oversize) files.
 */

import { describe, expect, it } from "vitest";
import type { ForgePrFile } from "$lib/ipc/types";
import {
  parseHunkHeader,
  parsePrPatch,
  prFilesToFileDiffs,
} from "./prDiffModel";

function prFile(patch: Partial<ForgePrFile> & { path: string }): ForgePrFile {
  return {
    status: "modified",
    additions: 0,
    deletions: 0,
    patch: null,
    ...patch,
  };
}

describe("parseHunkHeader", () => {
  it("parses full and abbreviated headers", () => {
    expect(parseHunkHeader("@@ -1,5 +1,6 @@")).toEqual({
      oldStart: 1,
      oldCount: 5,
      newStart: 1,
      newCount: 6,
    });
    expect(parseHunkHeader("@@ -3 +3 @@")).toEqual({
      oldStart: 3,
      oldCount: 1,
      newStart: 3,
      newCount: 1,
    });
    // The section heading after the closing @@ must not break the parse.
    expect(parseHunkHeader("@@ -0,0 +1,4 @@ fn main()")).toEqual({
      oldStart: 0,
      oldCount: 0,
      newStart: 1,
      newCount: 4,
    });
  });

  it("rejects non-header lines", () => {
    expect(parseHunkHeader("context")).toBeNull();
    expect(parseHunkHeader("--- a/file")).toBeNull();
    expect(parseHunkHeader("@@ -1,x +1,2 @@")).toBeNull();
  });
});

describe("parsePrPatch", () => {
  it("maps signs to origins and tracks line numbers", () => {
    const hunks = parsePrPatch(
      [
        "@@ -1,3 +1,4 @@",
        " context",
        "-removed one",
        "+added one",
        "+added two",
        " more context",
      ].join("\n"),
    );
    expect(hunks).toHaveLength(1);
    const lines = hunks[0]!.lines;
    expect(hunks[0]!.old_start).toBe(1);
    expect(hunks[0]!.new_start).toBe(1);
    expect(lines.map((line) => line.origin)).toEqual([
      " ",
      "-",
      "+",
      "+",
      " ",
    ]);
    expect(lines.map((line) => line.old_no)).toEqual([1, 2, null, null, 3]);
    expect(lines.map((line) => line.new_no)).toEqual([1, null, 2, 3, 4]);
  });

  it("ignores pre-hunk noise (rename headers, file headers)", () => {
    const hunks = parsePrPatch(
      ["rename from a.txt", "rename to b.txt", "@@ -1 +1 @@", "-old", "+new"].join(
        "\n",
      ),
    );
    expect(hunks).toHaveLength(1);
    expect(hunks[0]!.lines.map((line) => line.text)).toEqual(["old", "new"]);
  });

  it("carries no-newline markers as metadata and keeps numbers", () => {
    const hunks = parsePrPatch(
      ["@@ -1,2 +1,2 @@", "-tail", "\\ No newline at end of file", "+tail"].join("\n"),
    );
    expect(hunks[0]!.lines).toHaveLength(3);
    const marker = hunks[0]!.lines[1]!;
    expect(marker.origin).toBe("=");
    expect(marker.text).toBe("No newline at end of file");
    // The marker consumed no line numbers: the added line stays new line 1.
    expect(hunks[0]!.lines[2]!.new_no).toBe(1);
    expect(hunks[0]!.lines[2]!.old_no).toBeNull();
  });

  it("splits consecutive hunks", () => {
    const hunks = parsePrPatch(
      ["@@ -1 +1 @@", "-a", "+b", "@@ -10 +10 @@", "+c"].join("\n"),
    );
    expect(hunks).toHaveLength(2);
    expect(hunks[1]!.old_start).toBe(10);
    expect(hunks[1]!.lines[0]!.new_no).toBe(10);
  });

  it("returns nothing for patch text without hunks", () => {
    expect(parsePrPatch("")).toEqual([]);
    expect(parsePrPatch("not a patch")).toEqual([]);
  });
});

describe("prFilesToFileDiffs", () => {
  it("converts patched files with counts and hunks", () => {
    const files = [
      prFile({
        path: "src/a.ts",
        status: "modified",
        additions: 1,
        deletions: 1,
        patch: "@@ -1 +1 @@\n-old\n+new",
      }),
    ];
    const diffs = prFilesToFileDiffs(files);
    expect(diffs).toHaveLength(1);
    expect(diffs[0]).toMatchObject({
      path: "src/a.ts",
      old_path: null,
      binary: false,
      is_image: false,
      additions: 1,
      deletions: 1,
    });
    expect(diffs[0]!.hunks).toHaveLength(1);
    expect(diffs[0]!.hunks[0]!.lines.map((line) => line.origin)).toEqual([
      "-",
      "+",
    ]);
  });

  it("marks patch-less files binary with no hunks", () => {
    const diffs = prFilesToFileDiffs([
      prFile({ path: "asset.png", status: "added", additions: 0, deletions: 0 }),
    ]);
    expect(diffs[0]!.binary).toBe(true);
    expect(diffs[0]!.hunks).toEqual([]);
  });
});
