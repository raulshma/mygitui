/**
 * Unit tests for the StatusPanel's selection algebra (statusSelection.ts):
 * VS Code list semantics — key identity (`section:path`), toggle/range/all,
 * prune on refresh, selection following files across a stage/unstage move,
 * selection-scoped actions, and the stage/unstage/discard split.
 */

import { describe, expect, it } from "vitest";
import type { ChangeKind, StatusEntry } from "$lib/ipc/types";
import { buildStatusIndex, type StatusRow } from "$lib/components/panels/statusModel";
import {
  actionRows,
  allSelection,
  countLabel,
  parseRowKey,
  pruneSelection,
  rangeSelection,
  remapSelection,
  selectionActions,
  toggleSelection,
} from "$lib/components/panels/statusSelection";

function entry(
  path: string,
  index: ChangeKind,
  worktree: ChangeKind,
): StatusEntry {
  return { path, old_path: null, index, worktree };
}

/** All rows (both sections) for the given entries, in section order. */
function rows(entries: StatusEntry[]): StatusRow[] {
  return buildStatusIndex(entries).rows;
}

/** Entry modified in the index AND the worktree → a row in each group. */
function both(path: string): StatusEntry {
  return entry(path, "modified", "modified");
}

describe("toggleSelection", () => {
  it("adds a missing key", () => {
    expect(toggleSelection(["changes:a"], "changes:b")).toEqual([
      "changes:a",
      "changes:b",
    ]);
  });

  it("removes a present key", () => {
    expect(toggleSelection(["changes:a", "changes:b"], "changes:a")).toEqual([
      "changes:b",
    ]);
  });

  it("toggles per key, not per path: same path in both groups is distinct", () => {
    expect(toggleSelection(["staged:a", "changes:a"], "changes:a")).toEqual([
      "staged:a",
    ]);
  });
});

describe("rangeSelection", () => {
  // buildStatusIndex orders staged rows before changes rows, so the
  // visible list is [staged:a, staged:d, changes:a, changes:b, changes:c].
  const list = rows([
    both("a"),
    entry("b", "unmodified", "modified"),
    entry("c", "unmodified", "modified"),
    both("d"),
  ]);

  it("selects forward inclusive", () => {
    expect(rangeSelection(list, 1, 2)).toEqual(["staged:d", "changes:a"]);
  });

  it("selects reversed ranges", () => {
    expect(rangeSelection(list, 3, 0)).toEqual(list.slice(0, 4).map((r) => r.key));
  });

  it("clamps a single index", () => {
    expect(rangeSelection(list, 2, 2)).toEqual(["changes:a"]);
  });
});

describe("allSelection / pruneSelection", () => {
  it("selects every row key", () => {
    const list = rows([both("a"), entry("b", "unmodified", "modified")]);
    expect(allSelection(list)).toEqual(["staged:a", "changes:a", "changes:b"]);
  });

  it("drops keys whose rows vanished", () => {
    const list = rows([entry("a", "unmodified", "modified")]);
    expect(pruneSelection(["changes:a", "changes:gone"], list)).toEqual([
      "changes:a",
    ]);
  });
});

describe("remapSelection", () => {
  const afterStage = rows([both("a"), entry("b", "added", "unmodified")]);
  const afterUnstage = rows([
    both("a"),
    entry("b", "unmodified", "modified"),
    entry("c", "unmodified", "modified"),
  ]);

  it("moves a staged file's selection from Changes to Staged Changes", () => {
    expect(
      remapSelection(["changes:a"], new Set(["a"]), false, afterStage),
    ).toEqual(["staged:a"]);
  });

  it("moves an unstaged file's selection back to Changes", () => {
    expect(
      remapSelection(["staged:b"], new Set(["b"]), true, afterUnstage),
    ).toEqual(["changes:b"]);
  });

  it("keeps the old key when the counterpart has not appeared yet", () => {
    expect(
      remapSelection(["changes:a"], new Set(["a"]), false, rows([])),
    ).toEqual(["changes:a"]);
  });

  it("keeps untouched live keys and drops untouched dead ones", () => {
    expect(
      remapSelection(
        ["changes:c", "changes:gone", "staged:a"],
        new Set(["a"]),
        true,
        afterUnstage,
      ),
    ).toEqual(["changes:c", "changes:a"]);
  });
  it("remaps submodule prefixed keys between changes and staged", () => {
    const subRows = buildStatusIndex(
      [both("foo.txt"), entry("bar.ts", "added", "unmodified")],
      "sub-a",
      "sub-a",
    ).rows;
    expect(
      remapSelection(["sub-a:changes:foo.txt"], new Set(["foo.txt"]), false, subRows),
    ).toEqual(["sub-a:staged:foo.txt"]);

    expect(
      remapSelection(["sub-a:staged:foo.txt"], new Set(["foo.txt"]), true, subRows),
    ).toEqual(["sub-a:changes:foo.txt"]);
  });

  it("scopes remap to the specified prefix when multiple repos share paths", () => {
    const combinedRows = [
      ...rows([both("foo.txt")]),
      ...buildStatusIndex([both("foo.txt")], "sub-a", "sub-a").rows,
    ];
    expect(
      remapSelection(
        ["changes:foo.txt", "sub-a:changes:foo.txt"],
        new Set(["foo.txt"]),
        false,
        combinedRows,
        "sub-a",
      ),
    ).toEqual(["changes:foo.txt", "sub-a:staged:foo.txt"]);
  });
});

describe("parseRowKey", () => {
  it("parses standard section:path keys", () => {
    expect(parseRowKey("staged:src/foo.ts")).toEqual({
      prefix: "",
      section: "staged",
      path: "src/foo.ts",
    });
    expect(parseRowKey("changes:README.md")).toEqual({
      prefix: "",
      section: "changes",
      path: "README.md",
    });
  });

  it("parses prefixed submodule keys", () => {
    expect(parseRowKey("sub-a:staged:file.txt")).toEqual({
      prefix: "sub-a",
      section: "staged",
      path: "file.txt",
    });
    expect(parseRowKey("nested/sub:changes:lib/mod.rs")).toEqual({
      prefix: "nested/sub",
      section: "changes",
      path: "lib/mod.rs",
    });
  });

  it("returns null for unrecognized keys", () => {
    expect(parseRowKey("random-string")).toBeNull();
    expect(parseRowKey("")).toBeNull();
  });
});

describe("actionRows", () => {
  const list = rows([both("a"), entry("b", "unmodified", "modified")]);

  it("targets only the row when it is not selected", () => {
    expect(
      actionRows(list[2]!, ["changes:a"], list).map((r) => r.key),
    ).toEqual(["changes:b"]);
  });

  it("targets the whole selection when the row is part of it", () => {
    expect(
      actionRows(list[0]!, ["staged:a", "changes:b"], list).map((r) => r.key),
    ).toEqual(["staged:a", "changes:b"]);
  });
});

describe("selectionActions", () => {
  const list = rows([
    entry("staged-only", "modified", "unmodified"),
    entry("work-only", "unmodified", "modified"),
    entry("conflict", "conflicted", "modified"),
    entry("untracked", "untracked", "untracked"),
  ]);

  it("splits rows by section and excludes conflicts", () => {
    expect(selectionActions(list)).toEqual({
      stage: ["work-only", "untracked"],
      unstage: ["staged-only"],
      discard: ["work-only", "untracked"],
    });
  });
});

describe("countLabel", () => {
  it("stays bare for one file", () => {
    expect(countLabel("Stage", 1)).toBe("Stage");
  });

  it("adds the count for many files", () => {
    expect(countLabel("Unstage", 3)).toBe("Unstage 3 files");
  });
});
