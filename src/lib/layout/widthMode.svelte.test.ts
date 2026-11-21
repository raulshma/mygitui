/**
 * Unit tests for the width-mode store (`$lib/layout/widthMode.svelte`):
 * pure threshold resolution, the injected-env lifecycle (initial resolve,
 * resize re-evaluation, change-guarded reflection, unsubscribe), and the
 * default env's wiring against jsdom (`window.innerWidth` + `resize` +
 * `<html data-width-mode>`).
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  resolveWidthMode,
  startWidthMode,
  stopWidthMode,
  WIDTH_MODE_ATTR,
  WIDTH_MODES,
  WIDTH_NARROW_MIN,
  WIDTH_WIDE_MIN,
  widthMode,
  type WidthMode,
} from "./widthMode.svelte";

afterEach(() => {
  stopWidthMode();
  delete document.documentElement.dataset.widthMode;
  vi.restoreAllMocks();
});

describe("resolveWidthMode thresholds", () => {
  it("maps the §4.2 ladder: Wide ≥1280, Medium 1024–1279, Narrow 800–1023, Stacked <800", () => {
    expect(resolveWidthMode(WIDTH_WIDE_MIN)).toBe("wide");
    expect(resolveWidthMode(WIDTH_WIDE_MIN - 1)).toBe("medium");
    expect(resolveWidthMode(1024)).toBe("medium");
    expect(resolveWidthMode(1023)).toBe("narrow");
    expect(resolveWidthMode(WIDTH_NARROW_MIN)).toBe("narrow");
    expect(resolveWidthMode(WIDTH_NARROW_MIN - 1)).toBe("stacked");
    expect(resolveWidthMode(0)).toBe("stacked");
  });

  it("fails open to wide on non-finite widths (a broken read hides nothing)", () => {
    expect(resolveWidthMode(Number.NaN)).toBe("wide");
    expect(resolveWidthMode(Number.POSITIVE_INFINITY)).toBe("wide");
  });
});

describe("startWidthMode with an injected env", () => {
  /** Minimal env fake: programmatic width + captured listeners. */
  function fakeEnv(initialWidth: number | null) {
    const listeners = new Set<() => void>();
    const reflected: WidthMode[] = [];
    let width = initialWidth;
    return {
      reflected,
      listeners,
      setWidth(next: number | null): void {
        width = next;
      },
      fireResize(): void {
        for (const listener of [...listeners]) listener();
      },
      env: {
        readWidth: () => width,
        onResize: (handler: () => void) => {
          listeners.add(handler);
          return () => listeners.delete(handler);
        },
        reflect: (mode: WidthMode) => void reflected.push(mode),
      },
    };
  }

  it("resolves + reflects the width immediately and exposes it reactively", () => {
    const fake = fakeEnv(1100);
    expect(widthMode.current).toBe("wide"); // pre-start default
    startWidthMode(fake.env);
    expect(widthMode.current).toBe("medium");
    expect(fake.reflected).toEqual(["medium"]);
  });

  it("re-resolves on resize and skips unknown widths", () => {
    const fake = fakeEnv(1440);
    startWidthMode(fake.env);
    expect(widthMode.current).toBe("wide");

    fake.setWidth(799);
    fake.fireResize();
    expect(widthMode.current).toBe("stacked");
    expect(fake.reflected).toEqual(["wide", "stacked"]);

    // A failed read (null) keeps the current mode and does not reflect.
    fake.setWidth(null);
    fake.fireResize();
    expect(widthMode.current).toBe("stacked");
    expect(fake.reflected).toEqual(["wide", "stacked"]);
  });

  it("change-guards the reflection across a resize storm", () => {
    const fake = fakeEnv(900);
    startWidthMode(fake.env);
    for (let i = 0; i < 5; i++) {
      fake.setWidth(WIDTH_NARROW_MIN + i); // stays within Narrow
      fake.fireResize();
    }
    expect(widthMode.current).toBe("narrow");
    expect(fake.reflected).toEqual(["narrow"]); // one write, not five
  });

  it("stop unsubscribes from resize (no further updates)", () => {
    const fake = fakeEnv(1440);
    const stop = startWidthMode(fake.env);
    expect(fake.listeners.size).toBe(1);
    stop();
    expect(fake.listeners.size).toBe(0);
    fake.setWidth(700);
    fake.fireResize();
    expect(widthMode.current).toBe("wide");
  });

  it("is idempotent: a running instance is stopped before restart", () => {
    const first = fakeEnv(1440);
    const second = fakeEnv(600);
    startWidthMode(first.env);
    startWidthMode(second.env);
    expect(first.listeners.size).toBe(0);
    expect(second.listeners.size).toBe(1);
    expect(widthMode.current).toBe("stacked");
    expect(second.reflected).toEqual(["stacked"]);
  });
});

describe("default env wiring (jsdom)", () => {
  it("mirrors window.innerWidth onto <html data-width-mode> and tracks resizes", () => {
    window.innerWidth = 1000;
    startWidthMode();
    expect(document.documentElement.getAttribute(WIDTH_MODE_ATTR)).toBe(
      "narrow",
    );
    expect(widthMode.current).toBe("narrow");

    window.innerWidth = 1280;
    window.dispatchEvent(new Event("resize"));
    expect(document.documentElement.dataset.widthMode).toBe("wide");

    window.innerWidth = 500;
    window.dispatchEvent(new Event("resize"));
    expect(document.documentElement.dataset.widthMode).toBe("stacked");
  });

  it("stops tracking after stopWidthMode", () => {
    window.innerWidth = 1440;
    startWidthMode();
    stopWidthMode();
    window.innerWidth = 700;
    window.dispatchEvent(new Event("resize"));
    expect(document.documentElement.dataset.widthMode).toBe("wide");
  });
});

describe("WIDTH_MODES ladder", () => {
  it("lists the four modes roomiest first (persistence iteration order)", () => {
    expect([...WIDTH_MODES]).toEqual(["wide", "medium", "narrow", "stacked"]);
  });
});
