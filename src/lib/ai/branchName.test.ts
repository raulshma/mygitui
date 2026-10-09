/**
 * Unit tests for the branch-name wrapper (ai/branchName.ts): the pure
 * description builder (context → one prompt description) and the
 * call-through to `ai.run` with a stubbed store path.
 */

import { describe, expect, it, vi } from "vitest";

// Stub the app-wide store BEFORE importing the module under test: the
// suggestion path must route through `ai.run` (opt-in gate + busy state).
const runMock = vi.fn();
vi.mock("$lib/ai/ai.svelte", () => ({
  ai: { run: (...args: unknown[]) => runMock(...args) },
}));

import {
  branchNameDescription,
  MAX_SUBJECTS,
  suggestBranchName,
} from "$lib/ai/branchName";

describe("branchNameDescription", () => {
  it("renders just the description when alone", () => {
    expect(branchNameDescription({ description: "fix login crash" })).toBe(
      "What it is for: fix login crash",
    );
  });

  it("folds in recent subjects and the base branch", () => {
    const out = branchNameDescription({
      description: "retry logic",
      recentSubjects: ["Add backoff", "Fix flaky test"],
      baseBranch: "main",
    });
    expect(out).toContain("What it is for: retry logic");
    expect(out).toContain("Recent commit subjects: Add backoff; Fix flaky test");
    expect(out).toContain("Branches from: main");
  });

  it("caps recent subjects", () => {
    const out = branchNameDescription({
      description: "x",
      recentSubjects: ["sub1", "sub2", "sub3", "sub4", "sub5", "sub6", "sub7"],
    });
    expect(out).not.toContain("sub6");
    expect(out).not.toContain("sub7");
    // Exactly MAX_SUBJECTS survive.
    expect(out.split("; ").length - 1).toBe(MAX_SUBJECTS - 1);
  });

  it("collapses whitespace and skips empty parts", () => {
    expect(
      branchNameDescription({
        description: "  multi\n  space \t words  ",
        recentSubjects: ["", "   "],
      }),
    ).toBe("What it is for: multi space words");
  });

  it("returns an empty string for an empty context", () => {
    expect(branchNameDescription({ description: "" })).toBe("");
  });
});

describe("suggestBranchName", () => {
  it("routes through ai.run with the built description and parses the name", async () => {
    runMock.mockResolvedValue({
      kind: "branch-name",
      result: { text: "fix-login-crash", model: "m", backend: "opencode", elapsedMs: 1 },
      branchName: "fix-login-crash",
    });
    const name = await suggestBranchName("repo-1", { description: "fix login crash" });
    expect(name).toBe("fix-login-crash");
    expect(runMock).toHaveBeenCalledWith(
      "branch-name",
      { repoId: "repo-1", description: "What it is for: fix login crash" },
      undefined,
    );
  });

  it("falls back to the raw text when parsing yielded no name", async () => {
    runMock.mockResolvedValue({
      kind: "branch-name",
      result: { text: " raw-name ", model: "m", backend: "opencode", elapsedMs: 1 },
    });
    const name = await suggestBranchName("repo-1", { description: "x" });
    expect(name).toBe("raw-name");
  });

  it("propagates AiErrors (opt-in gating etc.)", async () => {
    runMock.mockRejectedValue(new Error("opt-in required"));
    await expect(
      suggestBranchName("repo-1", { description: "x" }),
    ).rejects.toThrow("opt-in required");
  });
});
