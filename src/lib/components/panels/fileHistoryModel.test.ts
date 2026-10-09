/**
 * Unit tests for the file-history selection model (panels/fileHistoryModel.ts):
 * shift-click ranges and the aggregate-diff endpoints.
 */

import { describe, expect, it } from "vitest";
import {
  aggregateRange,
  rangeIndices,
} from "$lib/components/panels/fileHistoryModel";

const commit = (sha: string, parents: string[]) => ({ sha, parents });

/** Newest-first walk like the file history list: index 0 = newest. */
const walk = [
  commit("c3", ["c2"]),
  commit("c2", ["c1"]),
  commit("c1", ["c0"]),
  commit("c0", []), // root
];

describe("rangeIndices", () => {
  it("builds an inclusive forward range from the anchor down", () => {
    expect(rangeIndices(1, 3)).toEqual([1, 2, 3]);
  });

  it("normalizes a reversed (upward) shift-click", () => {
    expect(rangeIndices(3, 1)).toEqual([1, 2, 3]);
  });

  it("degrades to a single index when anchor equals the click", () => {
    expect(rangeIndices(2, 2)).toEqual([2]);
  });
});

describe("aggregateRange", () => {
  it("diffs the oldest's first parent against the newest", () => {
    // base = oldest's parent (c0); the label is git notation for it.
    expect(aggregateRange(walk, 0, 2)).toEqual({
      baseSha: "c0",
      targetSha: "c3",
      baseLabel: "c1^",
      targetLabel: "c3",
    });
  });

  it("labels the base with the oldest's short sha plus ^", () => {
    const range = aggregateRange(
      [commit("1234567890abcdef", ["fedcba0987654321"])],
      0,
      0,
    );
    expect(range).toMatchObject({
      baseLabel: "1234567^",
      targetLabel: "1234567",
    });
  });

  it("degrades a single-commit range to its own parent0 diff", () => {
    expect(aggregateRange(walk, 1, 1)).toEqual({
      baseSha: "c1",
      targetSha: "c2",
      baseLabel: "c2^",
      targetLabel: "c2",
    });
  });

  it("rejects a range whose oldest commit is a root commit", () => {
    expect(aggregateRange(walk, 0, 3)).toEqual({
      error: "Oldest selected commit is a root commit — no diff base",
    });
  });

  it("rejects out-of-range indices", () => {
    expect(aggregateRange(walk, 0, 9)).toEqual({
      error: "Selection is out of range",
    });
  });
});
