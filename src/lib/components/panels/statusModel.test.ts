/**
 * Unit tests for the StatusPanel's pure model (statusModel.ts): section
 * classification, path splitting, kind letters/classes, filtering, and the
 * 10k-entry filter performance budget (<16ms target; 50ms loose CI bound).
 */

import { describe, expect, it } from "vitest";
import type { ChangeKind, StatusEntry, RepoStatus } from "$lib/ipc/types";
import {
  buildStatusIndex,
  classifyEntry,
  filterSections,
  kindClass,
  kindLetter,
  SECTION_ORDER,
  splitPath,
} from "$lib/components/panels/statusModel";

function entry(
  path: string,
  index: ChangeKind,
  worktree: ChangeKind,
  old_path: string | null = null,
): StatusEntry {
  return { path, old_path, index, worktree };
}

const UNMODIFIED: ChangeKind = "unmodified";

describe("classifyEntry", () => {
  it("puts index-side changes in Staged", () => {
    expect(classifyEntry(entry("a.ts", "added", UNMODIFIED))).toEqual([
      "staged",
    ]);
    expect(classifyEntry(entry("a.ts", "renamed", UNMODIFIED, "old.ts"))).toEqual(
      ["staged"],
    );
  });

  it("puts worktree-side changes in Unstaged", () => {
    expect(classifyEntry(entry("a.ts", UNMODIFIED, "modified"))).toEqual([
      "unstaged",
    ]);
    expect(classifyEntry(entry("a.ts", UNMODIFIED, "deleted"))).toEqual([
      "unstaged",
    ]);
  });

  it("an entry with changes on both sides appears in Staged AND Unstaged", () => {
    expect(
      classifyEntry(entry("a.ts", "modified", "modified")),
    ).toEqual(["staged", "unstaged"]);
  });

  it("untracked worktree files land in Untracked", () => {
    expect(classifyEntry(entry("new.ts", UNMODIFIED, "untracked"))).toEqual([
      "untracked",
    ]);
  });

  it("conflicted entries land in Conflicted (plus Unstaged when worktree changed)", () => {
    expect(classifyEntry(entry("c.ts", "conflicted", UNMODIFIED))).toEqual([
      "conflicted",
    ]);
    expect(classifyEntry(entry("c.ts", "conflicted", "modified"))).toEqual([
      "conflicted",
      "unstaged",
    ]);
    // Worktree-side conflict alone also counts.
    expect(classifyEntry(entry("c.ts", UNMODIFIED, "conflicted"))).toEqual([
      "conflicted",
    ]);
  });

  it("fully unmodified (or ignored) entries produce no sections", () => {
    expect(classifyEntry(entry("a.ts", UNMODIFIED, UNMODIFIED))).toEqual([]);
    expect(classifyEntry(entry("a.ts", "ignored", "ignored"))).toEqual([]);
  });
});

describe("splitPath", () => {
  it("splits posix paths keeping the trailing separator on the dir", () => {
    expect(splitPath("src/lib/a.ts")).toEqual({
      dir: "src/lib/",
      base: "a.ts",
    });
  });

  it("splits windows-separator paths", () => {
    expect(splitPath("src\\lib\\a.ts")).toEqual({
      dir: "src\\lib\\",
      base: "a.ts",
    });
  });

  it("root-level files have an empty dir", () => {
    expect(splitPath("README.md")).toEqual({ dir: "", base: "README.md" });
  });
});

describe("kindLetter / kindClass", () => {
  it("maps every ChangeKind to a letter", () => {
    expect(kindLetter("added")).toBe("A");
    expect(kindLetter("modified")).toBe("M");
    expect(kindLetter("deleted")).toBe("D");
    expect(kindLetter("renamed")).toBe("R");
    expect(kindLetter("copied")).toBe("C");
    expect(kindLetter("untracked")).toBe("U");
    expect(kindLetter("conflicted")).toBe("!");
    expect(kindLetter("unmodified")).toBe("·");
  });

  it("maps kinds to color classes (copied shares the renamed/tertiary style)", () => {
    expect(kindClass("added")).toBe("k-added");
    expect(kindClass("deleted")).toBe("k-deleted");
    expect(kindClass("renamed")).toBe("k-renamed");
    expect(kindClass("copied")).toBe("k-renamed");
    expect(kindClass("conflicted")).toBe("k-conflicted");
    expect(kindClass("untracked")).toBe("k-untracked");
  });
});

describe("buildStatusIndex + filterSections", () => {
  const ENTRIES: StatusEntry[] = [
    entry("src/staged.ts", "added", UNMODIFIED),
    entry("src/both.ts", "modified", "modified"),
    entry("src/dirty.ts", UNMODIFIED, "modified"),
    entry("notes/new.txt", UNMODIFIED, "untracked"),
    entry("conflict.ts", "conflicted", "modified"),
    entry("moved.ts", "renamed", UNMODIFIED, "before.ts"),
  ];

  const INDEX = buildStatusIndex(ENTRIES);

  it("builds rows per (entry, section) in SECTION_ORDER", () => {
    expect(INDEX.total).toBe(8); // both.ts and conflict.ts count twice
    expect(
      INDEX.rows.map((row) => `${row.section}:${row.entry.path}`),
    ).toEqual([
      "conflicted:conflict.ts",
      "staged:src/staged.ts",
      "staged:src/both.ts",
      "staged:moved.ts",
      "unstaged:src/both.ts",
      "unstaged:src/dirty.ts",
      "unstaged:conflict.ts",
      "untracked:notes/new.txt",
    ]);
    expect(INDEX.bySection.untracked).toHaveLength(1);
  });

  it("row kind reflects the section's side (staged→index, unstaged→worktree)", () => {
    const stagedBoth = INDEX.bySection.staged.find(
      (r) => r.entry.path === "src/both.ts",
    );
    const unstagedBoth = INDEX.bySection.unstaged.find(
      (r) => r.entry.path === "src/both.ts",
    );
    expect(stagedBoth?.kind).toBe("modified");
    expect(unstagedBoth?.kind).toBe("modified");
    expect(INDEX.bySection.conflicted[0]?.kind).toBe("conflicted");
    expect(INDEX.bySection.untracked[0]?.kind).toBe("untracked");
  });

  it("precomputes dir/base and lowercase search text including old_path", () => {
    const renamed = INDEX.bySection.staged.find((r) => r.base === "moved.ts");
    expect(renamed?.dir).toBe("");
    expect(renamed?.search).toBe("moved.ts before.ts");
    expect(INDEX.bySection.staged[0]?.search).toBe("src/staged.ts");
  });

  it("empty needle returns every (non-empty) section in order", () => {
    const sections = filterSections(INDEX, "");
    expect(sections.map((s) => s.id)).toEqual([...SECTION_ORDER]);
    expect(sections[0]?.rows).toHaveLength(1); // conflicted
  });

  it("filtering is case-insensitive over path and old_path", () => {
    expect(
      filterSections(INDEX, "SRC/").map((s) => s.rows.map((r) => r.base)),
    ).toEqual([["staged.ts", "both.ts"], ["both.ts", "dirty.ts"]]);
    // Rename matching finds the NEW path and the OLD path alike.
    expect(filterSections(INDEX, "before.ts")[0]?.rows[0]?.entry.path).toBe(
      "moved.ts",
    );
  });

  it("a needle with no hits yields no sections", () => {
    expect(filterSections(INDEX, "zzz-nope")).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// Performance: filter 10k entries well under a frame
// ---------------------------------------------------------------------------

describe("statusModel performance", () => {
  const KINDS: ChangeKind[] = ["modified", "added", "deleted", "untracked"];

  /** 10k entries with deterministic variety (mixed dirs, some renames). */
  function entries10k(): StatusEntry[] {
    const out: StatusEntry[] = [];
    for (let i = 0; i < 10_000; i++) {
      const kind = KINDS[i % KINDS.length]!;
      const path = `src/pkg${i % 40}/mod_${i}.ts`;
      const old =
        i % 17 === 0 ? `src/pkg${i % 40}/old_${i}.ts` : null;
      if (kind === "untracked") {
        out.push(entry(path, "unmodified", "untracked", old));
      } else if (i % 5 === 0) {
        out.push(entry(path, kind, "modified", old)); // both sides changed
      } else {
        out.push(entry(path, kind, "unmodified", old));
      }
    }
    return out;
  }

  it("filters 10k entries in <16ms (loose CI bound 50ms)", () => {
    const entries = entries10k();
    const all = buildStatusIndex(entries);
    expect(all.total).toBeGreaterThan(10_000); // some entries in two sections

    const needles = ["src/pkg3/mod_", "old_", "zzz-no-match", ""];
    // Warm-up pass (JIT) so the measured runs reflect steady state.
    for (const n of needles) filterSections(all, n);

    for (const needle of needles) {
      const t0 = performance.now();
      const sections = filterSections(all, needle);
      const ms = performance.now() - t0;
      // eslint-disable-next-line no-console -- perf assertion prints the actual
      console.log(
        `[perf] filter 10k entries (${all.total} rows) needle=${JSON.stringify(needle)}: ${ms.toFixed(2)}ms`,
      );
      expect(ms).toBeLessThan(50); // loose CI bound; target is 16ms
      if (needle === "zzz-no-match") {
        expect(sections).toEqual([]);
      } else if (needle === "") {
        expect(sections.reduce((n, s) => n + s.rows.length, 0)).toBe(all.total);
      } else {
        expect(sections.length).toBeGreaterThan(0);
      }
    }
  });

  it("builds the 10k-entry index once per status change in <50ms", () => {
    const entries = entries10k();
    const t0 = performance.now();
    const index = buildStatusIndex(entries);
    const ms = performance.now() - t0;
    // eslint-disable-next-line no-console -- perf assertion prints the actual
    console.log(`[perf] build index for 10k entries: ${ms.toFixed(2)}ms`);
    expect(index.total).toBeGreaterThan(10_000);
    expect(ms).toBeLessThan(50);
  });

  it("model stays consistent with a full RepoStatus round trip", () => {
    const status: RepoStatus = {
      branch: "main",
      head: "abc123",
      detached: false,
      ahead: 1,
      behind: 2,
      merging: false,
      rebasing: false,
      sequencer: false,
      entries: entries10k().slice(0, 100),
    };
    const index = buildStatusIndex(status.entries);
    // Every entry appears in at least one section (all fixtures are changes).
    for (const e of status.entries) {
      expect(classifyEntry(e).length).toBeGreaterThan(0);
    }
    expect(index.rows.length).toBe(index.total);
  });
});
