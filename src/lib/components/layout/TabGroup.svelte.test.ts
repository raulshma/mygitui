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
import TabGroup from "./TabGroup.svelte";

afterEach(cleanup);

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
    ...overrides,
  };
}

function auxClick(target: Element, button = 1): Promise<unknown> {
  return fireEvent(target, new MouseEvent("auxclick", { bubbles: true, button }));
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
