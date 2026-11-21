/**
 * Indication state machine tests (E1 §9.2) — pure derivation from snapshot
 * data → chip states, no stores, no mounting. Covers every state (loading,
 * none, ok, running, warning, failing, merged/closed, unpushed, degraded),
 * the worst-state collapse ladder, attention/failure classification and the
 * degraded guidance copy.
 */

import { describe, expect, it } from "vitest";
import type { CheckInfo, ForgeSnapshot, PrInfo, WorkflowRun } from "$lib/ipc/client";
import {
  checksChip,
  countChecks,
  degradedFromFailureKind,
  degradedGuidance,
  degradedLabel,
  deriveChips,
  isAttention,
  isFailure,
  prChip,
  prHeadDiverged,
  repoDegraded,
  retryLabel,
  runDurationLabel,
  runsChip,
  summarize,
  worstChip,
  type ChipId,
  type ForgeChipInput,
  type ForgeChipState,
} from "./indication.svelte";

function pr(patch: Partial<PrInfo> = {}): PrInfo {
  return {
    number: 222,
    title: "Add the footer",
    head_ref_name: "feat",
    head_ref_oid: "",
    base_ref_name: "main",
    state: "OPEN",
    is_draft: false,
    url: "https://github.com/a/b/pull/222",
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
    mergeable: "",
    labels: [],
    ...patch,
  };
}

function check(name: string, state: CheckInfo["state"]): CheckInfo {
  return { name, state };
}

function run(patch: Partial<WorkflowRun> = {}): WorkflowRun {
  return {
    database_id: 1802,
    number: 77,
    name: "CI",
    display_title: "CI",
    event: "push",
    head_branch: "feat",
    head_sha: "abc",
    status: "completed",
    conclusion: "success",
    url: "https://github.com/a/b/actions/runs/1802",
    created_at: null,
    updated_at: null,
    started_at: "2026-11-21T10:00:00Z",
    attempt: 1,
    workflow_database_id: 1,
    ...patch,
  };
}

const READY: ForgeChipInput = {
  loading: false,
  degraded: null,
  degradedMessage: "",
  retryInMs: null,
};

describe("prChip", () => {
  it("renders loading before data and none without a PR", () => {
    expect(prChip({ ...READY, loading: true }, null)).toEqual({
      kind: "loading",
      text: "…",
    });
    expect(prChip(READY, null)).toEqual({ kind: "none", text: "no PR" });
  });

  it("carries number + state in the text, never color alone", () => {
    expect(prChip(READY, pr())).toEqual({ kind: "ok", text: "#222" });
    expect(prChip(READY, pr({ is_draft: true }))).toEqual({
      kind: "warning",
      text: "#222 · draft",
    });
    expect(
      prChip(READY, pr({ review_decision: "CHANGES_REQUESTED" })),
    ).toEqual({ kind: "warning", text: "#222 · changes requested" });
    expect(prChip(READY, pr({ state: "MERGED" }))).toEqual({
      kind: "merged",
      text: "#222 · merged",
    });
    expect(prChip(READY, pr({ state: "CLOSED" }))).toEqual({
      kind: "closed",
      text: "#222 · closed",
    });
  });

  it("degraded overrides the data states", () => {
    expect(
      prChip({ ...READY, degraded: "not-authed", degradedMessage: "run gh auth login" }, pr()),
    ).toMatchObject({ kind: "degraded", degraded: "not-authed" });
  });
});

describe("checksChip", () => {
  it("shows unpushed before any check data (§9.2)", () => {
    expect(checksChip(READY, [check("ci", "fail")], false)).toEqual({
      kind: "unpushed",
      text: "unpushed",
    });
    // null = unknown: normal states apply.
    expect(checksChip(READY, null, null)).toEqual({ kind: "none", text: "no checks" });
  });

  it("summarizes counts with the failure count in the text", () => {
    const checks = [check("a", "pass"), check("b", "fail"), check("c", "pass"), check("d", "pending")];
    expect(checksChip(READY, checks, true)).toEqual({
      kind: "failing",
      count: 1,
      text: "2/4 · 1 failing",
    });
  });

  it("pending CI runs as the running state (spinner, no elapsed)", () => {
    expect(checksChip(READY, [check("a", "pass"), check("b", "pending")], true)).toEqual({
      kind: "running",
      startedAtMs: null,
      text: "1/2 · running",
    });
  });

  it("all green is ok; skipping counts toward the total (pass/total)", () => {
    expect(checksChip(READY, [check("a", "pass"), check("b", "skipping")], true)).toEqual({
      kind: "ok",
      text: "1/2",
    });
  });

  it("loading with no checks yet is loading", () => {
    expect(checksChip({ ...READY, loading: true }, null, true)).toEqual({
      kind: "loading",
      text: "…",
    });
  });
});

describe("runsChip", () => {
  const NOW = Date.parse("2026-11-21T10:02:30Z");

  it("no run is none", () => {
    expect(runsChip(READY, null, NOW)).toEqual({ kind: "none", text: "no runs" });
  });

  it("in_progress is running with the started_at elapsed anchor", () => {
    expect(
      runsChip(READY, run({ status: "in_progress", conclusion: "" }), NOW),
    ).toEqual({
      kind: "running",
      startedAtMs: Date.parse("2026-11-21T10:00:00Z"),
      text: "#77",
    });
  });

  it("queued is a warning (visible, not alarming)", () => {
    expect(runsChip(READY, run({ status: "queued", conclusion: "" }), NOW)).toEqual({
      kind: "warning",
      text: "#77 · queued",
    });
  });

  it("success is ok with the run duration", () => {
    expect(runsChip(READY, run({ updated_at: "2026-11-21T10:02:00Z" }), NOW)).toEqual({
      kind: "ok",
      text: "#77 · 2m",
    });
  });

  it("failures are failing; cancelled is muted none with the outcome text", () => {
    expect(runsChip(READY, run({ conclusion: "failure" }), NOW)).toEqual({
      kind: "failing",
      count: 1,
      text: "#77 · 2m",
    });
    expect(runsChip(READY, run({ conclusion: "timed_out" }), NOW).kind).toBe("failing");
    const cancelled = runsChip(READY, run({ conclusion: "cancelled" }), NOW);
    expect(cancelled.kind).toBe("none");
    expect(cancelled.text).toBe("#77 · cancelled");
  });

  it("runDurationLabel ticks live runs and humanizes", () => {
    expect(runDurationLabel(run({ status: "in_progress", conclusion: "" }), NOW)).toBe("2m");
    expect(runDurationLabel(run({ started_at: null }), NOW)).toBe(null);
    expect(runDurationLabel(run({ started_at: "bogus" }), NOW)).toBe(null);
  });
});

describe("worstChip collapse (§9.2 <1024px)", () => {
  it("follows failing > degraded > running > warning > unpushed > ok > merged/closed > none > loading", () => {
    const ladder: ForgeChipState["kind"][] = [
      "loading",
      "none",
      "merged",
      "closed",
      "ok",
      "unpushed",
      "warning",
      "running",
      "degraded",
      "failing",
    ];
    const fakeText = "";
    const states: Array<[ChipId, ForgeChipState]> = ladder.map((kind) => [
      "checks",
      { kind, text: fakeText } as ForgeChipState,
    ]);
    for (let i = 0; i < ladder.length; i += 1) {
      for (let j = i + 1; j < ladder.length; j += 1) {
        // merged/closed share a rank (settled states) — a tie keeps the
        // earlier chip, so skip that pair.
        if (
          (ladder[i] === "merged" && ladder[j] === "closed") ||
          (ladder[i] === "closed" && ladder[j] === "merged")
        ) {
          continue;
        }
        const winner = worstChip([states[i], states[j]]);
        expect(winner?.state.kind, `${ladder[j]} beats ${ladder[i]}`).toBe(
          ladder[j],
        );
      }
    }
  });

  it("ties keep the earlier chip (PR, then checks, then runs)", () => {
    const ok = { kind: "ok", text: "x" } as never;
    const winner = worstChip([
      ["pr", ok],
      ["checks", ok],
      ["runs", ok],
    ]);
    expect(winner?.id).toBe("pr");
  });

  it("a failure never hides behind ok states", () => {
    const worst = worstChip([
      ["pr", { kind: "ok", text: "#1" } as never],
      ["checks", { kind: "failing", count: 2, text: "1/2 · 2 failing" } as never],
      ["runs", { kind: "ok", text: "#9" } as never],
    ]);
    expect(worst?.state.kind).toBe("failing");
  });
});

describe("attention + failure classification", () => {
  it("attention = failing, degraded, warning, unpushed", () => {
    expect(isAttention({ kind: "failing", count: 1, text: "" })).toBe(true);
    expect(isAttention({ kind: "degraded", degraded: "offline", text: "", message: "", retryInMs: null })).toBe(true);
    expect(isAttention({ kind: "warning", text: "" })).toBe(true);
    expect(isAttention({ kind: "unpushed", text: "" })).toBe(true);
    expect(isAttention({ kind: "running", text: "", startedAtMs: null })).toBe(false);
    expect(isAttention({ kind: "ok", text: "" })).toBe(false);
    expect(isAttention({ kind: "none", text: "" })).toBe(false);
    expect(isAttention({ kind: "loading", text: "" })).toBe(false);
  });

  it("failure = failing only (toast set)", () => {
    expect(isFailure({ kind: "failing", count: 1, text: "" })).toBe(true);
    expect(isFailure({ kind: "warning", text: "" })).toBe(false);
    expect(isFailure({ kind: "degraded", degraded: "offline", text: "", message: "", retryInMs: null })).toBe(false);
  });
});

describe("degraded derivation", () => {
  it("maps ForgeError kinds onto named degraded states", () => {
    expect(degradedFromFailureKind("NoGh")).toBe("no-gh");
    expect(degradedFromFailureKind("NotAuthed")).toBe("not-authed");
    expect(degradedFromFailureKind("NotRepo")).toBe("not-repo");
    expect(degradedFromFailureKind("RateLimited")).toBe("rate-limited");
    expect(degradedFromFailureKind("Offline")).toBe("offline");
    expect(degradedFromFailureKind("Other")).toBe(null);
    expect(degradedFromFailureKind(null)).toBe(null);
  });

  it("global gh status wins over snapshot failures", () => {
    expect(repoDegraded({ available: false, version: "", authed: false }, "RateLimited", null)).toBe("no-gh");
    expect(
      repoDegraded({ available: true, version: "2.63.3", authed: false }, "RateLimited", null),
    ).toBe("not-authed");
    expect(
      repoDegraded({ available: true, version: "2.63.3", authed: true }, "NotRepo", null),
    ).toBe("not-repo");
  });

  it("falls back to the snapshot's part error kind", () => {
    const snapshot = {
      error: { kind: "RateLimited", message: "quota", url: null },
    } as unknown as ForgeSnapshot;
    expect(repoDegraded(null, null, snapshot)).toBe("rate-limited");
    expect(repoDegraded(null, null, null)).toBe(null);
  });
});

describe("guidance + labels", () => {
  it("no-gh and unauthed guidance carry the copyable login command", () => {
    expect(degradedGuidance("no-gh", "").command).toBe("gh auth login");
    expect(degradedGuidance("no-gh", "").docsUrl).toContain("cli.github.com");
    expect(degradedGuidance("not-authed", "").command).toBe("gh auth login");
    expect(degradedGuidance("not-repo", "origin is not a GitHub remote").message).toContain(
      "origin is not a GitHub remote",
    );
    expect(degradedGuidance("rate-limited", "").command).toBe(null);
    expect(degradedGuidance("offline", "").command).toBe(null);
  });

  it("degraded labels are short chip text", () => {
    expect(degradedLabel("no-gh")).toBe("no gh");
    expect(degradedLabel("not-authed")).toBe("not authed");
    expect(degradedLabel("not-repo")).toBe("off");
    expect(degradedLabel("offline")).toBe("offline");
    expect(degradedLabel("rate-limited")).toBe("rate limited");
  });

  it("retryLabel humanizes the backoff estimate", () => {
    expect(retryLabel(null)).toBe("");
    expect(retryLabel(45_000)).toBe("retry in 45s");
    expect(retryLabel(120_000)).toBe("retry in 2m");
    expect(retryLabel(0)).toBe("");
  });
});

describe("deriveChips + helpers", () => {
  it("derives all three chips + collapse + attention count", () => {
    const snapshot = {
      pr: pr({ is_draft: true }),
      head_checks: [check("a", "pass"), check("b", "fail")],
      pushed: true,
      run: run({ conclusion: "failure", updated_at: "2026-11-21T10:02:00Z" }),
    } as unknown as ForgeSnapshot;
    const chips = deriveChips(READY, snapshot, Date.parse("2026-11-21T10:02:30Z"));
    expect(chips.pr.kind).toBe("warning");
    expect(chips.checks.kind).toBe("failing");
    expect(chips.runs.kind).toBe("failing");
    expect(chips.worst?.state.kind).toBe("failing");
    expect(chips.attentionCount).toBe(3);
  });

  it("prHeadDiverged needs both shas and a difference", () => {
    expect(prHeadDiverged(null)).toBe(false);
    expect(prHeadDiverged({ pr_head_sha: "", head_sha: "abc" })).toBe(false);
    expect(prHeadDiverged({ pr_head_sha: "abc", head_sha: "abc" })).toBe(false);
    expect(prHeadDiverged({ pr_head_sha: "def", head_sha: "abc" })).toBe(true);
  });

  it("countChecks + summarize describe the check list in one line", () => {
    const checks = [check("a", "pass"), check("b", "pass"), check("c", "fail"), check("d", "pending"), check("e", "skipping")];
    expect(countChecks(checks)).toEqual({ pass: 2, fail: 1, pending: 1, total: 5 });
    expect(summarize(checks)).toBe("2/5 · 1 failing");
    expect(summarize([check("a", "pass")])).toBe("1/1");
    expect(summarize([])).toBe("no checks");
  });
});
