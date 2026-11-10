/**
 * Tests for the overlay stack owner (`$lib/state/overlays.svelte`):
 * layer-based "top" resolution, idempotent open, and close semantics.
 */
import { describe, expect, it } from "vitest";
import { OverlayStack } from "./overlays.svelte";

describe("OverlayStack", () => {
  it("top is null when nothing is open", () => {
    const stack = new OverlayStack();
    expect(stack.top).toBeNull();
    expect(stack.isTop("palette")).toBe(false);
  });

  it("the highest layer wins regardless of registration order", () => {
    const stack = new OverlayStack();
    // Palette opens first, then a context menu floats above it.
    stack.open("palette", 30);
    stack.open("context-menu", 40);
    expect(stack.top).toBe("context-menu");
    expect(stack.isTop("palette")).toBe(false);
    // Closing the menu hands the top back to the palette.
    stack.close("context-menu");
    expect(stack.top).toBe("palette");
  });

  it("covers a lower overlay opened later (panel under palette)", () => {
    const stack = new OverlayStack();
    stack.open("palette", 30);
    stack.open("conflict-editor", 10); // mounts under the palette scrim
    expect(stack.top).toBe("palette");
    stack.close("palette");
    expect(stack.top).toBe("conflict-editor");
  });

  it("re-opening re-layers an id", () => {
    const stack = new OverlayStack();
    stack.open("x", 10);
    stack.open("x", 50);
    stack.open("y", 30);
    expect(stack.top).toBe("x");
  });

  it("open is idempotent and close is a no-op for unknown ids", () => {
    const stack = new OverlayStack();
    stack.open("a", 30);
    stack.open("a", 30);
    expect(stack.isOpen("a")).toBe(true);
    stack.close("nope");
    expect(stack.isOpen("a")).toBe(true);
    stack.close("a");
    expect(stack.isOpen("a")).toBe(false);
  });

  it("the most recently opened overlay wins within one layer", () => {
    const stack = new OverlayStack();
    stack.open("first", 30);
    stack.open("second", 30);
    expect(stack.top).toBe("second");
    stack.close("second");
    expect(stack.top).toBe("first");
  });
});
