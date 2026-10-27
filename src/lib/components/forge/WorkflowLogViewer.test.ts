/**
 * Component tests for the Actions log viewer: the render window follows the
 * tail on large logs, search resets to the first result, and an empty payload
 * shows the caller's empty text instead of a bogus "no matches" state.
 *
 * The fixture is intentionally rendered with a small `windowSize`; jsdom is
 * far slower than a browser at inserting thousands of rows, and the window
 * math is identical at any size.
 */

import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import WorkflowLogViewer from "./WorkflowLogViewer.svelte";

afterEach(() => cleanup());

const WINDOW = 100;

function logOf(count: number): string {
  return Array.from(
    { length: count },
    (_, index) => `build\tBuild\talpha-${index + 1}`,
  ).join("\n");
}

/** Message text of each rendered line, in document order. */
function renderedLines(container: HTMLElement): string[] {
  return [...container.querySelectorAll(".log-line .lm")].map(
    (line) => line.textContent ?? "",
  );
}

describe("WorkflowLogViewer", () => {
  it("renders the tail of a large log instead of the first window", async () => {
    const { container } = render(WorkflowLogViewer, {
      props: { text: logOf(250), windowSize: WINDOW },
    });
    await tick();

    const lines = renderedLines(container);
    expect(lines).toHaveLength(WINDOW);
    expect(lines[0]).toBe("alpha-151");
    expect(lines[WINDOW - 1]).toBe("alpha-250");
    expect(container.querySelector(".log-more")?.textContent).toContain(
      "Showing lines 151–250 of 250",
    );
  });

  it("resets to the first search result", async () => {
    const { container } = render(WorkflowLogViewer, {
      props: { text: logOf(250), windowSize: WINDOW },
    });
    await tick();
    expect(renderedLines(container)[0]).toBe("alpha-151");

    await fireEvent.input(screen.getByRole("searchbox", { name: "Search logs" }), {
      target: { value: "alpha" },
    });
    await tick();

    const lines = renderedLines(container);
    expect(lines).toHaveLength(WINDOW);
    expect(lines[0]).toBe("alpha-1");
    expect(lines[WINDOW - 1]).toBe("alpha-100");
    expect(container.querySelector(".log-count")?.textContent).toContain("250 hits");
  });

  it("shows emptyText for an empty successful payload", () => {
    render(WorkflowLogViewer, {
      props: { text: "", emptyText: "No output for this job." },
    });

    expect(screen.getByText("No output for this job.")).toBeTruthy();
    expect(screen.queryByText(/No log lines match/)).toBeNull();
  });

  it("renders individual steps as collapsible sections and allows toggling them", async () => {
    const raw = [
      "build\tSet up job\talpha",
      "build\tCheckout\tbeta",
    ].join("\n");
    const { container } = render(WorkflowLogViewer, {
      props: { text: raw, windowSize: WINDOW },
    });
    await tick();

    const stepHeads = container.querySelectorAll(".log-step-head");
    expect(stepHeads).toHaveLength(2);
    expect(stepHeads[0]?.textContent).toContain("Set up job");
    expect(stepHeads[1]?.textContent).toContain("Checkout");

    await fireEvent.click(stepHeads[0]!);
    await tick();
    expect(stepHeads[0]?.getAttribute("aria-expanded")).toBe("false");

    const stepLines = container.querySelectorAll(".log-line .lm");
    expect([...stepLines].map((l) => l.textContent)).toEqual(["beta"]);

    await fireEvent.click(stepHeads[0]!);
    await tick();
    expect(stepHeads[0]?.getAttribute("aria-expanded")).toBe("true");
    const reExpandedLines = container.querySelectorAll(".log-line .lm");
    expect([...reExpandedLines].map((l) => l.textContent)).toEqual(["alpha", "beta"]);
  });
});
