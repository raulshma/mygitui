/**
 * Tests for the appearance store (src/lib/theme/appearance.svelte).
 *
 * The store normalizes every mutation, persists to localStorage, applies
 * tokens through one managed <style> element, selects the style sheet via a
 * `style-<name>` root class, and round-trips preset codes.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  APPEARANCE_EVENT,
  APPEARANCE_STORAGE_KEY,
  appearance,
  buildCss,
  buildFontCss,
} from "./appearance.svelte";
import { DEFAULT_APPEARANCE, encodeAppearance } from "./preset/codec";
import { getBaseColor } from "./preset/values";

function resetDom() {
  window.localStorage.clear();
  document.documentElement.className = "";
  document.getElementById("mygitui-appearance")?.remove();
  document.getElementById("mygitui-appearance-fonts")?.remove();
}

describe("appearance store", () => {
  beforeEach(() => {
    resetDom();
    // Fresh module state per test: init() re-reads storage.
    appearance.init();
  });

  it("init() injects no CSS for the default config, but selects the style", () => {
    // The defaults are exactly what app.css ships — a token element would
    // only duplicate work. The style class still selects nova's sheet.
    expect(document.getElementById("mygitui-appearance")).toBeNull();
    expect(document.documentElement.classList.contains("style-nova")).toBe(true);
  });

  it("set() writes the token element; reset() drops it again", () => {
    appearance.set({ baseColor: "taupe" });
    const style = document.getElementById("mygitui-appearance");
    expect(style).not.toBeNull();
    expect(style!.textContent).toContain("--background:");
    expect(style!.textContent).toContain(".dark");

    appearance.reset();
    expect(document.getElementById("mygitui-appearance")).toBeNull();
    expect(document.documentElement.classList.contains("style-nova")).toBe(true);
  });

  it("set() normalizes, persists, and applies live", () => {
    const effective = appearance.set({ baseColor: "taupe", theme: "violet" });
    expect(effective.baseColor).toBe("taupe");
    expect(effective.theme).toBe("violet");

    expect(JSON.parse(window.localStorage.getItem(APPEARANCE_STORAGE_KEY)!)).toMatchObject({
      baseColor: "taupe",
    });

    const css = document.getElementById("mygitui-appearance")!.textContent!;
    const taupe = getBaseColor("taupe")!;
    expect(css).toContain(`--background: ${taupe.cssVars.light.background}`);
  });

  it("coerces invalid fields through normalization", () => {
    const effective = appearance.set({ radius: "nonsense" as never, style: "lyra" });
    // lyra forces square corners; an unknown radius falls back to default.
    expect(effective.radius).toBe("none");
    expect(effective.style).toBe("lyra");
  });

  it("swaps the style class when the style changes", async () => {
    appearance.set({ style: "vega" });
    const root = document.documentElement;
    // Non-default sheets load on demand; the class lands with the chunk.
    // The first compile of a sheet is slow (Tailwind), hence the timeout.
    await vi.waitFor(
      () => {
        expect(root.classList.contains("style-vega")).toBe(true);
      },
      { timeout: 15_000 },
    );
    expect(root.classList.contains("style-nova")).toBe(false);
    expect(root.classList.toString().match(/style-/g)).toHaveLength(1);
  });

  it("writes font vars once the face lands, and drops them on reset", async () => {
    appearance.set({ font: "lora" });
    await vi.waitFor(
      () => {
        // The element appears only when the chunk is in the document — the
        // previous family keeps rendering until then (single-reflow swap).
        expect(document.getElementById("mygitui-appearance-fonts")?.textContent).toContain(
          "--font-sans: 'Lora Variable', serif",
        );
      },
      { timeout: 15_000 },
    );

    appearance.reset();
    expect(document.getElementById("mygitui-appearance-fonts")).toBeNull();
  });

  it("round-trips preset codes and rejects junk", () => {
    expect(appearance.applyPresetCode(encodeAppearance({ theme: "amber" }))).toBe(true);
    expect(appearance.config.theme).toBe("amber");

    expect(appearance.applyPresetCode("not-a-code")).toBe(false);
    expect(appearance.config.theme).toBe("amber");
  });

  it("reset() restores the defaults and persists", () => {
    appearance.set({ menuAccent: "bold", font: "lora" });
    appearance.reset();
    expect(appearance.config).toEqual(DEFAULT_APPEARANCE);
    expect(JSON.parse(window.localStorage.getItem(APPEARANCE_STORAGE_KEY)!)).toEqual(
      DEFAULT_APPEARANCE,
    );
  });

  it("init() restores a persisted config", () => {
    appearance.set({ chartColor: "rose", menuColor: "inverted" });
    // Simulate a reload: fresh DOM, storage intact.
    resetDom();
    appearance.init();
    expect(appearance.config.chartColor).toBe("rose");
    expect(appearance.config.menuColor).toBe("inverted");
  });

  it("announces changes on the typed window event", () => {
    let seen: unknown = null;
    const listener = (event: Event) => (seen = (event as CustomEvent).detail);
    window.addEventListener(APPEARANCE_EVENT, listener);
    try {
      appearance.set({ radius: "small" });
      expect(seen).toMatchObject({ radius: "small" });
    } finally {
      window.removeEventListener(APPEARANCE_EVENT, listener);
    }
  });
});

describe("buildCss / buildFontCss", () => {
  it("buildCss writes only token blocks (font vars live in their own element)", () => {
    const css = buildCss(DEFAULT_APPEARANCE);
    expect(css).toMatch(/^:root \{/);
    expect(css).toContain(".dark {");
    expect(css).not.toContain("--font-sans");
  });

  it("buildFontCss returns null for the defaults", () => {
    expect(buildFontCss(DEFAULT_APPEARANCE)).toBeNull();
  });

  it("buildFontCss drives the heading font and falls back to the body font", () => {
    const heading = buildFontCss({ ...DEFAULT_APPEARANCE, fontHeading: "lora" });
    expect(heading).toContain("--font-sans: 'Inter Variable', sans-serif");
    expect(heading).toContain("--font-heading: 'Lora Variable', serif");

    const body = buildFontCss({ ...DEFAULT_APPEARANCE, font: "lora" });
    expect(body).toContain("--font-sans: 'Lora Variable', serif");
    expect(body).toContain("--font-heading: 'Lora Variable', serif");
  });
});
