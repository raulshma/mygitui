/**
 * Unit tests for the rebase planner pure model (`./plannerModel`): range
 * extraction, plan construction, exact `RebaseStep` serialization, validation
 * rules, reordering and cherry-pick ordering. No runes, no IPC, no DOM.
 */

import { describe, expect, it } from "vitest";
import type { CommitInfo } from "$lib/ipc/types";
import {
  REBASE_ACTIONS,
  buildPlan,
  cutAtBase,
  moveRow,
  orderForCherryPick,
  rangeForRebase,
  toRebaseSteps,
  validatePlan,
  type PlanRow,
} from "./plannerModel";

// ---------------------------------------------------------------------------
// Fixtures — newest-first flat log, like HistoryStore.flat.commits
// ---------------------------------------------------------------------------

function commit(sha: string, summary = `subject ${sha}`): CommitInfo {
  return {
    sha,
    parents: [],
    author: { name: "Ada", email: "ada@example.com", time: 1, offset_minutes: 0 },
    committer: { name: "Ada", email: "ada@example.com", time: 1, offset_minutes: 0 },
    message: `${summary}\n\nbody of ${sha}\n`,
    summary,
    refs: [],
  };
}

/** Newest-first: head is the first entry. */
const LOG: CommitInfo[] = [commit("head"), commit("mid"), commit("old"), commit("base"), commit("root")];

function row(sha: string, action: PlanRow["action"], message = ""): PlanRow {
  return { sha, summary: `subject ${sha}`, action, message };
}

// ---------------------------------------------------------------------------
// rangeForRebase / cutAtBase
// ---------------------------------------------------------------------------

describe("rangeForRebase", () => {
  it("returns the whole list for a null base (all reachable from HEAD)", () => {
    expect(rangeForRebase(LOG, null)).toEqual(LOG);
  });

  it("cuts HEAD..baseSha exclusive of the base", () => {
    expect(rangeForRebase(LOG, "base").map((c) => c.sha)).toEqual([
      "head",
      "mid",
      "old",
    ]);
  });

  it("returns an empty range when the base is HEAD itself", () => {
    expect(rangeForRebase(LOG, "head")).toEqual([]);
  });

  it("returns everything when the base is not in the list", () => {
    expect(rangeForRebase(LOG, "missing")).toEqual(LOG);
  });

  it("does not mutate the input array", () => {
    const before = LOG.map((c) => c.sha);
    rangeForRebase(LOG, "mid");
    expect(LOG.map((c) => c.sha)).toEqual(before);
  });
});

describe("cutAtBase", () => {
  it("returns the slice once the base is accumulated", () => {
    expect(cutAtBase(LOG.slice(0, 3), "mid")).toEqual([LOG[0]]);
  });

  it("returns null while the base has not been seen yet", () => {
    expect(cutAtBase(LOG.slice(0, 2), "base")).toBeNull();
  });

  it("treats the HEAD sha as an immediately empty range (not null)", () => {
    expect(cutAtBase(LOG, "head")).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// buildPlan
// ---------------------------------------------------------------------------

describe("buildPlan", () => {
  it("reverses the newest-first range into oldest-first todo order", () => {
    const plan = buildPlan([LOG[0] as CommitInfo, LOG[1] as CommitInfo, LOG[2] as CommitInfo]);
    expect(plan.map((r) => r.sha)).toEqual(["old", "mid", "head"]);
  });

  it("defaults every row to pick and prefills the reword buffer", () => {
    const [row] = buildPlan([LOG[0] as CommitInfo]);
    expect(row?.action).toBe("pick");
    expect(row?.summary).toBe("subject head");
    expect(row?.message).toBe((LOG[0] as CommitInfo).message);
  });

  it("builds an empty plan for an empty range", () => {
    expect(buildPlan([])).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// toRebaseSteps — exact backend serialization
// ---------------------------------------------------------------------------

describe("toRebaseSteps", () => {
  it("serializes the exact RebaseStep triple", () => {
    const steps = toRebaseSteps([
      row("a".repeat(40), "pick"),
      row("b".repeat(40), "reword", "new subject\n"),
      row("c".repeat(40), "drop"),
    ]);
    expect(steps).toEqual([
      { sha: "a".repeat(40), action: "pick", new_message: null },
      { sha: "b".repeat(40), action: "reword", new_message: "new subject\n" },
      { sha: "c".repeat(40), action: "drop", new_message: null },
    ]);
  });

  it("keeps the raw (untrimmed) message when it has non-whitespace content", () => {
    const steps = toRebaseSteps([row("a".repeat(40), "reword", "  kept  \n")]);
    expect(steps[0]?.new_message).toBe("  kept  \n");
  });

  it("serializes new_message null for a blank reword message", () => {
    for (const blank of ["", "   \n\t"]) {
      const steps = toRebaseSteps([row("a".repeat(40), "reword", blank)]);
      expect(steps[0]?.new_message).toBeNull();
    }
  });

  it("ignores the message buffer for every action except reword", () => {
    const steps = toRebaseSteps([
      row("a".repeat(40), "pick", "ignored"),
      row("b".repeat(40), "squash", "ignored"),
      row("c".repeat(40), "fixup", "ignored"),
      row("d".repeat(40), "edit", "ignored"),
      row("e".repeat(40), "drop", "ignored"),
    ]);
    expect(steps.map((s) => s.new_message)).toEqual([null, null, null, null, null]);
  });
});

// ---------------------------------------------------------------------------
// validatePlan
// ---------------------------------------------------------------------------

describe("validatePlan", () => {
  it("rejects an empty plan", () => {
    expect(validatePlan([])).toMatch(/empty/i);
  });

  it("accepts a valid all-pick plan", () => {
    expect(validatePlan(toRebaseSteps([row("a1", "pick"), row("b2", "pick")]))).toBeNull();
  });

  it("rejects squash as the first step", () => {
    const err = validatePlan(toRebaseSteps([row("a1", "squash"), row("b2", "pick")]));
    expect(err).toMatch(/[Ss]quash.*first/);
  });

  it("rejects fixup as the first step", () => {
    const err = validatePlan(toRebaseSteps([row("a1", "fixup")]));
    expect(err).toMatch(/[Ff]ixup.*first/);
  });

  it("accepts squash/fixup after the first row", () => {
    expect(
      validatePlan(toRebaseSteps([row("a1", "pick"), row("b2", "squash"), row("c3", "fixup")])),
    ).toBeNull();
  });

  it("rejects a reword without a message", () => {
    const err = validatePlan(toRebaseSteps([row("a1", "pick"), row("b2", "reword", "  ")]));
    expect(err).toMatch(/[Rr]eword.*message/);
  });

  it("accepts a reword with a message", () => {
    expect(validatePlan(toRebaseSteps([row("a1", "reword", "renamed")]))).toBeNull();
  });

  it("rejects unknown actions and names the action", () => {
    const err = validatePlan([{ sha: "a1b2c3d", action: "explode", new_message: null }]);
    expect(err).toMatch(/"explode"/);
  });

  it("reports only the first problem", () => {
    const err = validatePlan([
      { sha: "first0", action: "squash", new_message: null },
      { sha: "second1", action: "nope", new_message: null },
    ]);
    expect(err).toMatch(/[Ss]quash.*first/);
    expect(err).not.toMatch(/nope/);
  });

  it("knows every REBASE_ACTIONS entry as valid", () => {
    const plan = REBASE_ACTIONS.map((action, i) => ({
      sha: `sha${i}xxxx`,
      action,
      new_message: action === "reword" ? "msg" : null,
    })).reverse(); // keep squash/fixup off the first row
    expect(validatePlan(plan)).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// moveRow
// ---------------------------------------------------------------------------

describe("moveRow", () => {
  const rows = ["a", "b", "c", "d"];

  it("moves a row down", () => {
    expect(moveRow(rows, 0, 2)).toEqual(["b", "c", "a", "d"]);
  });

  it("moves a row up", () => {
    expect(moveRow(rows, 3, 1)).toEqual(["a", "d", "b", "c"]);
  });

  it("returns the same array when from === to", () => {
    expect(moveRow(rows, 1, 1)).toBe(rows);
  });

  it("returns the same array for out-of-range indices", () => {
    expect(moveRow(rows, -1, 0)).toBe(rows);
    expect(moveRow(rows, 0, 4)).toBe(rows);
    expect(moveRow(rows, 4, 0)).toBe(rows);
  });

  it("does not mutate the input", () => {
    moveRow(rows, 0, 3);
    expect(rows).toEqual(["a", "b", "c", "d"]);
  });
});

// ---------------------------------------------------------------------------
// orderForCherryPick
// ---------------------------------------------------------------------------

describe("orderForCherryPick", () => {
  it("orders a newest-first selection oldest-first", () => {
    // Selection arrives in list (newest-first) order from a shift-range.
    const picked = orderForCherryPick(["head", "mid", "old"], LOG);
    expect(picked).toEqual(["old", "mid", "head"]);
  });

  it("handles an arbitrary selection order", () => {
    expect(orderForCherryPick(["mid", "head"], LOG)).toEqual(["mid", "head"]);
    expect(orderForCherryPick(["base", "head"], LOG)).toEqual(["base", "head"]);
  });

  it("deduplicates repeated shas", () => {
    expect(orderForCherryPick(["mid", "mid", "head"], LOG)).toEqual(["mid", "head"]);
  });

  it("drops shas that are not in the commit list", () => {
    expect(orderForCherryPick(["ghost", "mid"], LOG)).toEqual(["mid"]);
  });

  it("returns an empty array for an empty selection or empty log", () => {
    expect(orderForCherryPick([], LOG)).toEqual([]);
    expect(orderForCherryPick(["head"], [])).toEqual([]);
  });

  it("never mutates its inputs", () => {
    const shas = ["head", "mid"];
    orderForCherryPick(shas, LOG);
    expect(shas).toEqual(["head", "mid"]);
  });
});
