/**
 * Unit tests for the pure safety model (`safetyModel.ts`) — relative ages,
 * checkpoint titles, danger classification, preview line rendering and
 * reset parameter shapes. No Svelte, no IPC.
 */

import { describe, expect, it } from "vitest";
import type { CheckpointInfo, PreviewInfo } from "$lib/ipc/types";
import {
  buildResetParams,
  buildResetPreviewParams,
  checkpointTitle,
  classifyDanger,
  formatPreviewFile,
  formatRelativeAge,
  isMergedPreview,
  PREVIEW_FILE_CAP,
  previewSummaryLines,
  RESET_MODES,
  resetIsDestructive,
  RESET_PREVIEW_KIND,
  sortNewestFirst,
} from "./safetyModel";

/** Unix-seconds timestamp for 2026-10-07T12:00:00Z. */
const NOW = 1_789_761_600;

function cp(patch: Partial<CheckpointInfo> = {}): CheckpointInfo {
  return {
    id: "1789761600-pre-hard-reset",
    reason: "pre-hard-reset",
    ref_name: "refs/mygitui/checkpoints/1789761600-pre-hard-reset",
    created_at: NOW,
    branch: "main",
    has_worktree_state: true,
    ...patch,
  };
}

function preview(patch: Partial<PreviewInfo> = {}): PreviewInfo {
  return {
    kind: "reset_hard",
    summary: "Hard reset to abc1234",
    files: [{ path: "src/a.ts", change: "modified" }],
    ...patch,
  };
}

describe("formatRelativeAge", () => {
  it("formats the fresh buckets", () => {
    expect(formatRelativeAge(NOW, NOW)).toBe("just now");
    expect(formatRelativeAge(NOW - 30, NOW)).toBe("just now");
    expect(formatRelativeAge(NOW - 60, NOW)).toBe("1m ago");
    expect(formatRelativeAge(NOW - 5 * 60, NOW)).toBe("5m ago");
    expect(formatRelativeAge(NOW - 3 * 3_600, NOW)).toBe("3h ago");
    expect(formatRelativeAge(NOW - 2 * 86_400, NOW)).toBe("2d ago");
  });

  it("formats months and years past 60 days", () => {
    expect(formatRelativeAge(NOW - 61 * 86_400, NOW)).toBe("2mo ago");
    expect(formatRelativeAge(NOW - 400 * 86_400, NOW)).toBe("1y ago");
  });

  it("clamps timestamps from the future to just now", () => {
    expect(formatRelativeAge(NOW + 500, NOW)).toBe("just now");
  });
});

describe("checkpointTitle", () => {
  it("joins reason, relative age and branch", () => {
    expect(checkpointTitle(cp({ created_at: NOW - 120 }), NOW)).toBe(
      "pre-hard-reset · 2m ago · main",
    );
  });

  it("shows detached when there is no branch", () => {
    expect(checkpointTitle(cp({ branch: null }), NOW)).toBe(
      "pre-hard-reset · just now · detached",
    );
    expect(checkpointTitle(cp({ branch: "" }), NOW)).toBe(
      "pre-hard-reset · just now · detached",
    );
  });
});

describe("sortNewestFirst", () => {
  it("sorts by created_at descending without mutating the input", () => {
    const a = cp({ id: "a", created_at: 100 });
    const b = cp({ id: "b", created_at: 300 });
    const c = cp({ id: "c", created_at: 200 });
    const input = [a, b, c];

    expect(sortNewestFirst(input).map((x) => x.id)).toEqual(["b", "c", "a"]);
    expect(input.map((x) => x.id)).toEqual(["a", "b", "c"]);
  });
});

describe("classifyDanger", () => {
  it("marks the tree-destroying previews as danger", () => {
    for (const kind of ["reset_hard", "clean", "checkout_force"]) {
      expect(classifyDanger(kind).level).toBe("danger");
    }
    expect(classifyDanger("reset_hard")).toEqual({
      level: "danger",
      verb: "Hard reset",
      noun: "hard reset",
    });
    expect(classifyDanger("checkout_force").verb).toBe("Force checkout");
    expect(classifyDanger("clean").noun).toBe("clean");
  });

  it("splits branch_delete by merged state", () => {
    expect(classifyDanger("branch_delete", { merged: true })).toEqual({
      level: "warn",
      verb: "Delete",
      noun: "branch delete",
    });
    expect(classifyDanger("branch_delete", { merged: false })).toEqual({
      level: "danger",
      verb: "Force delete",
      noun: "force branch delete",
    });
    // Default (no merged flag) fails loud.
    expect(classifyDanger("branch_delete").level).toBe("danger");
  });

  it("treats unknown kinds as danger", () => {
    expect(classifyDanger("explode")).toEqual({
      level: "danger",
      verb: "explode",
      noun: "explode",
    });
  });
});

describe("previewSummaryLines", () => {
  it("puts the summary first and formats file rows", () => {
    const info = preview({
      summary: "Hard reset to abc1234",
      files: [
        { path: "src/a.ts", change: "modified" },
        { path: "old.txt", change: "" },
      ],
    });
    expect(previewSummaryLines(info)).toEqual([
      "Hard reset to abc1234",
      "src/a.ts — modified",
      "old.txt",
    ]);
  });

  it("caps the file list and appends an `and N more` line", () => {
    const files = Array.from({ length: PREVIEW_FILE_CAP + 7 }, (_, i) => ({
      path: `f${i}.txt`,
      change: "deleted",
    }));
    const lines = previewSummaryLines(preview({ files }));
    // summary + cap + trailing count
    expect(lines).toHaveLength(1 + PREVIEW_FILE_CAP + 1);
    expect(lines[0]).toBe("Hard reset to abc1234");
    expect(lines.at(-1)).toBe("and 7 more");
    expect(lines.at(-2)).toBe("f49.txt — deleted");
  });

  it("handles a missing summary and empty file list", () => {
    expect(previewSummaryLines(preview({ summary: "", files: [] }))).toEqual(
      [],
    );
    expect(previewSummaryLines(preview({ files: [] }))).toEqual([
      "Hard reset to abc1234",
    ]);
  });

  it("formats a single file with and without a change label", () => {
    expect(formatPreviewFile({ path: "a", change: "added" })).toBe(
      "a — added",
    );
    expect(formatPreviewFile({ path: "a", change: "" })).toBe("a");
  });
});

describe("isMergedPreview", () => {
  it("detects the backend's merged shape (no files, merged summary)", () => {
    expect(
      isMergedPreview(preview({ summary: "merged into HEAD", files: [] })),
    ).toBe(true);
  });

  it("is false for unmerged (commits listed) or other summaries", () => {
    expect(
      isMergedPreview(
        preview({
          summary: "3 commits become unreachable",
          files: [{ path: "abc123", change: "add feature" }],
        }),
      ),
    ).toBe(false);
    expect(isMergedPreview(preview({ summary: "Hard reset to abc", files: [] }))).toBe(
      false,
    );
  });
});

describe("reset params", () => {
  it("buildResetParams passes kind + to verbatim", () => {
    expect(buildResetParams("hard", "origin/main")).toEqual({
      kind: "hard",
      to: "origin/main",
    });
    expect(buildResetParams("soft", "HEAD~1")).toEqual({
      kind: "soft",
      to: "HEAD~1",
    });
  });

  it("exposes the preview kind and its params shape", () => {
    expect(RESET_PREVIEW_KIND).toBe("reset_hard");
    expect(buildResetPreviewParams("feat")).toEqual({ to: "feat" });
  });

  it("describes all five reset modes", () => {
    expect(RESET_MODES.map((m) => m.kind)).toEqual([
      "soft",
      "mixed",
      "keep",
      "merge",
      "hard",
    ]);
    for (const mode of RESET_MODES) {
      expect(mode.label.length).toBeGreaterThan(0);
      expect(mode.description.length).toBeGreaterThan(0);
    }
  });

  it("classifies hard/keep/merge as destructive, soft/mixed as safe", () => {
    expect(resetIsDestructive("hard")).toBe(true);
    expect(resetIsDestructive("keep")).toBe(true);
    expect(resetIsDestructive("merge")).toBe(true);
    expect(resetIsDestructive("soft")).toBe(false);
    expect(resetIsDestructive("mixed")).toBe(false);
  });
});
