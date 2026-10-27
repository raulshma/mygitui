/**
 * Unit tests for the action-run popout handoff (`runPopout.ts`): snapshot
 * round trips, malformed-payload rejection and bounded accumulation.
 */

import { beforeEach, describe, expect, it } from "vitest";
import type { ActionRun } from "$lib/stores/actions.svelte";
import {
  ACTION_RUN_POPOUT_PREFIX,
  readStashedActionRun,
  stashActionRun,
} from "./runPopout";

function makeRun(patch: Partial<ActionRun> & { run_id: string }): ActionRun {
  return {
    repo_id: "repo-1",
    name: "Lint",
    command: "npm run lint",
    lines: ["one"],
    dropped: 0,
    done: false,
    exit_code: null,
    order: 1,
    status: "running",
    ...patch,
  };
}

beforeEach(() => {
  localStorage.clear();
});

describe("stashActionRun / readStashedActionRun", () => {
  it("round trips a run", () => {
    const run = makeRun({ run_id: "act-1", lines: ["a", "b"], dropped: 3 });
    stashActionRun(run);
    expect(readStashedActionRun("act-1")).toEqual(run);
  });

  it("returns null for unknown, malformed or foreign payloads", () => {
    expect(readStashedActionRun("nope")).toBeNull();

    localStorage.setItem(`${ACTION_RUN_POPOUT_PREFIX}bad-json`, "{");
    expect(readStashedActionRun("bad-json")).toBeNull();

    localStorage.setItem(
      `${ACTION_RUN_POPOUT_PREFIX}bad-shape`,
      JSON.stringify({ run_id: "bad-shape", lines: "nope" }),
    );
    expect(readStashedActionRun("bad-shape")).toBeNull();

    localStorage.setItem(
      `${ACTION_RUN_POPOUT_PREFIX}bad-status`,
      JSON.stringify({ ...makeRun({ run_id: "bad-status" }), status: "zombie" }),
    );
    expect(readStashedActionRun("bad-status")).toBeNull();
  });

  it("prunes old snapshots once the stash grows past its bound", () => {
    for (let i = 0; i < 25; i += 1) {
      stashActionRun(makeRun({ run_id: `r${i}` }));
    }
    expect(readStashedActionRun("r0")).toBeNull();
    expect(readStashedActionRun("r24")).not.toBeNull();
    const stashed = Object.keys(localStorage).filter((key) =>
      key.startsWith(ACTION_RUN_POPOUT_PREFIX),
    );
    expect(stashed).toHaveLength(20);
  });
});
