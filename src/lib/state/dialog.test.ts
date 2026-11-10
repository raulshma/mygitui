/**
 * Tests for the shared dialog-state machine (`$lib/state/dialog`): the
 * closed/open union shape and arg atomicity.
 */
import { describe, expect, it } from "vitest";
import { closedDialog, openDialog, type DialogState } from "./dialog";

type Args = { title: string; initial: string };

describe("DialogState", () => {
  it("closedDialog produces a closed state for any arg type", () => {
    const state: DialogState<Args> = closedDialog();
    expect(state).toEqual({ open: false });
  });

  it("openDialog carries the full arg set together with open", () => {
    const state = openDialog<Args>({ title: "Rename", initial: "main" });
    expect(state).toEqual({ open: true, title: "Rename", initial: "main" });
    if (state.open) {
      expect(state.title).toBe("Rename");
      expect(state.initial).toBe("main");
    }
  });

  it("args exist only while open", () => {
    const closed: DialogState<Args> = closedDialog();
    expect(closed.open).toBe(false);
    // The closed branch has no arg fields at all — invalid combos are
    // unrepresentable, not merely avoided.
    expect(Object.keys(closed)).toEqual(["open"]);
  });
});
