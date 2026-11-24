/**
 * TabGroup middle-click: panel tabs (and the compact head used by the
 * activity-bar sidebar) hide the panel through the same `hide` action as
 * the context menu, mirroring the repo/file tab strips. Other buttons and
 * left clicks must not.
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createRawSnippet } from "svelte";
import type { TabsNode } from "$lib/layout/layoutModel";
import { setInFlightDrag } from "$lib/layout/panelDnd";
import TabGroup from "./TabGroup.svelte";

afterEach(() => {
  cleanup();
  setInFlightDrag(null);
});

const node: TabsNode = {
  kind: "tabs",
  id: "t1",
  tabs: ["status", "history"],
  active: 0,
};

function groupProps(overrides: Record<string, unknown> = {}) {
  return {
    node,
    renderPanel: () => createRawSnippet(() => ({ render: () => "<div></div>" })),
    hiddenPanels: [],
    canMoveLeft: false,
    canMoveRight: false,
    onActivate: vi.fn(),
    onAdd: vi.fn(),
    onAction: vi.fn(),
    onMovePanel: vi.fn(),
    ...overrides,
  };
}

function auxClick(target: Element, button = 1): Promise<unknown> {
  return fireEvent(target, new MouseEvent("auxclick", { bubbles: true, button }));
}

function createDragEvent(type: string, props: Record<string, unknown> = {}): DragEvent {
  const data: Record<string, string> = {};
  const dataTransfer = {
    setData: vi.fn((format: string, val: string) => {
      data[format] = val;
    }),
    getData: vi.fn((format: string) => data[format] || ""),
    effectAllowed: "none",
    dropEffect: "none",
  };
  const event = new Event(type, { bubbles: true, cancelable: true }) as any;
  event.dataTransfer = dataTransfer;
  Object.assign(event, props);
  return event as DragEvent;
}

describe("TabGroup middle-click", () => {
  it("hides the panel whose tab is middle-clicked", async () => {
    const onAction = vi.fn();
    render(TabGroup, groupProps({ onAction }));

    await auxClick(screen.getByRole("tab", { name: "Status" }));

    expect(onAction).toHaveBeenCalledWith("hide", "t1", "status");
  });

  it("keeps left clicks on tabs activating, not hiding", async () => {
    const onAction = vi.fn();
    const onActivate = vi.fn();
    render(TabGroup, groupProps({ onAction, onActivate }));

    const tab = screen.getByRole("tab", { name: "History" });
    await fireEvent.click(tab);
    await auxClick(tab, 2);

    expect(onActivate).toHaveBeenCalledWith("t1", 1);
    expect(onAction).not.toHaveBeenCalled();
  });

  it("hides the active panel from the compact panel head", async () => {
    const onAction = vi.fn();
    render(TabGroup, groupProps({ onAction, hideTabs: true }));

    const head = document.querySelector(".panel-head");
    expect(head).not.toBeNull();
    await auxClick(head!);

    expect(onAction).toHaveBeenCalledWith("hide", "t1", "status");
  });
});

describe("TabGroup drag and drop", () => {
  it("renders tabs with draggable attribute", () => {
    render(TabGroup, groupProps());
    const tab = screen.getByRole("tab", { name: "Status" });
    expect(tab.getAttribute("draggable")).toBe("true");
  });

  it("reorders tabs within the same group on drag and drop", async () => {
    const onMovePanel = vi.fn();
    render(TabGroup, groupProps({ onMovePanel }));

    const statusTab = screen.getByRole("tab", { name: "Status" });
    const historyTab = screen.getByRole("tab", { name: "History" });
    const historyWrapper = historyTab.closest(".relative")!;

    vi.spyOn(historyWrapper, "getBoundingClientRect").mockReturnValue({
      left: 100,
      width: 100,
      right: 200,
      top: 0,
      bottom: 40,
      height: 40,
    } as DOMRect);

    // Start dragging "status"
    const dragStartEvent = createDragEvent("dragstart");
    await fireEvent(statusTab, dragStartEvent);

    // Tab is dimmed with opacity-40
    expect(statusTab.className).toContain("opacity-40");

    // Drag over "history" towards the right half (after)
    const dragOverEvent = createDragEvent("dragover", { clientX: 180 });
    await fireEvent(historyWrapper, dragOverEvent);

    // Drop on history
    const dropEvent = createDragEvent("drop", { clientX: 180 });
    await fireEvent(historyWrapper, dropEvent);

    // "status" moved after "history" (target index 1 in the remaining list)
    expect(onMovePanel).toHaveBeenCalledWith("status", {
      kind: "tab",
      nodeId: "t1",
      index: 1,
    });
  });

  it("moves tabs from another group into this group on drop", async () => {
    const onMovePanel = vi.fn();
    render(TabGroup, groupProps({ onMovePanel }));

    const historyTab = screen.getByRole("tab", { name: "History" });
    const historyWrapper = historyTab.closest(".relative")!;

    vi.spyOn(historyWrapper, "getBoundingClientRect").mockReturnValue({
      left: 100,
      width: 100,
      right: 200,
      top: 0,
      bottom: 40,
      height: 40,
    } as DOMRect);

    // Simulate tab dragged from group "t2"
    setInFlightDrag({ panel: "branches", sourceGroupId: "t2" });

    // Drag over "history" before midpoint
    const dragOverEvent = createDragEvent("dragover", { clientX: 120 });
    await fireEvent(historyWrapper, dragOverEvent);

    // Drop on history before it (index 1)
    const dropEvent = createDragEvent("drop", { clientX: 120 });
    await fireEvent(historyWrapper, dropEvent);

    expect(onMovePanel).toHaveBeenCalledWith("branches", {
      kind: "tab",
      nodeId: "t1",
      index: 1,
    });
  });

  it("splits a new group when dropped on the left or right of panel body", async () => {
    const onMovePanel = vi.fn();
    render(TabGroup, groupProps({ onMovePanel }));

    const panelBody = document.querySelector(".panel-body")!;
    expect(panelBody).not.toBeNull();

    vi.spyOn(panelBody, "getBoundingClientRect").mockReturnValue({
      left: 0,
      width: 400,
      right: 400,
      top: 40,
      bottom: 400,
      height: 360,
    } as DOMRect);

    setInFlightDrag({ panel: "history", sourceGroupId: "t1" });

    // Drag over left edge (clientX = 50 < 400 * 0.25 = 100)
    await fireEvent(panelBody, createDragEvent("dragover", { clientX: 50 }));
    // Drop on left edge
    await fireEvent(panelBody, createDragEvent("drop", { clientX: 50 }));

    expect(onMovePanel).toHaveBeenCalledWith("history", {
      kind: "before",
      nodeId: "t1",
      asTabs: true,
    });

    // Drag over right edge (clientX = 350 > 400 * 0.75 = 300)
    setInFlightDrag({ panel: "status", sourceGroupId: "t1" });
    await fireEvent(panelBody, createDragEvent("dragover", { clientX: 350 }));
    await fireEvent(panelBody, createDragEvent("drop", { clientX: 350 }));

    expect(onMovePanel).toHaveBeenCalledWith("status", {
      kind: "after",
      nodeId: "t1",
      asTabs: true,
    });
  });
});
