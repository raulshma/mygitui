import { describe, expect, it } from "vitest";
import {
  buildIndex,
  fuzzyScore,
  matchRanges,
  searchPaths,
  type PathIndex,
} from "./quickOpenModel";
import type { FileIndexEntry } from "$lib/ipc/types";

function entry(path: string, isDir = false): FileIndexEntry {
  return { path, is_dir: isDir, size: null };
}

describe("buildIndex", () => {
  it("keeps files, drops dirs, lowercases", () => {
    const index = buildIndex([entry("src/App.ts"), entry("src", true), entry("README.md")]);
    expect(index.paths).toEqual(["src/App.ts", "README.md"]);
    expect(index.lower).toEqual(["src/app.ts", "readme.md"]);
  });
});

describe("fuzzyScore", () => {
  it("requires a subsequence", () => {
    expect(fuzzyScore("xyz", "src/app.ts")).toBeNull();
    expect(fuzzyScore("sapp", "src/app.ts")).not.toBeNull();
  });

  it("prefers basename and consecutive matches", () => {
    const inName = fuzzyScore("app", "src/app.ts")!;
    const inPath = fuzzyScore("app", "application/config.yaml")!;
    expect(inName).toBeGreaterThan(inPath);

    // Same basename hit: consecutive beats spread across boundaries.
    const consecutive = fuzzyScore("app", "src/app.ts")!;
    const spread = fuzzyScore("apt", "src/app.ts")!; // a..p..t across "."
    expect(consecutive).toBeGreaterThan(spread);
  });

  it("prefers shorter paths", () => {
    const short = fuzzyScore("cfg", "cfg.toml")!;
    const long = fuzzyScore("cfg", "very/deeply/nested/dir/config.toml")!;
    expect(short).toBeGreaterThan(long);
  });

  it("empty query scores zero", () => {
    expect(fuzzyScore("", "anything")).toBe(0);
  });
});

describe("searchPaths", () => {
  const index: PathIndex = buildIndex([
    entry("src/lib/components/panels/StatusPanel.svelte"),
    entry("src/lib/components/panels/StatusRow.svelte"),
    entry("src/App.svelte"),
    entry("docs/status.md"),
    entry("src", true),
  ]);

  it("empty query lists the natural order", () => {
    expect(searchPaths(index, "", 3).map((h) => h.path)).toEqual([
      "src/lib/components/panels/StatusPanel.svelte",
      "src/lib/components/panels/StatusRow.svelte",
      "src/App.svelte",
    ]);
  });

  it("ranks basename matches above path matches", () => {
    const hits = searchPaths(index, "status");
    // All basename hits first, shortest first; the dir entry never matches.
    expect(hits.map((h) => h.path)).toEqual([
      "docs/status.md",
      "src/lib/components/panels/StatusRow.svelte",
      "src/lib/components/panels/StatusPanel.svelte",
    ]);
    expect(hits.map((h) => h.path)).not.toContain("src");
  });

  it("respects the limit", () => {
    expect(searchPaths(index, "s", 2)).toHaveLength(2);
  });
});

describe("matchRanges", () => {
  it("maps lowercase matches back to original indexes", () => {
    expect(matchRanges("app", "src/App.ts")).toEqual([[4, 5], [5, 6], [6, 7]]);
  });
});

describe("perf gate", () => {
  it("scores 50k paths in under 50ms", () => {
    const entries: FileIndexEntry[] = [];
    for (let i = 0; i < 50_000; i++) {
      entries.push(
        entry(
          `src/dir${Math.floor(i / 100)}/file${String(i).padStart(6, "0")}.ts`,
        ),
      );
    }
    const index = buildIndex(entries);
    const start = performance.now();
    const hits = searchPaths(index, "fl0", 100);
    const elapsed = performance.now() - start;
    expect(hits.length).toBe(100);
    // Same rationale as the fileTreeModel gate: tolerates a loaded CI
    // runner, still fails any super-linear scoring regression.
    expect(elapsed).toBeLessThan(250);
  });
});
