/**
 * Unit tests for the history lane's pure logic: `LogFilter` serialization,
 * the incremental `LogIndex` flatten cache, and the restored-selection
 * resolution. Formatting-helper tests live in `$lib/git/format.test.ts`.
 */

import { describe, expect, it } from "vitest";
import {
  EMPTY_FILTER,
  LogIndex,
  resolveRestoredSelection,
  toLogFilter,
} from "$lib/stores/history-logic";
import type { CommitInfo, GraphRow, LogPage } from "$lib/ipc/types";

// ---------------------------------------------------------------------------
// Filter serialization
// ---------------------------------------------------------------------------

describe("toLogFilter", () => {
  it("serializes empty fields into the exact backend shape", () => {
    expect(toLogFilter(EMPTY_FILTER)).toEqual({
      text: null,
      regex: false,
      author: null,
      path: null,
      after_unix: null,
      before_unix: null,
      refs: [],
      follow: false,
      pickaxe: null,
    });
  });

  it("trims free-text fields and converts valid days to unix edges", () => {
    expect(
      toLogFilter({
        text: "  fix leak  ",
        regex: false,
        pickaxe: "",
        author: " alice ",
        path: "src/lib/",
        after: "2026-01-02",
        before: "2026-03-31",
      }),
    ).toEqual({
      text: "fix leak",
      regex: false,
      author: "alice",
      path: "src/lib/",
      after_unix: 1_767_312_000,
      before_unix: 1_775_001_599,
      refs: [],
      follow: false,
      pickaxe: null,
    });
  });

  it("drops invalid days to null instead of NaN", () => {
    const filter = toLogFilter({ ...EMPTY_FILTER, after: "nope", before: "2026-02-30" });
    expect(filter.after_unix).toBeNull();
    expect(filter.before_unix).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// LogIndex (flatten cache)
// ---------------------------------------------------------------------------

function sha(i: number): string {
  return (i + 1).toString(16).padStart(40, "0");
}

function commit(i: number, refs: string[] = []): CommitInfo {
  return {
    sha: sha(i),
    parents: i > 0 ? [sha(i - 1)] : [],
    author: { name: `Author ${i}`, email: `a${i}@x.com`, time: 1_700_000 + i, offset_minutes: 0 },
    committer: { name: `Author ${i}`, email: `a${i}@x.com`, time: 1_700_000 + i, offset_minutes: 0 },
    message: `commit ${i}\n\nbody ${i}\n`,
    summary: `commit ${i}`,
    refs,
  };
}

function row(c: CommitInfo): GraphRow {
  return { sha: c.sha, lane: 0, edges: [], lane_count: 1 };
}

function makePage(from: number, count: number, generation = 0): LogPage {
  const commits = Array.from({ length: count }, (_, k) => commit(from + k));
  return { commits, rows: commits.map(row), next_cursor: null, generation };
}

describe("LogIndex", () => {
  it("appends pages keeping commits and rows index-aligned", () => {
    const index = new LogIndex();
    expect(index.append(makePage(0, 3))).toBe(3);
    expect(index.append(makePage(3, 2))).toBe(2);
    expect(index.size).toBe(5);
    expect(index.commits.map((c) => c.summary)).toEqual([
      "commit 0",
      "commit 1",
      "commit 2",
      "commit 3",
      "commit 4",
    ]);
    for (let i = 0; i < index.size; i++) {
      expect(index.rows[i]?.sha).toBe(index.commits[i]?.sha);
    }
    expect(index.indexOf(sha(4))).toBe(4);
    expect(index.indexOf(sha(999))).toBe(-1);
    expect(index.has(sha(0))).toBe(true);
  });

  it("dedupes boundary-overlap commits by sha", () => {
    const index = new LogIndex();
    index.append(makePage(0, 3)); // shas 0,1,2
    const added = index.append(makePage(2, 3)); // 2,3,4 — sha 2 overlaps
    expect(added).toBe(2);
    expect(index.size).toBe(5);
    expect(index.commits.filter((c) => c.sha === sha(2))).toHaveLength(1);
    expect(index.rows).toHaveLength(5);
  });

  it("falls back to sha matching when rows are missing (length mismatch)", () => {
    const index = new LogIndex();
    const page = makePage(0, 3);
    page.rows = [page.rows[0]!, page.rows[2]!]; // row for sha(1) missing
    index.append(page);
    expect(index.rows.map((r) => r.sha)).toEqual([sha(0), sha(1), sha(2)]);
    expect(index.rows[1]).toEqual({ sha: sha(1), lane: 0, edges: [], lane_count: 1 });
  });

  it("synthesizes a fallback row for a missing graph row", () => {
    const index = new LogIndex();
    const page = makePage(0, 2);
    page.rows = [page.rows[0]!];
    index.append(page);
    expect(index.rows[1]).toEqual({ sha: sha(1), lane: 0, edges: [], lane_count: 1 });
  });

  it("appends incrementally: array identity is stable, never reflattened", () => {
    const index = new LogIndex();
    index.append(makePage(0, 3));
    const commitsRef = index.commits;
    const rowsRef = index.rows;
    index.append(makePage(3, 2));
    expect(index.commits).toBe(commitsRef);
    expect(index.rows).toBe(rowsRef);
    expect(commitsRef).toHaveLength(5);
  });

  it("resets while keeping array identity", () => {
    const index = new LogIndex();
    index.append(makePage(0, 3));
    const ref = index.commits;
    index.reset();
    expect(index.size).toBe(0);
    expect(index.commits).toBe(ref);
    expect(index.indexOf(sha(0))).toBe(-1);
    index.append(makePage(0, 1));
    expect(index.size).toBe(1);
  });

  it("flattens 50k commits (100 pages × 500) in under 5ms", () => {
    // JSON round-trip: distinct string objects for commit/row shas, like the
    // IPC layer would produce (forces real content compares, no pointer fast
    // path between a commit's sha and its row's sha).
    const wire = JSON.parse(
      JSON.stringify(
        Array.from({ length: 100 }, (_, p) => makePage(p * 500, 500)),
      ),
    ) as LogPage[];
    // Best-of-5: the first pass warms up; later passes dodge CPU contention
    // from vitest workers running sibling suites in parallel.
    let best = Number.POSITIVE_INFINITY;
    for (let run = 0; run < 5; run++) {
      const index = new LogIndex();
      const t0 = performance.now();
      for (const page of wire) index.append(page);
      best = Math.min(best, performance.now() - t0);
      expect(index.size).toBe(50_000);
      expect(index.indexOf(sha(49_999))).toBe(49_999); // lazy index materializes
    }
    expect(best).toBeLessThan(5);
  });
});

// ---------------------------------------------------------------------------
// Restored-selection resolution
// ---------------------------------------------------------------------------

describe("resolveRestoredSelection", () => {
  it("resolves with the row index once the commit reaches the loaded log", () => {
    const index = new LogIndex();
    index.append(makePage(0, 3));
    expect(resolveRestoredSelection(index, sha(2), false)).toEqual({
      kind: "resolved",
      index: 2,
    });
    // Exhaustion is irrelevant once resolved.
    expect(resolveRestoredSelection(index, sha(2), true)).toEqual({
      kind: "resolved",
      index: 2,
    });
  });

  it("stays pending while the stream may still deliver the commit", () => {
    const index = new LogIndex();
    index.append(makePage(0, 3));
    expect(resolveRestoredSelection(index, sha(50), false)).toEqual({
      kind: "pending",
    });
  });

  it("reports pending for an empty log that is still streaming", () => {
    const index = new LogIndex();
    expect(resolveRestoredSelection(index, sha(0), false)).toEqual({
      kind: "pending",
    });
  });

  it("drops a commit the exhausted stream never delivered", () => {
    const index = new LogIndex();
    index.append(makePage(0, 3));
    expect(resolveRestoredSelection(index, sha(50), true)).toEqual({
      kind: "dropped",
    });
  });
});
