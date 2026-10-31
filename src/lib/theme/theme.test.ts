/**
 * Tests for the shadcn theme foundation (src/lib/theme).
 *
 * The theme is class-based: the applied scheme toggles `.dark` on <html>
 * (plus the legacy `data-theme` stamp and `color-scheme`), follows
 * `prefers-color-scheme` live, and honors a persisted manual override.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  initTheme,
  disposeTheme,
  currentScheme,
  getThemePreference,
  setThemePreference,
  THEME_PREFERENCE_EVENT,
  THEME_SCHEME_EVENT,
  THEME_STORAGE_KEY,
} from "./index";

type ChangeListener = (event: { matches: boolean }) => void;

/**
 * jsdom has no matchMedia; install a controllable stand-in whose `matches`
 * can be flipped, notifying registered "change" listeners synchronously.
 */
function installMatchMediaStub(initialDark: boolean) {
  const listeners = new Set<ChangeListener>();
  const query = {
    matches: initialDark,
    media: "(prefers-color-scheme: dark)",
    onchange: null,
    addEventListener: (type: string, listener: ChangeListener) => {
      if (type === "change") listeners.add(listener);
    },
    removeEventListener: (_type: string, listener: ChangeListener) => {
      listeners.delete(listener);
    },
    addListener: (listener: ChangeListener) => listeners.add(listener),
    removeListener: (listener: ChangeListener) => listeners.delete(listener),
    dispatchEvent: () => true,
  };
  vi.stubGlobal("matchMedia", vi.fn(() => query as unknown as MediaQueryList));
  return {
    setDark(dark: boolean) {
      query.matches = dark;
      for (const listener of [...listeners]) listener({ matches: dark });
    },
  };
}

const root = () => document.documentElement;

function resetRoot() {
  root().style.cssText = "";
  root().removeAttribute("data-theme");
  root().classList.remove("dark");
}

describe("initTheme", () => {
  beforeEach(() => {
    disposeTheme();
    resetRoot();
    localStorage.removeItem(THEME_STORAGE_KEY);
    setThemePreference("system");
    vi.unstubAllGlobals();
  });

  it("resolves without throwing and stamps the light scheme", async () => {
    installMatchMediaStub(false);

    await expect(initTheme()).resolves.toBeUndefined();

    expect(root().classList.contains("dark")).toBe(false);
    expect(root().dataset.theme).toBe("light");
    expect(root().style.getPropertyValue("color-scheme")).toBe("light");
    expect(currentScheme()).toBe("light");
  });

  it("toggles .dark (and the stamps) when the OS switches schemes and back", async () => {
    const media = installMatchMediaStub(false);
    await initTheme();

    expect(root().classList.contains("dark")).toBe(false);

    media.setDark(true);
    expect(root().classList.contains("dark")).toBe(true);
    expect(root().dataset.theme).toBe("dark");
    expect(root().style.getPropertyValue("color-scheme")).toBe("dark");
    expect(currentScheme()).toBe("dark");

    media.setDark(false);
    expect(root().classList.contains("dark")).toBe(false);
    expect(currentScheme()).toBe("light");
  });

  it("applies dark immediately when the OS is already dark", async () => {
    installMatchMediaStub(true);
    await initTheme();

    expect(root().classList.contains("dark")).toBe(true);
    expect(root().style.getPropertyValue("color-scheme")).toBe("dark");
  });

  it("never throws when matchMedia is unavailable", async () => {
    // jsdom ships no matchMedia at all; simply do not stub it.
    await expect(initTheme()).resolves.toBeUndefined();
    expect(root().dataset.theme).toBe("light");
    expect(currentScheme()).toBe("light");
  });

  it("never throws when matchMedia itself explodes", async () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => {
        throw new Error("matchMedia unavailable");
      }),
    );
    await expect(initTheme()).resolves.toBeUndefined();
    expect(currentScheme()).toBe("light");
  });

  it("re-initializes cleanly without stacking duplicate listeners", async () => {
    const media = installMatchMediaStub(false);
    await initTheme();
    await initTheme();

    media.setDark(true);
    expect(root().classList.contains("dark")).toBe(true);
    media.setDark(false);
    expect(root().classList.contains("dark")).toBe(false);
  });
});

describe("theme preference override (M7 I2)", () => {
  beforeEach(() => {
    disposeTheme();
    resetRoot();
    localStorage.removeItem(THEME_STORAGE_KEY);
    setThemePreference("system");
    vi.unstubAllGlobals();
  });

  it("defaults to system and applies the OS scheme", async () => {
    installMatchMediaStub(true);
    await initTheme();

    expect(getThemePreference()).toBe("system");
    expect(root().classList.contains("dark")).toBe(true);
    expect(currentScheme()).toBe("dark");
  });

  it("override pins light even when the OS is dark, and persists", async () => {
    installMatchMediaStub(true);
    await initTheme();

    setThemePreference("light");

    expect(root().classList.contains("dark")).toBe(false);
    expect(root().style.getPropertyValue("color-scheme")).toBe("light");
    expect(currentScheme()).toBe("light");
    expect(getThemePreference()).toBe("light");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("light");
  });

  it("override pins dark when the OS is light", async () => {
    installMatchMediaStub(false);
    await initTheme();

    setThemePreference("dark");

    expect(root().classList.contains("dark")).toBe(true);
    expect(root().style.getPropertyValue("color-scheme")).toBe("dark");
    expect(currentScheme()).toBe("dark");
  });

  it("ignores OS switches while overridden; system re-follows after reset", async () => {
    const media = installMatchMediaStub(false);
    await initTheme();

    setThemePreference("light");
    media.setDark(true);
    expect(root().classList.contains("dark")).toBe(false);
    expect(currentScheme()).toBe("light");

    setThemePreference("system");
    // The stored override is gone, so the current OS state applies…
    expect(root().classList.contains("dark")).toBe(true);
    expect(currentScheme()).toBe("dark");
    // …and the OS keeps driving live from here on.
    media.setDark(false);
    expect(root().classList.contains("dark")).toBe(false);
    media.setDark(true);
    expect(root().classList.contains("dark")).toBe(true);
    expect(getThemePreference()).toBe("system");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("system");
  });

  it("persists across re-initialization (initTheme re-applies the override)", async () => {
    installMatchMediaStub(false);
    await initTheme();
    setThemePreference("dark");

    await initTheme();

    expect(getThemePreference()).toBe("dark");
    expect(root().classList.contains("dark")).toBe(true);
  });

  it("seeds the preference from localStorage at init", async () => {
    installMatchMediaStub(false); // OS light — override must win anyway
    localStorage.setItem(THEME_STORAGE_KEY, "dark");

    await initTheme();

    expect(getThemePreference()).toBe("dark");
    expect(root().classList.contains("dark")).toBe(true);
  });

  it("falls back to system on a corrupt persisted value", async () => {
    installMatchMediaStub(true);
    localStorage.setItem(THEME_STORAGE_KEY, "sepia");

    await initTheme();

    expect(getThemePreference()).toBe("system");
    expect(root().classList.contains("dark")).toBe(true);
  });

  it("honors a preference set before init", async () => {
    installMatchMediaStub(false);
    disposeTheme();
    setThemePreference("dark");

    await initTheme();

    expect(root().classList.contains("dark")).toBe(true);
  });

  it("setThemePreference is never-throwing and ignores invalid values", async () => {
    installMatchMediaStub(false);
    await initTheme();

    expect(() => setThemePreference("banana" as never)).not.toThrow();
    expect(getThemePreference()).toBe("system");
    expect(root().classList.contains("dark")).toBe(false);
  });

  it("announces preference changes so mounted toggles stay in sync", () => {
    const seen: string[] = [];
    const onChange = (event: Event) => {
      seen.push((event as CustomEvent<string>).detail);
    };
    window.addEventListener(THEME_PREFERENCE_EVENT, onChange);
    try {
      setThemePreference("dark");
      setThemePreference("system");
    } finally {
      window.removeEventListener(THEME_PREFERENCE_EVENT, onChange);
    }
    expect(seen).toEqual(["dark", "system"]);
  });
});

describe("applied scheme stamp + announcement", () => {
  beforeEach(() => {
    disposeTheme();
    resetRoot();
    localStorage.removeItem(THEME_STORAGE_KEY);
    setThemePreference("system");
    vi.unstubAllGlobals();
  });

  it("stamps <html data-theme>/.dark and announces every applied scheme", async () => {
    installMatchMediaStub(false);
    const seen: string[] = [];
    const onChange = (event: Event) => {
      seen.push((event as CustomEvent<string>).detail);
    };
    window.addEventListener(THEME_SCHEME_EVENT, onChange);
    try {
      await initTheme();
      expect(document.documentElement.dataset.theme).toBe("light");

      setThemePreference("dark");
      expect(document.documentElement.dataset.theme).toBe("dark");

      setThemePreference("light");
      expect(document.documentElement.dataset.theme).toBe("light");
    } finally {
      window.removeEventListener(THEME_SCHEME_EVENT, onChange);
    }
    expect(seen).toEqual(["light", "dark", "light"]);
  });

  it("announces OS-driven switches too (Shiki follows live)", async () => {
    const media = installMatchMediaStub(false);
    await initTheme();

    const seen: string[] = [];
    const onChange = (event: Event) => {
      seen.push((event as CustomEvent<string>).detail);
    };
    window.addEventListener(THEME_SCHEME_EVENT, onChange);
    try {
      media.setDark(true);
    } finally {
      window.removeEventListener(THEME_SCHEME_EVENT, onChange);
    }
    expect(seen).toEqual(["dark"]);
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });
});
