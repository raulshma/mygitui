import { describe, expect, it } from "vitest";
import {
  buildStatusBadges,
  compareNames,
  createRoot,
  filterRows,
  findNode,
  flattenVisible,
  insertEntry,
  insertListing,
  removeEntry,
  renameEntry,
  sortSiblings,
  type FileNode,
} from "./fileTreeModel";
import type { DirListing, StatusEntry } from "$lib/ipc/types";

/** Shorthand listing builder (paths derive names). */
function listing(dir: string, entries: Array<[string, "file" | "dir" | "symlink" | "submodule"]>, truncated = false): DirListing {
  return {
    path: dir,
    truncated,
    entries: entries.map(([name, kind]) => ({
      name,
      path: dir === "" ? name : `${dir}/${name}`,
      kind,
      size: kind === "file" ? 3 : null,
    })),
  };
}

describe("compareNames", () => {
  it("sorts naturally (digits as numbers)", () => {
    const names = ["f10.txt", "f2.txt", "f1.txt", "g.txt"];
    expect([...names].sort(compareNames)).toEqual([
      "f1.txt",
      "f2.txt",
      "f10.txt",
      "g.txt",
    ]);
  });

  it("is case-insensitive", () => {
    expect(compareNames("Apple", "banana")).toBeLessThan(0);
    expect(compareNames("BANANA", "apple")).toBeGreaterThan(0);
  });
});

describe("insertListing + findNode", () => {
  it("builds a lazy tree and replaces child lists on re-list", () => {
    const root = createRoot();
    expect(insertListing(root, listing("", [["src", "dir"], ["README.md", "file"]]))).not.toBeNull();
    expect(insertListing(root, listing("src", [["main.rs", "file"], ["lib", "dir"]]))).not.toBeNull();

    const src = findNode(root, "src");
    expect(src?.children?.map((c) => c.name)).toEqual(["lib", "main.rs"]); // dirs first
    expect(findNode(root, "src/main.rs")?.size).toBe(3);
    expect(findNode(root, "src/lib")?.children).toBeNull(); // not listed yet
    expect(findNode(root, "nope")).toBeNull();
  });

  it("creates intermediate dirs for nested listings (stale-parent safety)", () => {
    const root = createRoot();
    // Listing src/lib when src was never listed: chain is synthesized.
    expect(insertListing(root, listing("src/lib", [["a.rs", "file"]]))).not.toBeNull();
    expect(findNode(root, "src/lib/a.rs")).not.toBeNull();
  });

  it("keeps truncated flag on the listed dir", () => {
    const root = createRoot();
    insertListing(root, listing("", [["big", "dir"]], true));
    expect(findNode(root, "big")?.truncated).toBe(false);
    expect(root.truncated).toBe(true);
  });
});

describe("insertEntry / removeEntry / renameEntry", () => {
  function tree(): FileNode {
    const root = createRoot();
    insertListing(root, listing("", [["src", "dir"]]));
    insertListing(root, listing("src", [["main.rs", "file"]]));
    return root;
  }

  it("inserts a new file into a listed parent, sorted", () => {
    const root = tree();
    expect(insertEntry(root, "src", { name: "aaa.rs", kind: "file" })).toBe(true);
    expect(findNode(root, "src/aaa.rs")?.path).toBe("src/aaa.rs");
    expect(
      findNode(root, "src")?.children?.map((c) => c.name),
    ).toEqual(["aaa.rs", "main.rs"]);
    expect(insertEntry(root, "src", { name: "main.rs", kind: "file" })).toBe(false);
    // Unlisted parent: reported, not crash.
    expect(insertEntry(root, "missing/dir", { name: "x", kind: "file" })).toBe(false);
  });

  it("inserts dirs before files", () => {
    const root = tree();
    insertEntry(root, "src", { name: "zdir", kind: "dir" });
    expect(findNode(root, "src")?.children?.[0]?.name).toBe("zdir");
  });

  it("removes deleted entries", () => {
    const root = tree();
    expect(removeEntry(root, "src/main.rs")).toBe(true);
    expect(findNode(root, "src/main.rs")).toBeNull();
    expect(removeEntry(root, "src/main.rs")).toBe(false);
  });

  it("renames within the tree and keeps paths exact", () => {
    const root = tree();
    expect(renameEntry(root, "src/main.rs", "src/renamed.rs")).toBe(true);
    expect(findNode(root, "src/main.rs")).toBeNull();
    expect(findNode(root, "src/renamed.rs")?.path).toBe("src/renamed.rs");

    // Move into the (listed) root.
    expect(renameEntry(root, "src/renamed.rs", "top.rs")).toBe(true);
    expect(findNode(root, "top.rs")?.path).toBe("top.rs");
    expect(findNode(root, "top.rs")?.children).toBeNull();
  });
});

describe("flattenVisible + filterRows", () => {
  function tree(): FileNode {
    const root = createRoot();
    insertListing(root, listing("", [["src", "dir"], ["README.md", "file"]]));
    insertListing(root, listing("src", [["lib", "dir"], ["main.rs", "file"]]));
    insertListing(root, listing("src/lib", [["util.rs", "file"]]));
    findNode(root, "src")!.expanded = true;
    findNode(root, "src/lib")!.expanded = true;
    return root;
  }

  it("flattens only expanded subtrees with depth (dirs first)", () => {
    const root = tree();
    const rows = flattenVisible(root);
    expect(rows.map((r) => r.node.path)).toEqual([
      "src",
      "src/lib",
      "src/lib/util.rs",
      "src/main.rs",
      "README.md",
    ]);
    expect(rows.map((r) => r.depth)).toEqual([0, 1, 2, 1, 0]);
  });

  it("collapses hidden subtrees (dir row itself stays visible)", () => {
    const root = tree();
    findNode(root, "src")!.expanded = false;
    expect(flattenVisible(root).map((r) => r.node.path)).toEqual([
      "src",
      "README.md",
    ]);
  });

  it("filters by substring keeping ancestor rows", () => {
    const root = tree();
    const rows = filterRows(root, "util");
    expect(rows.map((r) => r.node.path)).toEqual([
      "src",
      "src/lib",
      "src/lib/util.rs",
    ]);
  });

  it("matches files directly without expanding siblings", () => {
    const root = tree();
    const rows = filterRows(root, "readme");
    expect(rows.map((r) => r.node.path)).toEqual(["README.md"]);
    expect(filterRows(root, "   ")).toEqual([]);
  });
});

describe("buildStatusBadges", () => {
  const entry = (
    path: string,
    index: StatusEntry["index"],
    worktree: StatusEntry["index"] = "unmodified",
  ): StatusEntry => ({ path, old_path: null, index, worktree });

  it("maps file badges, index side winning", () => {
    const badges = buildStatusBadges([
      entry("a.txt", "unmodified", "modified"),
      entry("b.txt", "added"),
      entry("c.txt", "unmodified", "untracked"),
    ]);
    expect(badges.get("a.txt")).toBe("modified");
    expect(badges.get("b.txt")).toBe("added");
    expect(badges.get("c.txt")).toBe("untracked");
  });

  it("rolls folder badges up with severity", () => {
    const badges = buildStatusBadges([
      entry("src/keep.txt", "unmodified", "modified"),
      entry("src/gone.txt", "deleted"),
      entry("docs/new.txt", "added"),
    ]);
    expect(badges.get("src")).toBe("deleted"); // more severe than modified
    expect(badges.get("docs")).toBe("added");
    expect(badges.has("src/gone.txt")).toBe(true);
    expect(badges.has("docs")).toBe(true);
  });
});

describe("sortSiblings", () => {
  it("places submodules with dirs", () => {
    const root = createRoot();
    insertListing(root, listing("", [["b", "file"], ["a", "dir"], ["s", "submodule"]]));
    expect(root.children!.map((c) => c.name)).toEqual(["a", "s", "b"]);
  });
});

describe("perf gate", () => {
  it("flattens + filters a 100k-node tree in under a frame budget", () => {
    const root = createRoot();
    // 100 dirs × 1000 files, all listed + expanded.
    const entries: Array<[string, "file" | "dir"]> = [];
    for (let d = 0; d < 100; d++) entries.push([`dir${String(d).padStart(3, "0")}`, "dir"]);
    insertListing(root, listing("", entries as never));
    for (let d = 0; d < 100; d++) {
      const dir = `dir${String(d).padStart(3, "0")}`;
      const files: Array<[string, "file"]> = [];
      for (let f = 0; f < 1000; f++) files.push([`file${String(f).padStart(4, "0")}.txt`, "file"]);
      insertListing(root, listing(dir, files as never));
      findNode(root, dir)!.expanded = true;
    }

    const start = performance.now();
    const rows = flattenVisible(root);
    expect(rows.length).toBe(100_100); // 100k files + 100 dir rows
    const filtered = filterRows(root, "file0999");
    expect(filtered.length).toBeGreaterThan(0);
    const elapsed = performance.now() - start;
    // The gate targets algorithmic blowups (a quadratic pass over 100k
    // nodes takes seconds): the budget tolerates a loaded CI runner but
    // still fails any super-linear regression.
    expect(elapsed).toBeLessThan(250);
  });
});
