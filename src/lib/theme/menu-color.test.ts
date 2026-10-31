/**
 * Tests for the menu color engine (src/lib/theme/menu-color).
 *
 * startMenuColor() keeps every `.cn-menu-target` element in sync with the
 * appearance config's menuColor: inverted adds `.dark`, translucent adds
 * `.cn-menu-translucent`.
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { appearance } from "./appearance.svelte";
import { startMenuColor, TARGET_SELECTOR_FOR_TESTS } from "./menu-color";

function mountTarget(id: string): HTMLElement {
  const el = document.createElement("div");
  el.className = "cn-menu-target";
  el.dataset.testid = id;
  document.body.appendChild(el);
  return el;
}

let stop: () => void = () => {};

describe("menu color", () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.body.textContent = "";
    appearance.init();
  });

  afterEach(() => stop());

  it("leaves targets untouched for the default menu color", () => {
    appearance.set({ menuColor: "default" });
    stop = startMenuColor();
    const el = mountTarget("a");
    // Sync runs synchronously at start; new targets sync via the observer.
    expect(el.classList.contains("dark")).toBe(false);
    expect(el.classList.contains("cn-menu-translucent")).toBe(false);
  });

  it("marks targets dark for inverted and translucent for variants", () => {
    appearance.set({ menuColor: "inverted" });
    const inverted = mountTarget("b");
    stop = startMenuColor(); // syncs existing targets at start
    expect(inverted.classList.contains("dark")).toBe(true);
    expect(inverted.classList.contains("cn-menu-translucent")).toBe(false);

    appearance.set({ menuColor: "inverted-translucent" });
    expect(inverted.classList.contains("dark")).toBe(true);
    expect(inverted.classList.contains("cn-menu-translucent")).toBe(true);

    appearance.set({ menuColor: "default-translucent" });
    expect(inverted.classList.contains("dark")).toBe(false);
    expect(inverted.classList.contains("cn-menu-translucent")).toBe(true);
  });

  it("syncs targets mounted after start via the mutation observer", async () => {
    appearance.set({ menuColor: "inverted" });
    stop = startMenuColor();
    const late = mountTarget("c");
    // MutationObserver callbacks are async (rAF-throttled here).
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(late.classList.contains("dark")).toBe(true);
  });

  it("exports a selector that finds all targets", () => {
    mountTarget("d");
    mountTarget("e");
    expect(document.querySelectorAll(TARGET_SELECTOR_FOR_TESTS)).toHaveLength(2);
  });
});
