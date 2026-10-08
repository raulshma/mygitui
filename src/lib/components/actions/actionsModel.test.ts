/**
 * Unit tests for the pure actions model (run classification, line
 * truncation, run ordering, clean-preview grouping, confirm thresholds).
 */

import { describe, expect, it } from "vitest";
import {
  CLEAN_CONFIRM_THRESHOLD,
  classifyRun,
  cleanSelectionSummary,
  dirOf,
  groupByDir,
  groupLabel,
  needsTypeConfirm,
  outputDisplayLines,
  runStatusLabel,
  runsForRepo,
  sortRunsNewestFirst,
  truncateLines,
} from "./actionsModel";

describe("classifyRun", () => {
  it("is running until the done event, whatever the code", () => {
    expect(classifyRun(false, null)).toBe("running");
    expect(classifyRun(false, 0)).toBe("running");
    expect(classifyRun(false, 2)).toBe("running");
  });

  it("classifies exit 0 as success and anything else as failed", () => {
    expect(classifyRun(true, 0)).toBe("success");
    expect(classifyRun(true, 1)).toBe("failed");
    expect(classifyRun(true, -1)).toBe("failed");
    // Done without a code (killed): non-zero semantics.
    expect(classifyRun(true, null)).toBe("failed");
  });

  it("labels running vs exit codes", () => {
    expect(runStatusLabel("running", null)).toBe("running");
    expect(runStatusLabel("success", 0)).toBe("exit 0");
    expect(runStatusLabel("failed", 3)).toBe("exit 3");
    expect(runStatusLabel("failed", null)).toBe("exit ?");
  });
});

describe("truncateLines", () => {
  it("keeps everything under the cap", () => {
    const result = truncateLines(["a", "b", "c"], 5);
    expect(result).toEqual({ lines: ["a", "b", "c"], dropped: 0 });
  });

  it("keeps the most recent lines past the cap and counts the drop", () => {
    const lines = Array.from({ length: 12 }, (_, i) => `line${i}`);
    const result = truncateLines(lines, 10);
    expect(result.lines).toHaveLength(10);
    expect(result.lines[0]).toBe("line2");
    expect(result.lines[9]).toBe("line11");
    expect(result.dropped).toBe(2);
  });

  it("handles the exact cap and empty input", () => {
    expect(truncateLines(["a"], 1)).toEqual({ lines: ["a"], dropped: 0 });
    expect(truncateLines([], 10)).toEqual({ lines: [], dropped: 0 });
  });

  it("does not mutate the input", () => {
    const lines = ["a", "b"];
    truncateLines(lines, 1);
    expect(lines).toEqual(["a", "b"]);
  });

  it("renders the truncation notice ahead of the kept lines", () => {
    const display = outputDisplayLines(["x", "y"], 7, 5000);
    expect(display).toEqual(["… 7 earlier lines truncated (kept 5000)", "x", "y"]);
    expect(outputDisplayLines(["x"], 0, 5000)).toEqual(["x"]);
  });
});

describe("run ordering", () => {
  it("sorts newest (highest order) first", () => {
    const runs = [
      { order: 1, name: "old" },
      { order: 3, name: "newest" },
      { order: 2, name: "middle" },
    ];
    expect(sortRunsNewestFirst(runs).map((r) => r.name)).toEqual([
      "newest",
      "middle",
      "old",
    ]);
  });

  it("filters by repo and sorts within it", () => {
    const runs = [
      { repo_id: "a", order: 1, name: "a1" },
      { repo_id: "b", order: 2, name: "b2" },
      { repo_id: "a", order: 3, name: "a3" },
    ];
    expect(runsForRepo(runs, "a").map((r) => r.name)).toEqual(["a3", "a1"]);
    expect(runsForRepo(runs, "c")).toEqual([]);
  });
});

describe("clean preview grouping", () => {
  it("splits paths into immediate-directory groups, root first", () => {
    const groups = groupByDir([
      { path: "root.txt", change: "deleted" },
      { path: "src/a.ts", change: "deleted" },
      { path: "loose.log", change: "deleted" },
      { path: "src/deep/b.ts", change: "deleted" },
      { path: "dist/z.js", change: "deleted" },
    ]);
    expect(groups.map((g) => g.dir)).toEqual(["", "dist", "src", "src/deep"]);
    expect(groups[0].files.map((f) => f.path)).toEqual(["root.txt", "loose.log"]);
    expect(groups[2].files.map((f) => f.path)).toEqual(["src/a.ts"]);
    expect(groups[3].files.map((f) => f.path)).toEqual(["src/deep/b.ts"]);
  });

  it("labels the root group and dir groups distinctly", () => {
    const [root, dir] = groupByDir([
      { path: "a.txt", change: "deleted" },
      { path: "d/b.txt", change: "deleted" },
    ]);
    expect(groupLabel(root)).toBe("(root)");
    expect(groupLabel(dir)).toBe("d/");
  });

  it("computes directory prefixes for both separators", () => {
    expect(dirOf("file.txt")).toBe("");
    expect(dirOf("a/b/c.txt")).toBe("a/b");
    expect(dirOf("a\\b\\c.txt")).toBe("a\\b");
  });

  it("handles empty file lists", () => {
    expect(groupByDir([])).toEqual([]);
  });
});

describe("clean confirmation", () => {
  it("asks for type-to-confirm only past the threshold", () => {
    expect(needsTypeConfirm(CLEAN_CONFIRM_THRESHOLD)).toBe(false);
    expect(needsTypeConfirm(CLEAN_CONFIRM_THRESHOLD + 1)).toBe(true);
    expect(needsTypeConfirm(0)).toBe(false);
  });

  it("summarizes the selection with correct pluralization", () => {
    expect(cleanSelectionSummary(3, 9)).toBe("3 of 9 paths selected");
    expect(cleanSelectionSummary(1, 1)).toBe("1 of 1 path selected");
    expect(cleanSelectionSummary(0, 0)).toBe("0 of 0 paths selected");
  });
});
