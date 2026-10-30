/**
 * ActivityBar rail pins: pinned panels render as real rail buttons after
 * the defaults, leave the "More panels" overflow menu, gain a pin action
 * there (via the shared context menu), and unpin through the pinned
 * button's right-click menu.
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PanelId } from "$lib/layout/layoutModel";
import { contextMenu } from "$lib/components/menu/contextMenuStore.svelte";
import ContextMenu from "$lib/components/menu/ContextMenu.svelte";
import { railPins } from "$lib/stores/railPins.svelte";
import ActivityBar from "./ActivityBar.svelte";

const PANELS: PanelId[] = [
  "files",
  "status",
  "branches",
  "remotes",
  "terminal",
  "stashes",
  "reflog",
];

function baseProps() {
  return {
    panels: PANELS,
    pinned: railPins.pins,
    active: "status" as PanelId | null,
    onSelect: vi.fn(),
    onHistory: vi.fn(),
    onToggleCollapse: vi.fn(),
    onTogglePin: (panel: PanelId) => railPins.toggle(panel),
  };
}

afterEach(() => {
  cleanup();
  contextMenu.hide();
  railPins.clear();
});

function railButtons(): string[] {
  return [
    ...document.querySelectorAll<HTMLButtonElement>("[data-activity-panel]"),
  ]
    .filter((btn) => btn.dataset.activityPanel !== "history")
    .map((btn) => btn.dataset.activityPanel!);
}

describe("ActivityBar rail pins", () => {
  it("renders pinned panels as rail buttons after the defaults", () => {
    railPins.pin("stashes");
    render(ActivityBar, baseProps());
    render(ContextMenu);
    expect(railButtons()).toEqual([
      "files",
      "status",
      "branches",
      "remotes",
      "terminal",
      "stashes",
    ]);
  });

  it("pinned panels leave the overflow menu", async () => {
    railPins.pin("stashes");
    render(ActivityBar, baseProps());

    await fireEvent.click(screen.getByTitle("More panels"));
    const labels = contextMenu.entries.map((entry) =>
      "label" in entry ? entry.label : "",
    );
    expect(labels).not.toContain("Stashes");
    expect(labels).toContain("Reflog");
  });

  it("pins from the overflow menu's pin action", async () => {
    const { rerender } = render(ActivityBar, baseProps());
    render(ContextMenu);

    await fireEvent.click(screen.getByTitle("More panels"));
    // Entries render in `panels` order — stashes is the first pin action.
    const pin = screen.getAllByTitle("Pin to rail")[0]!;
    await fireEvent.click(pin);
    expect(railPins.pins).toEqual(["stashes"]);
    expect(contextMenu.open).toBe(false);

    // Prop reactivity: RepoView re-passes the store's pins on change.
    await rerender(baseProps());
    expect(railButtons()).toContain("stashes");
  });

  it("unpins via the pinned button's right-click menu", async () => {
    railPins.pin("reflog");
    const { rerender } = render(ActivityBar, baseProps());
    render(ContextMenu);

    const button = document.querySelector<HTMLButtonElement>(
      '[data-activity-panel="reflog"]',
    );
    expect(button).not.toBeNull();
    await fireEvent.contextMenu(button!);
    expect(contextMenu.open).toBe(true);
    await fireEvent.click(screen.getByText("Unpin from rail"));
    expect(railPins.pins).toEqual([]);

    await rerender(baseProps());
    expect(railButtons()).not.toContain("reflog");
  });

  it("right-click on a default button opens no unpin menu", () => {
    render(ActivityBar, baseProps());
    render(ContextMenu);

    const button = document.querySelector<HTMLButtonElement>(
      '[data-activity-panel="status"]',
    );
    fireEvent.contextMenu(button!);
    expect(contextMenu.open).toBe(false);
  });

  it("omits pin affordances without onTogglePin", async () => {
    render(ActivityBar, { ...baseProps(), onTogglePin: null });
    render(ContextMenu);

    await fireEvent.click(screen.getByTitle("More panels"));
    expect(
      contextMenu.entries.some(
        (entry) => !("separator" in entry) && entry.action !== undefined,
      ),
    ).toBe(false);
  });
});
