/**
 * Component tests for DiffViewer's line-granular selection (M12, lane C):
 * gutter clicks build a selection, shift-click extends within the hunk,
 * the hunk header gains "Stage selected"/"Discard selected" (dispatching
 * the same `{lines: …}` StageTarget the hunk actions use), Escape clears,
 * and everything is gated behind the hunkStaging/hunkDiscard props.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { tick } from "svelte";
import type { Transport } from "$lib/ipc/client";
import { resetTransport, setTransport } from "$lib/ipc/client";
import type { DiffHunk, DiffLine, FileDiff } from "$lib/ipc/types";
import DiffViewer from "$lib/components/diff/DiffViewer.svelte";

// svelte must resolve to its CLIENT build under vitest (see GraphCanvas.test.ts).
vi.mock("svelte", async () => {
  const client = await import(
    // @ts-expect-error runtime-only redirect into svelte's client build (no d.ts).
    "../../../../node_modules/svelte/src/index-client.js"
  );
  return { ...client, tick: client.tick };
});

// Keep the (lazy) Shiki pipeline out of the test entirely.
vi.mock("$lib/diff/highlight", () => ({
  cachedTokens: () => null,
  requestTokens: () => {},
  onTokensLanded: () => () => {},
  syntaxVersion: () => 0,
  setScheme: () => {},
}));

// ---------------------------------------------------------------------------
// Fixtures

function line(
  origin: DiffLine["origin"],
  text: string,
  oldNo: number | null,
  newNo: number | null,
): DiffLine {
  return { old_no: oldNo, new_no: newNo, origin, text, highlights: [] };
}

function fixture(): FileDiff[] {
  const hunk: DiffHunk = {
    old_start: 1,
    new_start: 1,
    lines: [
      line(" ", "keep", 1, 1),
      line("-", "old", 2, null),
      line("+", "new", null, 2),
      line("+", "new2", null, 3),
      line(" ", "tail", 4, 4),
    ],
  };
  return [
    {
      path: "src/a.txt",
      old_path: null,
      binary: false,
      is_image: false,
      additions: 2,
      deletions: 1,
      hunks: [hunk],
    },
  ];
}

/** Transport answering nothing; records every invocation. */
function recordingTransport(): Transport {
  return () => Promise.resolve();
}

class ResizeObserverStub implements ResizeObserver {
  static instances: ResizeObserverStub[] = [];
  constructor(readonly callback: ResizeObserverCallback) {
    ResizeObserverStub.instances.push(this);
  }
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
  fire(): void {
    this.callback([], this);
  }
}

interface ViewerProps {
  files: FileDiff[];
  repoId?: string;
  hunkStaging?: "stage" | "unstage";
  hunkDiscard?: boolean;
  mode?: "unified" | "split";
  onMutated?: () => void;
}

function mountViewer(props: Partial<ViewerProps> = {}) {
  const all: ViewerProps = {
    files: fixture(),
    repoId: "r1",
    hunkStaging: "stage",
    hunkDiscard: true,
    mode: "unified",
    ...props,
  };
  return render(DiffViewer, { props: all });
}

/** The gutter button whose text is `label`, scoped to one row. */
function gutter(container: HTMLElement, labelText: string, occurrence = 0): HTMLElement {
  const hits = [...container.querySelectorAll<HTMLButtonElement>("button.no")]
    .filter((b) => b.textContent === labelText);
  const hit = hits[occurrence];
  if (!hit) throw new Error(`no gutter "${labelText}" (#${occurrence})`);
  return hit;
}

async function click(labelText: string, container: HTMLElement, shift = false): Promise<void> {
  fireEvent.click(gutter(container, labelText), { shiftKey: shift });
  await tick();
}

beforeEach(() => {
  ResizeObserverStub.instances = [];
  vi.stubGlobal("ResizeObserver", ResizeObserverStub);
  // jsdom has no Element.scrollTo (the viewer resets scroll on new diffs).
  if (typeof Element.prototype.scrollTo !== "function") {
    Element.prototype.scrollTo = () => {};
  }
  resetTransport();
});

afterEach(() => {
  resetTransport();
  cleanup();
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------

describe("DiffViewer line selection", () => {
  it("is inert without repoId (read-only viewers see plain gutters)", async () => {
    const { container } = render(DiffViewer, {
      props: { files: fixture(), mode: "unified" },
    });
    await tick();
    expect(container.querySelector("button.no.click")).toBeNull();
    expect(container.querySelector(".sel-count")).toBeNull();
  });

  it("a gutter click starts a selection and shows the header actions", async () => {
    const { container, getByText } = mountViewer();
    await click("2", container); // old-side number of the '-' line
    expect(container.querySelector(".sel-count")?.textContent).toContain("1 selected");
    expect(getByText("Stage selected")).toBeTruthy();
    expect(getByText("Discard selected")).toBeTruthy();
  });

  it("shift-click extends within the hunk; ranges merge into spans", async () => {
    const { container } = mountViewer();
    await click("1", container); // context line, new-side 1
    await click("3", container, true); // extend to new-side 3
    expect(container.querySelector(".sel-count")?.textContent).toContain("3 selected");
  });

  it("selected rows render highlighted ('-' runs follow their paired '+')", async () => {
    const { container } = mountViewer();
    await click("2", container); // selects new-side 2
    // Backend semantics: the '-' run pairs with BOTH '+' lines and is kept
    // iff ANY is selected → the '-' row renders selected too.
    expect(container.querySelectorAll(".row.line.selected").length).toBe(2);
  });

  it("Escape clears the selection", async () => {
    const { container } = mountViewer();
    await click("2", container);
    expect(container.querySelector(".sel-count")).toBeTruthy();
    fireEvent.keyDown(document, { key: "Escape" });
    await tick();
    expect(container.querySelector(".sel-count")).toBeNull();
  });

  it("Stage selected dispatches the {lines} StageTarget", async () => {
    const transport = vi.fn(recordingTransport());
    setTransport(transport);
    const onMutated = vi.fn();
    const { container, getByText } = mountViewer({ onMutated });
    await click("1", container);
    await click("3", container, true);
    fireEvent.click(getByText("Stage selected"));
    await vi.waitFor(() => {
      expect(transport).toHaveBeenCalledWith(
        "stage",
        expect.objectContaining({
          repo_id: "r1",
          request: {
            targets: [
              { lines: { path: "src/a.txt", hunk: 0, ranges: [{ start: 1, end: 3 }] } },
            ],
            unstage: false,
          },
        }),
      );
    });
    await vi.waitFor(() => {
      expect(onMutated).toHaveBeenCalled();
    });
  });

  it("Discard selected dispatches discard with the same target", async () => {
    const transport = vi.fn(recordingTransport());
    setTransport(transport);
    const { container, getByText } = mountViewer();
    await click("2", container);
    fireEvent.click(getByText("Discard selected"));
    await vi.waitFor(() => {
      expect(transport).toHaveBeenCalledWith(
        "discard",
        expect.objectContaining({
          repo_id: "r1",
          targets: [
            { lines: { path: "src/a.txt", hunk: 0, ranges: [{ start: 2, end: 2 }] } },
          ],
        }),
      );
    });
  });

  it("Unstage mode relabels the action and sends unstage: true", async () => {
    const transport = vi.fn(recordingTransport());
    setTransport(transport);
    const { container, getByText } = mountViewer({ hunkStaging: "unstage" });
    await click("2", container);
    fireEvent.click(getByText("Unstage selected"));
    await vi.waitFor(() => {
      expect(transport).toHaveBeenCalledWith(
        "stage",
        expect.objectContaining({
          request: expect.objectContaining({ unstage: true }),
        }),
      );
    });
  });

  it("hunk-level discard prop gates the selection discard action", async () => {
    const { container, queryByText } = mountViewer({ hunkDiscard: false });
    await click("2", container);
    expect(queryByText("Discard selected")).toBeNull();
    expect(queryByText("Discard hunk")).toBeNull();
  });

  it("a fresh diff resets any selection", async () => {
    const rendered = mountViewer();
    await click("2", rendered.container);
    expect(rendered.container.querySelector(".sel-count")).toBeTruthy();
    await rendered.rerender({
      files: fixture(),
      repoId: "r1",
      hunkStaging: "stage",
      hunkDiscard: true,
      mode: "unified",
    });
    await tick();
    expect(rendered.container.querySelector(".sel-count")).toBeNull();
  });
});
