/**
 * Drop-decision tests for drag-and-drop staging (statusDnd.ts): which
 * target × which row origin = stage / unstage / noop, plus the drag-type
 * marker sniffing that keeps external file drops untouched.
 */
import { describe, expect, it } from "vitest";
import {
  STATUS_ROW_DRAG_TYPE,
  dropAction,
  isStatusRowDrag,
  sectionSide,
} from "./statusDnd";

describe("dropAction", () => {
  it("stages when an unstaged row drops on the staged section", () => {
    expect(dropAction("unstaged", "staged")).toBe("stage");
  });

  it("unstages when a staged row drops on the unstaged section", () => {
    expect(dropAction("staged", "unstaged")).toBe("unstage");
  });

  it("is a no-op when both sides match", () => {
    expect(dropAction("staged", "staged")).toBe("noop");
    expect(dropAction("unstaged", "unstaged")).toBe("noop");
  });
});

describe("sectionSide", () => {
  it("maps the staged section id to the staged side and everything else to unstaged", () => {
    expect(sectionSide("staged")).toBe("staged");
    expect(sectionSide("changes")).toBe("unstaged");
    // AgentChanges' worktree-only groups are all unstaged-side.
    expect(sectionSide("changed")).toBe("unstaged");
    expect(sectionSide("untracked")).toBe("unstaged");
  });
});

describe("isStatusRowDrag", () => {
  /** A DragEvent stand-in: jsdom has no DragEvent constructor. */
  function dragEvent(types: string[]): DragEvent {
    return { dataTransfer: { types } } as unknown as DragEvent;
  }

  it("accepts only drags carrying the status-row marker", () => {
    expect(isStatusRowDrag(dragEvent([STATUS_ROW_DRAG_TYPE]))).toBe(true);
    expect(
      isStatusRowDrag(dragEvent(["text/plain", STATUS_ROW_DRAG_TYPE])),
    ).toBe(true);
  });

  it("rejects foreign drags (OS files, tab reorders) and missing payloads", () => {
    expect(isStatusRowDrag(dragEvent(["Files"]))).toBe(false);
    expect(isStatusRowDrag(dragEvent(["application/x-mygitui-tab"]))).toBe(false);
    expect(isStatusRowDrag({ dataTransfer: null } as unknown as DragEvent)).toBe(
      false,
    );
  });
});
