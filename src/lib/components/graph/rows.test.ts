/**
 * Unit tests for the incremental row-flattening cache (rows.ts).
 * The 60fps gate depends on `update()` never re-flattening on plain appends.
 */

import { describe, expect, it } from "vitest";
import type { CommitInfo, LogPage } from "$lib/ipc/types";
import { RowCache } from "$lib/components/graph/rows";

function commit(sha: string): CommitInfo {
  return {
    sha,
    parents: [],
    author: { name: "Ada", email: "ada@example.com", time: 1, offset_minutes: 0 },
    committer: { name: "Ada", email: "ada@example.com", time: 1, offset_minutes: 0 },
    message: `${sha}\n`,
    summary: `commit ${sha}`,
    refs: [],
  };
}

function page(shas: string[], generation = 1): LogPage {
  return {
    commits: shas.map(commit),
    rows: shas.map((sha, i) => ({ sha, lane: i % 3, edges: [], lane_count: 3 })),
    next_cursor: null,
    generation,
  };
}

describe("RowCache", () => {
  it("starts empty", () => {
    const cache = new RowCache();
    expect(cache.length).toBe(0);
    expect(cache.rows).toEqual([]);
    expect(cache.update([])).toBe(false);
    expect(cache.stats.rebuilds).toBe(0);
  });

  it("flattens pages in order, rows and commits index-aligned", () => {
    const p1 = page(["a", "b"]);
    const p2 = page(["c", "d"]);
    const cache = new RowCache();
    cache.update([p1, p2]);

    expect(cache.length).toBe(4);
    expect(cache.rows.map((r) => r.sha)).toEqual(["a", "b", "c", "d"]);
    expect(cache.commitAt(2)?.summary).toBe("commit c");
    // First consumption goes through the append path (no prior prefix).
    expect(cache.stats.rebuilds).toBe(0);
    expect(cache.stats.appendedPages).toBe(2);
  });

  it("appends incrementally when the array grows with a shared prefix", () => {
    const p1 = page(["a", "b"]);
    const p2 = page(["c", "d"]);
    const p3 = page(["e"]);
    const cache = new RowCache();
    cache.update([p1, p2]);

    const rowsBefore = cache.rows.slice();
    const changed = cache.update([p1, p2, p3]);

    expect(changed).toBe(true);
    expect(cache.length).toBe(5);
    // No rebuild, and existing row objects keep their identity.
    expect(cache.stats.rebuilds).toBe(0);
    expect(cache.stats.appendedPages).toBe(3);
    expect(cache.rows.slice(0, 4)).toEqual(rowsBefore);
    expect(cache.rows[0]).toBe(p1.rows[0]);
    expect(cache.rows[4]?.sha).toBe("e");
  });

  it("is a no-op when handed the same array identity", () => {
    const pages = [page(["a"])];
    const cache = new RowCache();
    cache.update(pages);
    const stats = { ...cache.stats };

    expect(cache.update(pages)).toBe(false);
    expect(cache.stats).toEqual(stats);
  });

  it("fully rebuilds when the prefix is replaced (generation reset)", () => {
    const p1 = page(["a", "b"]);
    const p2 = page(["c"]);
    const cache = new RowCache();
    cache.update([p1, p2]);

    const fresh = page(["x", "y", "z"], 2);
    const changed = cache.update([fresh]);

    expect(changed).toBe(true);
    expect(cache.length).toBe(3);
    expect(cache.rows.map((r) => r.sha)).toEqual(["x", "y", "z"]);
    expect(cache.stats.rebuilds).toBe(1);
  });

  it("fully rebuilds when the page list shrinks", () => {
    const p1 = page(["a", "b"]);
    const p2 = page(["c"]);
    const cache = new RowCache();
    cache.update([p1, p2]);

    cache.update([p1]);

    expect(cache.length).toBe(2);
    expect(cache.rows.map((r) => r.sha)).toEqual(["a", "b"]);
    expect(cache.stats.rebuilds).toBe(1);
  });

  it("truncates misaligned pages to the shorter of rows/commits", () => {
    const short = page(["a", "b"]);
    const long = { ...short, rows: [...short.rows, { sha: "c", lane: 0, edges: [], lane_count: 1 }] };
    const cache = new RowCache();
    cache.update([long]);
    expect(cache.length).toBe(2);
    expect(cache.commitAt(2)).toBeNull();
  });

  it("indexOfSha resolves rows, extends lazily, and invalidates on rebuild", () => {
    const p1 = page(["a", "b"]);
    const p2 = page(["c"]);
    const cache = new RowCache();
    cache.update([p1]);

    expect(cache.indexOfSha("b")).toBe(1);
    expect(cache.indexOfSha("nope")).toBe(-1);
    expect(cache.indexOfSha("")).toBe(-1);

    cache.update([p1, p2]); // index map extends incrementally, old entries stay
    expect(cache.indexOfSha("c")).toBe(2);
    expect(cache.indexOfSha("a")).toBe(0);

    cache.update([page(["z"])]); // rebuild drops stale shas
    expect(cache.indexOfSha("a")).toBe(-1);
    expect(cache.indexOfSha("z")).toBe(0);
  });

  it("handles 250k rows across 500 pages incrementally (append-only, 1 rebuild max)", () => {
    const PAGE = 500;
    const TOTAL = 250_000;
    const pages: LogPage[] = [];
    const cache = new RowCache();
    for (let p = 0; p < TOTAL / PAGE; p += 1) {
      const base = p * PAGE;
      pages.push(
        page(Array.from({ length: PAGE }, (_, i) => `sha-${base + i}`)),
      );
      cache.update(pages.slice()); // parent reassigns a new array per stream tick
    }
    expect(cache.length).toBe(TOTAL);
    expect(cache.stats.rebuilds).toBe(0);
    expect(cache.rows[123_456]?.sha).toBe("sha-123456");
    expect(cache.commitAt(123_456)?.sha).toBe("sha-123456");
  });
});
