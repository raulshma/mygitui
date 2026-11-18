/**
 * Component tests for the Agent-mode changes list: the roving-tabindex
 * keyboard model (one tab-reachable row; Up/Down move focus, Enter opens),
 * the failed-mutation toast (a failed stage is visible, not console-only),
 * and the section windowing — large groups render a window between
 * spacers instead of 200-row truncation.
 */

import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import type { ChangeKind, RepoStatus, StatusEntry } from "$lib/ipc/types";
import AgentChanges from "./AgentChanges.svelte";

const { toastSpy } = vi.hoisted(() => ({ toastSpy: vi.fn() }));

// The mutation failure path must be observable without the backend: stage
// rejects, `refreshStatus` is stubbed out (it would IPC otherwise), and the
// toast spy records the fired message.
vi.mock("$lib/ipc/client", () => ({
  stage: vi.fn(() => Promise.reject(new Error("index locked"))),
  discard: vi.fn(() => Promise.resolve()),
}));
vi.mock("$lib/stores/tabs.svelte", () => ({
  refreshStatus: vi.fn(async () => {}),
}));
vi.mock("$lib/toast", async (importOriginal) => {
  const actual = await importOriginal<typeof import("$lib/toast")>();
  return { ...actual, toast: toastSpy };
});

afterEach(() => {
  cleanup();
  toastSpy.mockClear();
});

// jsdom lays nothing out: the tree scroller's clientHeight is always 0,
// which would make every section's window empty. A fixed 480px viewport
// gives the window math real numbers (20 rows + overscan).
const realClientHeight = Object.getOwnPropertyDescriptor(
  HTMLElement.prototype,
  "clientHeight",
);
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get() {
      return 480;
    },
  });
});
afterAll(() => {
  Object.defineProperty(HTMLElement.prototype, "clientHeight", realClientHeight ?? {
    configurable: true,
    get() {
      return 0;
    },
  });
});

function entry(path: string, index: ChangeKind, worktree: ChangeKind): StatusEntry {
  return { path, old_path: null, index, worktree };
}

function status(entries: StatusEntry[]): RepoStatus {
  return {
    branch: "main",
    head: null,
    detached: false,
    ahead: 0,
    behind: 0,
    merging: false,
    rebasing: false,
    sequencer: false,
    bisecting: false,
    entries,
  };
}

function renderedRows(): HTMLElement[] {
  return screen.getAllByRole("treeitem");
}

describe("AgentChanges roving tabindex", () => {
  it("exactly one row is tab-reachable (the first), the rest are -1", async () => {
    render(AgentChanges, {
      props: {
        repoId: "r",
        status: status([
          entry("a.ts", "unmodified", "modified"),
          entry("b.ts", "unmodified", "modified"),
          entry("c.ts", "unmodified", "modified"),
        ]),
      },
    });
    await tick();

    const rows = renderedRows();
    expect(rows.map((row) => row.getAttribute("tabindex"))).toEqual(["0", "-1", "-1"]);
  });

  it("ArrowDown/ArrowUp move focus; Enter opens the focused row", async () => {
    const onOpenPath = vi.fn();
    render(AgentChanges, {
      props: {
        repoId: "r",
        status: status([
          entry("a.ts", "unmodified", "modified"),
          entry("b.ts", "unmodified", "modified"),
        ]),
        onOpenPath,
      },
    });
    await tick();

    const rows = renderedRows();
    rows[0]!.focus();
    fireEvent.keyDown(rows[0]!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(rows[1]);

    fireEvent.keyDown(rows[1]!, { key: "Enter" });
    expect(onOpenPath).toHaveBeenCalledWith("b.ts", "worktree");

    fireEvent.keyDown(rows[1]!, { key: "ArrowUp" });
    expect(document.activeElement).toBe(rows[0]);

    // Home/End jump to the clamps.
    fireEvent.keyDown(rows[0]!, { key: "End" });
    expect(document.activeElement).toBe(rows[1]);
    fireEvent.keyDown(rows[1]!, { key: "Home" });
    expect(document.activeElement).toBe(rows[0]);
  });

  it("pointer clicks move roving focus too", async () => {
    render(AgentChanges, {
      props: {
        repoId: "r",
        status: status([
          entry("a.ts", "unmodified", "modified"),
          entry("b.ts", "unmodified", "modified"),
        ]),
      },
    });
    await tick();

    fireEvent.click(renderedRows()[1]!);
    await tick();
    expect(renderedRows().map((row) => row.getAttribute("tabindex"))).toEqual(["-1", "0"]);
  });
});

describe("AgentChanges failed mutations", () => {
  it("a failed stage fires the visible error toast", async () => {
    render(AgentChanges, {
      props: {
        repoId: "r",
        status: status([entry("a.ts", "unmodified", "modified")]),
      },
    });
    await tick();

    fireEvent.click(screen.getByRole("button", { name: "Stage a.ts" }));
    await tick();
    await tick();

    expect(toastSpy).toHaveBeenCalledTimes(1);
    const [message, opts] = toastSpy.mock.calls[0]!;
    expect(String(message)).toContain("Stage failed");
    expect(String(message)).toContain("index locked");
    expect(opts).toMatchObject({ kind: "error" });
  });
});

describe("AgentChanges virtualization", () => {
  it("large groups render a window with spacers, not a truncation row", async () => {
    const many = Array.from({ length: 300 }, (_, i) =>
      entry(`f${String(i).padStart(3, "0")}.ts`, "unmodified", "modified"),
    );
    const { container } = render(AgentChanges, {
      props: { repoId: "r", status: status(many) },
    });
    await tick();

    // The 200-row cap is gone: no "+N more" overflow row.
    expect(container.textContent).not.toContain("more");
    // Only the viewport window is mounted; spacers hold the rest's height
    // (jsdom has no layout — the inline style carries the math).
    const rows = renderedRows();
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.length).toBeLessThan(300);
    const spacers = container.querySelectorAll<HTMLElement>(".row-spacer");
    expect(spacers.length).toBeGreaterThan(0);
    const spacerRows = [...spacers].reduce((sum, spacer) => {
      return sum + parseFloat(spacer.style.height) / 24;
    }, 0);
    expect(rows.length + spacerRows).toBe(300);
  });
});
