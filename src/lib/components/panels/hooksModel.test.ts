/**
 * Tests for the Hooks section's derived state (panels/hooksModel.ts):
 * path derivation for local roots, action availability (open needs the
 * file present, copy only needs a root; remote/no-root rows have neither),
 * and the summary line.
 */
import { describe, expect, it } from "vitest";
import type { HookInfo } from "$lib/ipc/types";
import { hookPathFor, hookRowsFor, hooksSummary } from "./hooksModel";

function hook(kind: string, present: boolean, executable = false): HookInfo {
  return { kind, present, executable };
}

describe("hookPathFor", () => {
  it("joins the root with .git/hooks/<kind>", () => {
    expect(hookPathFor("/repos/demo", "pre-commit")).toBe("/repos/demo/.git/hooks/pre-commit");
    expect(hookPathFor("/repos/demo/", "commit-msg")).toBe("/repos/demo/.git/hooks/commit-msg");
    expect(hookPathFor("C:\\repos\\demo", "pre-push")).toBe("C:\\repos\\demo/.git/hooks/pre-push");
  });
});

describe("hookRowsFor", () => {
  it("derives path + affordances for a local repo", () => {
    const rows = hookRowsFor(
      [hook("pre-commit", true, true), hook("commit-msg", false), hook("pre-push", true, false)],
      "/repos/demo",
    );
    expect(rows).toEqual([
      {
        kind: "pre-commit",
        present: true,
        executable: true,
        path: "/repos/demo/.git/hooks/pre-commit",
        canOpen: true,
        canCopy: true,
      },
      {
        kind: "commit-msg",
        present: false,
        executable: false,
        path: "/repos/demo/.git/hooks/commit-msg",
        canOpen: false, // file does not exist — nothing to open
        canCopy: true,
      },
      {
        kind: "pre-push",
        present: true,
        executable: false,
        path: "/repos/demo/.git/hooks/pre-push",
        canOpen: true,
        canCopy: true,
      },
    ]);
  });

  it("disables every action without a local root (remote tabs)", () => {
    const rows = hookRowsFor([hook("pre-commit", true, true)], null);
    expect(rows[0]).toMatchObject({ path: "", canOpen: false, canCopy: false });
  });
});

describe("hooksSummary", () => {
  it("counts installed and runnable hooks", () => {
    const hooks = [hook("a", true, true), hook("b", true, false), hook("c", false)];
    expect(hooksSummary(hooks)).toBe("2 of 3 installed · 1 runnable.");
  });

  it("omits the runnable clause when none is executable", () => {
    expect(hooksSummary([hook("a", true), hook("b", false)])).toBe("1 of 2 installed.");
  });

  it("covers the empty list", () => {
    expect(hooksSummary([])).toBe("No hooks reported.");
  });
});
