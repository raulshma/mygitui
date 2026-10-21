/**
 * Unit tests for the pure forge model (`forgeModel.ts`): check
 * summarization, branch→PR matching, age formatting, banner mapping and PR
 * URL number extraction. No IPC, no Svelte.
 */

import { describe, expect, it } from "vitest";
import type { CheckInfo, PrInfo } from "$lib/ipc/client";
import {
  activeWorkflows,
  checkIcon,
  checkSummaryLabel,
  isPrOpen,
  parseWorkflowInputs,
  prAgeLabel,
  prForBranch,
  prMergeBlockedReason,
  prNumberFromUrl,
  prStateClass,
  prStateLabel,
  reviewDecisionLabel,
  runCanRerun,
  runDurationLabel,
  runHasFailures,
  runIsActive,
  runStateClass,
  runStateIcon,
  runStateKind,
  runStateLabel,
  statusBanner,
  stepStateClass,
  stepStateLabel,
  summarizeChecks,
  workflowStateLabel,
  worstChipClass,
  type Banner,
  type CheckSummary,
} from "./forgeModel";

function check(name: string, state: CheckInfo["state"]): CheckInfo {
  return { name, state };
}

function pr(patch: Partial<PrInfo> & { number: number }): PrInfo {
  return {
    title: `PR ${patch.number}`,
    head_ref_name: "feat",
    base_ref_name: "main",
    state: "OPEN",
    is_draft: false,
    url: `https://github.com/acme/widget/pull/${patch.number}`,
    created_at: null,
    updated_at: null,
    merged_at: null,
    closed_at: null,
    author: "",
    body: "",
    additions: 0,
    deletions: 0,
    changed_files: 0,
    review_decision: "",
    mergeable: "MERGEABLE",
    labels: [],
    ...patch,
  };
}

describe("summarizeChecks", () => {
  it("counts states and picks the worst (fail > pending > skipping > pass)", () => {
    const summary = summarizeChecks([
      check("a", "pass"),
      check("b", "pass"),
      check("c", "skipping"),
    ]);
    expect(summary).toEqual<CheckSummary>({
      pass: 2,
      fail: 0,
      pending: 0,
      skipping: 1,
      total: 3,
      worst: "skipping",
    });
    expect(summarizeChecks([check("a", "pending"), check("b", "pass")]).worst).toBe("pending");
    expect(summarizeChecks([check("a", "fail"), check("b", "pending")]).worst).toBe("fail");
    expect(summarizeChecks([check("a", "pass")]).worst).toBe("pass");
  });

  it("empty input → none", () => {
    expect(summarizeChecks([])).toEqual<CheckSummary>({
      pass: 0,
      fail: 0,
      pending: 0,
      skipping: 0,
      total: 0,
      worst: "none",
    });
  });

  it("labels and classes per worst state", () => {
    expect(checkSummaryLabel(summarizeChecks([]))).toBe("no checks");
    expect(
      checkSummaryLabel(summarizeChecks([check("a", "pass"), check("b", "pass")])),
    ).toBe("2/2 checks");
    expect(
      checkSummaryLabel(summarizeChecks([check("a", "pass"), check("b", "fail")])),
    ).toBe("1/2 checks");
    expect(
      checkSummaryLabel(summarizeChecks([check("a", "pass"), check("b", "pending")])),
    ).toBe("1/2 checks · pending");
    expect(worstChipClass("fail")).toBe("chip-fail");
    expect(worstChipClass("none")).toBe("chip-none");
    expect(checkIcon("pass")).toBe("✓");
    expect(checkIcon("fail")).toBe("✗");
    expect(checkIcon("skipping")).toBe("–");
    expect(checkIcon("pending")).toBe("•");
  });
});

describe("prForBranch", () => {
  it("matches head_ref_name, preferring OPEN over merged/closed", () => {
    const merged = pr({ number: 1, state: "MERGED" });
    const open = pr({ number: 2, state: "OPEN" });
    const closed = pr({ number: 3, state: "CLOSED" });
    expect(prForBranch([merged, closed, open], "feat")).toBe(open);
    expect(prForBranch([merged, closed], "feat")).toBe(merged);
    expect(prForBranch([], "feat")).toBeNull();
    expect(prForBranch([open], "")).toBeNull();
    expect(prForBranch([pr({ number: 9, head_ref_name: "other" })], "feat")).toBeNull();
  });
});

describe("pr state presentation", () => {
  it("maps states to labels/classes and openness", () => {
    expect(prStateLabel("OPEN")).toBe("Open");
    expect(prStateLabel("MERGED")).toBe("Merged");
    expect(prStateLabel("CLOSED")).toBe("Closed");
    expect(prStateLabel("weird")).toBe("weird");
    expect(prStateClass("OPEN")).toBe("chip-pr-open");
    expect(prStateClass("MERGED")).toBe("chip-pr-merged");
    expect(prStateClass("CLOSED")).toBe("chip-pr-closed");
    expect(isPrOpen("OPEN")).toBe(true);
    expect(isPrOpen("MERGED")).toBe(false);
  });
});

describe("pr mergeability + review decision", () => {
  it("blocks merge for non-open, draft and conflicting PRs", () => {
    expect(prMergeBlockedReason({ state: "OPEN", is_draft: false, mergeable: "MERGEABLE" })).toBeNull();
    expect(prMergeBlockedReason({ state: "OPEN", is_draft: true, mergeable: "MERGEABLE" })).toContain(
      "ready",
    );
    expect(
      prMergeBlockedReason({ state: "OPEN", is_draft: false, mergeable: "CONFLICTING" }),
    ).toContain("conflicts");
    expect(prMergeBlockedReason({ state: "MERGED", is_draft: false, mergeable: "MERGEABLE" })).toContain(
      "not open",
    );
  });

  it("labels known review decisions and blanks unknown ones", () => {
    expect(reviewDecisionLabel("APPROVED")).toBe("Approved");
    expect(reviewDecisionLabel("CHANGES_REQUESTED")).toBe("Changes requested");
    expect(reviewDecisionLabel("REVIEW_REQUIRED")).toBe("Review required");
    expect(reviewDecisionLabel("")).toBe("");
    expect(reviewDecisionLabel("SOMETHING_ELSE")).toBe("");
  });
});

describe("prAgeLabel", () => {
  const NOW = Date.parse("2026-10-07T12:00:00Z");

  function age(iso: string): string | null {
    return prAgeLabel(iso, NOW);
  }

  it("formats relative ages", () => {
    expect(age("2026-10-07T11:59:40Z")).toBe("just now");
    expect(age("2026-10-07T11:55:00Z")).toBe("5m");
    expect(age("2026-10-07T09:00:00Z")).toBe("3h");
    expect(age("2026-10-05T12:00:00Z")).toBe("2d");
    expect(age("2026-09-04T12:00:00Z")).toBe("4w");
    expect(age("2026-01-10T12:00:00Z")).toBe("9mo");
    expect(age("2024-06-07T12:00:00Z")).toBe("2y");
  });

  it("nulls on missing or unparsable input", () => {
    expect(prAgeLabel(null, NOW)).toBeNull();
    expect(prAgeLabel("not a date", NOW)).toBeNull();
  });
});

describe("statusBanner", () => {
  it("maps status onto banner guidance", () => {
    expect(statusBanner(null)).toEqual<Banner>({
      kind: "loading",
      text: expect.stringContaining("Checking"),
    });
    const missing = statusBanner({ available: false, version: "", authed: false });
    expect(missing.kind).toBe("missing");
    expect(missing.text).toContain("install");

    const unauthed = statusBanner({ available: true, version: "2.63.3", authed: false });
    expect(unauthed.kind).toBe("unauthed");
    expect(unauthed.text).toContain("2.63.3");

    const ready = statusBanner({ available: true, version: "2.63.3", authed: true });
    expect(ready).toEqual<Banner>({ kind: "ready", text: "" });
  });
});

describe("prNumberFromUrl", () => {
  it("extracts numbers from /pull/ URLs", () => {
    expect(prNumberFromUrl("https://github.com/a/b/pull/42")).toBe(42);
    expect(prNumberFromUrl("https://github.com/a/b/pull/7#issuecomment-1")).toBe(7);
    expect(prNumberFromUrl("https://github.com/a/b/pull/")).toBeNull();
    expect(prNumberFromUrl("https://github.com/a/b/tree/main")).toBeNull();
    expect(prNumberFromUrl(null)).toBeNull();
  });
});

describe("workflow run state", () => {
  it("maps lifecycle statuses and conclusions onto buckets", () => {
    expect(runStateKind({ status: "in_progress", conclusion: "" })).toBe("running");
    expect(runStateKind({ status: "queued", conclusion: "" })).toBe("queued");
    expect(runStateKind({ status: "waiting", conclusion: "" })).toBe("queued");
    expect(runStateKind({ status: "completed", conclusion: "success" })).toBe(
      "success",
    );
    expect(runStateKind({ status: "completed", conclusion: "failure" })).toBe(
      "failure",
    );
    expect(runStateKind({ status: "completed", conclusion: "timed_out" })).toBe(
      "failure",
    );
    expect(runStateKind({ status: "completed", conclusion: "cancelled" })).toBe(
      "cancelled",
    );
    expect(runStateKind({ status: "completed", conclusion: "skipped" })).toBe(
      "skipped",
    );
    expect(runStateKind({ status: "completed", conclusion: "" })).toBe("unknown");
  });

  it("labels and icons every bucket", () => {
    expect(runStateLabel({ status: "in_progress", conclusion: "" })).toBe(
      "Running",
    );
    expect(runStateLabel({ status: "queued", conclusion: "" })).toBe("Queued");
    expect(runStateLabel({ status: "completed", conclusion: "failure" })).toBe(
      "Failed",
    );
    expect(runStateLabel({ status: "completed", conclusion: "timed_out" })).toBe(
      "Timed out",
    );
    expect(runStateLabel({ status: "completed", conclusion: "startup_failure" })).toBe(
      "Startup failure",
    );
    expect(runStateLabel({ status: "completed", conclusion: "" })).toBe(
      "Completed",
    );
    expect(runStateIcon("success")).toBe("✓");
    expect(runStateIcon("failure")).toBe("✗");
    expect(runStateIcon("running")).toBe("●");
    expect(runStateClass("failure")).toBe("run-failure");
  });

  it("derives action availability", () => {
    expect(runIsActive({ status: "in_progress" })).toBe(true);
    expect(runIsActive({ status: "queued" })).toBe(true);
    expect(runIsActive({ status: "completed" })).toBe(false);
    expect(runIsActive({ status: "" })).toBe(false);
    expect(runCanRerun({ status: "completed" })).toBe(true);
    expect(runCanRerun({ status: "in_progress" })).toBe(false);
    expect(runHasFailures({ conclusion: "failure" })).toBe(true);
    expect(runHasFailures({ conclusion: "timed_out" })).toBe(true);
    expect(runHasFailures({ conclusion: "success" })).toBe(false);
  });

  it("formats durations compactly", () => {
    expect(
      runDurationLabel("2026-10-20T02:00:00Z", "2026-10-20T02:00:45Z"),
    ).toBe("45s");
    expect(
      runDurationLabel("2026-10-20T02:00:00Z", "2026-10-20T02:02:10Z"),
    ).toBe("2m 10s");
    expect(
      runDurationLabel("2026-10-20T02:00:00Z", "2026-10-20T05:08:00Z"),
    ).toBe("3h 8m");
    expect(runDurationLabel(null, "2026-10-20T05:08:00Z")).toBeNull();
    expect(
      runDurationLabel("2026-10-20T05:08:00Z", "2026-10-20T02:00:00Z"),
    ).toBeNull();
  });
});

describe("workflow job/step labels", () => {
  it("labels and classes conclusions", () => {
    expect(stepStateLabel("failure")).toBe("Failed");
    expect(stepStateLabel("success")).toBe("Success");
    expect(stepStateLabel("", "in_progress")).toBe("Running");
    expect(stepStateLabel("", "")).toBe("Pending");
    expect(stepStateClass("failure")).toBe("run-failure");
    expect(stepStateClass("skipped")).toBe("run-skipped");
  });

  it("labels workflow file states and filters active ones", () => {
    expect(workflowStateLabel("active")).toBe("Active");
    expect(workflowStateLabel("disabled_manually")).toBe("Disabled");
    expect(
      activeWorkflows([
        { id: 1, name: "CI", path: "ci.yml", state: "active" },
        {
          id: 2,
          name: "Old",
          path: "old.yml",
          state: "disabled_manually",
        },
      ]),
    ).toEqual([{ id: 1, name: "CI", path: "ci.yml", state: "active" }]);
  });
});

describe("parseWorkflowInputs", () => {
  it("parses key=value lines, skipping blanks and comments", () => {
    expect(parseWorkflowInputs("version=1.2.3\n\n# note\ndry_run = true\n")).toEqual({
      inputs: [
        { key: "version", value: "1.2.3" },
        { key: "dry_run", value: "true" },
      ],
      error: null,
    });
  });

  it("keeps '=' inside values", () => {
    expect(parseWorkflowInputs("flags=--x=1")).toEqual({
      inputs: [{ key: "flags", value: "--x=1" }],
      error: null,
    });
  });

  it("rejects malformed, empty, and duplicate keys", () => {
    expect(parseWorkflowInputs("novalue").error).toContain("key=value");
    expect(parseWorkflowInputs("=x").error).toContain("empty");
    expect(parseWorkflowInputs("a=1\na=2").error).toContain("Duplicate");
  });
});
