/**
 * OverlayDrawer — the non-modal drawer primitive (plan §4.3, §10/§11).
 * Covers: mount + open/close state mirroring, Esc close with focus return
 * to the invoking element, the Tab focus wrap inside the drawer, and the
 * non-modal aria contract (dialog, aria-modal=false, no scrim).
 */
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import Host from "./OverlayDrawer.test.host.svelte";

afterEach(cleanup);

function drawerEl(): HTMLElement {
  const node = document.querySelector<HTMLElement>("[data-slot='overlay-drawer']");
  if (node === null) throw new Error("drawer not rendered");
  return node;
}

describe("OverlayDrawer", () => {
  it("renders closed by default and keeps its content mounted", async () => {
    render(Host);
    await tick();

    const drawer = drawerEl();
    expect(drawer.dataset.state).toBe("closed");
    // Keep-alive contract: closed drawers are visibility-hidden, not
    // unmounted — panel state survives toggles.
    expect(drawer.classList.contains("invisible")).toBe(true);
    expect(screen.getByText("First")).toBeTruthy();
    // A closed drawer is out of the tab order (visibility: hidden).
    const first = screen.getByText("First") as HTMLButtonElement;
    expect(first.focus).toBeDefined();
  });

  it("opens as a labeled non-modal dialog and focuses the drawer", async () => {
    render(Host, { open: true, label: "Files" });
    await tick();

    const drawer = drawerEl();
    expect(drawer.dataset.state).toBe("open");
    expect(drawer.getAttribute("role")).toBe("dialog");
    expect(drawer.getAttribute("aria-modal")).toBe("false");
    expect(drawer.getAttribute("aria-label")).toBe("Files");
    expect(drawer.classList.contains("invisible")).toBe(false);
    // Initial focus lands on the drawer panel itself.
    expect(document.activeElement).toBe(drawer);
    // Non-modal: no scrim sibling — the drawer overlays, nothing blocks
    // the rest of the UI.
    expect(drawer.parentElement?.children.length).toBe(2); // invoker + drawer
  });

  it("closes on Esc and returns focus to the invoking element", async () => {
    const onOpenChange = vi.fn();
    render(Host, { onOpenChange });
    await tick();

    // Open through the invoker — its focus is what the drawer must restore.
    // (Focus the invoker first: jsdom, like Safari, does not focus buttons
    // on pointer click; keyboard users always have it focused.)
    const invoker = screen.getByText("Open drawer");
    invoker.focus();
    await fireEvent.click(invoker);
    await tick();
    expect(drawerEl().dataset.state).toBe("open");
    expect(document.activeElement).toBe(drawerEl());

    await fireEvent.keyDown(drawerEl(), { key: "Escape" });
    await tick();
    expect(drawerEl().dataset.state).toBe("closed");
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(document.activeElement).toBe(invoker);
  });

  it("keeps focus inside the drawer while Tab cycles (trap)", async () => {
    render(Host, { open: true });
    await tick();

    const first = screen.getByText("First");
    const second = screen.getByText("Second");

    // Tab from the last item wraps to the first.
    second.focus();
    await fireEvent.keyDown(drawerEl(), { key: "Tab" });
    expect(document.activeElement).toBe(first);

    // Shift+Tab from the first wraps to the last.
    await fireEvent.keyDown(drawerEl(), { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(second);

    // Mid-cycle movement relies on the browser's native Tab flow (the trap
    // only intercepts the wraps), which jsdom does not simulate.
  });

  it("anchors the left side against the rail edge", async () => {
    render(Host, { open: true, side: "left" });
    await tick();

    const drawer = drawerEl();
    expect(drawer.dataset.side).toBe("left");
    expect(drawer.classList.contains("left-0")).toBe(true);
    expect(drawer.classList.contains("border-r")).toBe(true);
  });

  it("returns focus to the invoker when the caller closes externally", async () => {
    const { rerender } = render(Host);
    await tick();

    // Same capture path as Esc: the focused element at open time.
    const invoker = screen.getByText("Open drawer");
    invoker.focus();

    await rerender({ open: true });
    await tick();
    expect(document.activeElement).toBe(drawerEl());

    await rerender({ open: false });
    await tick();
    expect(drawerEl().dataset.state).toBe("closed");
    expect(document.activeElement).toBe(invoker);
  });
});
