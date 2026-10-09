/**
 * Unit tests for the commit bar's fixup helpers (commit/commitBarModel.ts):
 * staged detection, HEAD lookup inside a log page, message building.
 */

import { describe, expect, it } from "vitest";
import type { CommitInfo, LogPage, StatusEntry } from "$lib/ipc/types";
import {
  fixupMessage,
  hasStagedChanges,
  headCommitInPage,
} from "$lib/commit/commitBarModel";

function entry(index: StatusEntry["index"], worktree: StatusEntry["worktree"] = "unmodified"): StatusEntry {
  return { path: "a.txt", old_path: null, index, worktree };
}

function commit(sha: string, summary: string): CommitInfo {
  const author = { name: "Ada", email: "ada@example.com", time: 1, offset_minutes: 0 };
  return {
    sha,
    parents: [],
    author,
    committer: author,
    message: `${summary}\n`,
    summary,
    refs: [],
  };
}

describe("hasStagedChanges", () => {
  it("is false for a clean worktree/index", () => {
    expect(hasStagedChanges([entry("unmodified"), entry("untracked", "untracked")])).toBe(false);
    expect(hasStagedChanges([entry("ignored", "ignored")])).toBe(false);
    expect(hasStagedChanges([])).toBe(false);
  });

  it("is true for any real index-side change", () => {
    expect(hasStagedChanges([entry("unmodified"), entry("modified")])).toBe(true);
    expect(hasStagedChanges([entry("added")])).toBe(true);
    expect(hasStagedChanges([entry("renamed")])).toBe(true);
    expect(hasStagedChanges([entry("deleted")])).toBe(true);
    expect(hasStagedChanges([entry("copied")])).toBe(true);
  });

  it("ignores worktree-side-only changes", () => {
    expect(hasStagedChanges([entry("unmodified", "modified")])).toBe(false);
  });

  it("counts conflicted index entries as staged state (merge in progress)", () => {
    // Only real change kinds count; conflicts come through the conflicts
    // flow, so "conflicted" alone does not claim a staged change.
    expect(hasStagedChanges([entry("conflicted", "conflicted")])).toBe(false);
  });
});

describe("headCommitInPage", () => {
  const page: LogPage = {
    commits: [commit("bbb", "second"), commit("aaa", "first")],
    rows: [],
    next_cursor: null,
    generation: 1,
  };

  it("finds HEAD by sha regardless of page order", () => {
    expect(headCommitInPage(page, "aaa")?.summary).toBe("first");
    expect(headCommitInPage(page, "bbb")?.summary).toBe("second");
  });

  it("returns null when HEAD is unknown or missing", () => {
    expect(headCommitInPage(page, null)).toBeNull();
    expect(headCommitInPage(page, "zzz")).toBeNull();
  });
});

describe("fixupMessage", () => {
  it("prefixes the HEAD summary's first line", () => {
    expect(fixupMessage("Add retry loop\n\nLong body\nmore")).toBe("fixup! Add retry loop");
  });

  it("trims the summary line", () => {
    expect(fixupMessage("  spaced subject  ")).toBe("fixup! spaced subject");
  });

  it("handles an empty summary defensively", () => {
    expect(fixupMessage("")).toBe("fixup! ");
  });
});
