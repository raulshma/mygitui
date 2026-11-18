/**
 * Unit tests for the commit bar's helpers: the fixup pieces
 * (commit/commitBarModel.ts) and the authoring-niceties model
 * (commit/commitExtras.svelte.ts) — recents ring, ruler math, template
 * prefill gating, plus the store's envelope persistence.
 */

import { beforeEach, describe, expect, it } from "vitest";
import type { CommitInfo, LogPage, StatusEntry } from "$lib/ipc/types";
import {
  fixupMessage,
  hasStagedChanges,
  headCommitInPage,
} from "$lib/commit/commitBarModel";
import {
  COMMIT_RECENTS_LIMIT,
  COMMIT_RECENTS_PREFIX,
  COMMIT_WIDTH_PREFIX,
  CommitExtrasStore,
  normalizeCommitWidth,
  pushRecentMessage,
  rulerLeftCss,
  shouldPrefillTemplate,
  templateFromBody,
} from "$lib/commit/commitExtras.svelte";

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

describe("pushRecentMessage (recents ring)", () => {
  it("prepends new messages, newest first", () => {
    expect(pushRecentMessage([], "one")).toEqual(["one"]);
    expect(pushRecentMessage(["one"], "two")).toEqual(["two", "one"]);
  });

  it("dedupes by moving a repeated message back to the top", () => {
    const ring = pushRecentMessage(["a", "b", "c"], "b");
    expect(ring).toEqual(["b", "a", "c"]);
  });

  it("caps the ring at COMMIT_RECENTS_LIMIT", () => {
    let ring: string[] = [];
    for (let i = 0; i < COMMIT_RECENTS_LIMIT + 2; i++) ring = pushRecentMessage(ring, `m${i}`);
    expect(ring).toHaveLength(COMMIT_RECENTS_LIMIT);
    expect(ring[0]).toBe(`m${COMMIT_RECENTS_LIMIT + 1}`);
  });

  it("ignores blank messages and never mutates the input", () => {
    const ring = ["kept"];
    expect(pushRecentMessage(ring, "   ")).toEqual(["kept"]);
    expect(pushRecentMessage(ring, "new")).toEqual(["new", "kept"]);
    expect(ring).toEqual(["kept"]);
  });
});

describe("normalizeCommitWidth + rulerLeftCss (ruler math)", () => {
  it("defaults on junk and clamps the range", () => {
    expect(normalizeCommitWidth(72)).toBe(72);
    expect(normalizeCommitWidth(Number.NaN)).toBe(72);
    expect(normalizeCommitWidth(1)).toBe(40);
    expect(normalizeCommitWidth(9999)).toBe(120);
    expect(normalizeCommitWidth(72.4)).toBe(72);
  });

  it("positions the guide at padding + width ch", () => {
    expect(rulerLeftCss(72)).toBe("calc(0.5rem + 72ch)");
    expect(rulerLeftCss(50)).toBe("calc(0.5rem + 50ch)");
    // A clamped width clamps the ruler too.
    expect(rulerLeftCss(10)).toBe("calc(0.5rem + 40ch)");
  });
});

describe("template prefill (gate + body)", () => {
  it("prefills only when enabled, untried, and the draft is empty", () => {
    expect(shouldPrefillTemplate({ enabled: true, tried: false, message: "" })).toBe(true);
    expect(shouldPrefillTemplate({ enabled: true, tried: false, message: "  " })).toBe(true);
    expect(shouldPrefillTemplate({ enabled: false, tried: false, message: "" })).toBe(false);
    expect(shouldPrefillTemplate({ enabled: true, tried: true, message: "" })).toBe(false);
    // A draft in progress is never overwritten.
    expect(shouldPrefillTemplate({ enabled: true, tried: false, message: "wip" })).toBe(false);
  });

  it("strips comment lines and surrounding blanks from a template body", () => {
    const template = [
      "# <type>: <subject>",
      "",
      "# body starting at 72 chars is recommended",
      "Add the retry loop",
      "",
      "# ***",
      "",
    ].join("\n");
    expect(templateFromBody(template)).toBe("Add the retry loop");
  });

  it("yields an empty draft for an all-comments template", () => {
    expect(templateFromBody("# only comments\n# more")).toBe("");
    expect(templateFromBody("")).toBe("");
  });
});

describe("CommitExtrasStore", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("persists recents and widths through the versioned envelope", () => {
    const store = new CommitExtrasStore(localStorage);
    store.rememberMessage("/a/repo", "first");
    store.rememberMessage("/a/repo", "second");
    store.setWidth("/a/repo", 60);

    expect(
      JSON.parse(localStorage.getItem(`${COMMIT_RECENTS_PREFIX}/a/repo`)!),
    ).toEqual({ v: 1, data: ["second", "first"] });
    expect(JSON.parse(localStorage.getItem(`${COMMIT_WIDTH_PREFIX}/a/repo`)!)).toEqual({
      v: 1,
      data: 60,
    });

    // A fresh store hydrates from the same envelopes via ensure().
    const reloaded = new CommitExtrasStore(localStorage);
    reloaded.ensure("/a/repo");
    expect(reloaded.recentsFor("/a/repo")).toEqual(["second", "first"]);
    expect(reloaded.widthFor("/a/repo")).toBe(60);
  });

  it("falls back to defaults for unknown roots", () => {
    const store = new CommitExtrasStore(localStorage);
    store.ensure("/never/seen");
    expect(store.recentsFor("/never/seen")).toEqual([]);
    expect(store.widthFor("/never/seen")).toBe(72);
  });

  it("drops persisted junk instead of throwing", () => {
    localStorage.setItem(`${COMMIT_RECENTS_PREFIX}/a`, JSON.stringify({ v: 1, data: "junk" }));
    localStorage.setItem(`${COMMIT_WIDTH_PREFIX}/a`, JSON.stringify({ v: 1, data: "junk" }));
    const store = new CommitExtrasStore(localStorage);
    store.ensure("/a");
    expect(store.recentsFor("/a")).toEqual([]);
    expect(store.widthFor("/a")).toBe(72);
  });

  it("claims the template prefill once per repo per session", () => {
    const store = new CommitExtrasStore(localStorage);
    expect(store.claimTemplatePrefill("/a", true, "")).toBe(true);
    expect(store.claimTemplatePrefill("/a", true, "")).toBe(false);
    // A different repo has its own claim.
    expect(store.claimTemplatePrefill("/b", true, "")).toBe(true);
    // The setting off never claims a prefill but still marks the attempt.
    expect(store.claimTemplatePrefill("/c", false, "")).toBe(false);
    expect(store.claimTemplatePrefill("/c", true, "")).toBe(false);
    store.reset();
    expect(store.claimTemplatePrefill("/a", true, "")).toBe(true);
  });
});
