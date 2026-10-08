/**
 * Unit tests for the pure forge model (`forgeModel.ts`): check
 * summarization, branch→PR matching, age formatting, banner mapping and PR
 * URL number extraction. No IPC, no Svelte.
 */

import { describe, expect, it } from "vitest";
import type { CheckInfo, PrInfo } from "$lib/ipc/client";
import {
  checkIcon,
  checkSummaryLabel,
  isPrOpen,
  prAgeLabel,
  prForBranch,
  prNumberFromUrl,
  prStateClass,
  prStateLabel,
  statusBanner,
  summarizeChecks,
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
