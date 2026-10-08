/**
 * Tests for the M3 theme foundation (src/lib/theme).
 *
 * Note on the resolver hook below: @material/material-color-utilities@0.4.0
 * shipped a few extensionless internal imports ("./dynamic_scheme" etc.).
 * Vite resolves those fine (so `vite build` is unaffected), but vitest
 * externalizes node_modules to Node's native ESM, which cannot. The hook
 * retries failing relative specifiers with ".js" appended — registered before
 * any import that pulls the package in.
 */
import { registerHooks } from "node:module";
import { beforeEach, describe, expect, it, vi } from "vitest";

registerHooks({
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (error) {
      const code = (error as NodeJS.ErrnoException | null)?.code;
      if (
        code === "ERR_MODULE_NOT_FOUND" &&
        (specifier.startsWith("./") || specifier.startsWith("../")) &&
        !specifier.endsWith(".js")
      ) {
        return nextResolve(specifier + ".js", context);
      }
      throw error;
    }
  },
});

// Imported dynamically so the hook above is in place first.
const {
  initTheme,
  disposeTheme,
  currentScheme,
  getThemePreference,
  setThemePreference,
  THEME_STORAGE_KEY,
  TOKEN_NAMES,
} = await import("./index");
const { BASELINE_SEED, schemeFromSeed, tokensForScheme } = await import("./dynamic-color");

const LIGHT = tokensForScheme(schemeFromSeed(BASELINE_SEED, false));
const DARK = tokensForScheme(schemeFromSeed(BASELINE_SEED, true));

/** Tokens every consumer can rely on; --m3c-* aliases mirror them. */
const CORE_TOKENS = [
  "primary",
  "on-primary",
  "primary-container",
  "on-primary-container",
  "secondary",
  "on-secondary",
  "secondary-container",
  "on-secondary-container",
  "tertiary",
  "on-tertiary",
  "tertiary-container",
  "on-tertiary-container",
  "error",
  "on-error",
  "error-container",
  "on-error-container",
  "surface",
  "on-surface",
  "on-surface-variant",
  "surface-container-lowest",
  "surface-container-low",
  "surface-container",
  "surface-container-high",
  "surface-container-highest",
  "outline",
  "outline-variant",
] as const;

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

const rootStyle = () => document.documentElement.style;

describe("initTheme", () => {
  beforeEach(() => {
    disposeTheme();
    rootStyle().cssText = "";
    localStorage.removeItem(THEME_STORAGE_KEY);
    setThemePreference("system");
    vi.unstubAllGlobals();
  });

  it("resolves without throwing and writes the core --m3-* custom properties", async () => {
    installMatchMediaStub(false);

    await expect(initTheme()).resolves.toBeUndefined();

    for (const token of CORE_TOKENS) {
      expect(rootStyle().getPropertyValue(`--m3-${token}`), `--m3-${token}`).toMatch(
        /^#[0-9a-f]{6}$/,
      );
    }
  });

  it("writes values derived from the baseline palette (light scheme first)", () => {
    installMatchMediaStub(false);
    return initTheme().then(() => {
      expect(rootStyle().getPropertyValue("--m3-surface")).toBe(LIGHT.surface);
      expect(rootStyle().getPropertyValue("--m3-primary")).toBe(LIGHT.primary);
      expect(rootStyle().getPropertyValue("--m3-on-primary")).toBe(LIGHT["on-primary"]);
      expect(rootStyle().getPropertyValue("--m3-primary-container")).toBe(
        LIGHT["primary-container"],
      );
      expect(rootStyle().getPropertyValue("--m3-outline")).toBe(LIGHT.outline);
      expect(rootStyle().getPropertyValue("color-scheme")).toBe("light");
      expect(currentScheme()).toBe("light");
    });
  });

  it("publishes the --m3c-* aliases m3-svelte components consume", () => {
    installMatchMediaStub(false);
    return initTheme().then(() => {
      expect(rootStyle().getPropertyValue("--m3c-primary")).toBe(LIGHT.primary);
      expect(rootStyle().getPropertyValue("--m3c-on-secondary-container")).toBe(
        LIGHT["on-secondary-container"],
      );
      expect(rootStyle().getPropertyValue("--m3c-surface-container-low")).toBe(
        LIGHT["surface-container-low"],
      );
    });
  });

  it("publishes every documented token", () => {
    installMatchMediaStub(false);
    return initTheme().then(() => {
      for (const name of TOKEN_NAMES) {
        expect(rootStyle().getPropertyValue(`--m3-${name}`), `--m3-${name}`).toMatch(
          /^#[0-9a-f]{6}$/,
        );
      }
    });
  });

  it("flips the token set (incl. --m3-surface) when the OS switches to dark and back", async () => {
    const media = installMatchMediaStub(false);
    await initTheme();

    const lightSurface = rootStyle().getPropertyValue("--m3-surface");
    expect(lightSurface).toBe(LIGHT.surface);

    media.setDark(true);
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
    expect(rootStyle().getPropertyValue("--m3-surface")).not.toBe(lightSurface);
    expect(rootStyle().getPropertyValue("--m3-on-surface")).toBe(DARK["on-surface"]);
    expect(rootStyle().getPropertyValue("color-scheme")).toBe("dark");
    expect(currentScheme()).toBe("dark");

    media.setDark(false);
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(LIGHT.surface);
    expect(currentScheme()).toBe("light");
  });

  it("applies the dark set immediately when the OS is already dark", () => {
    installMatchMediaStub(true);
    return initTheme().then(() => {
      expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
      expect(rootStyle().getPropertyValue("color-scheme")).toBe("dark");
    });
  });

  it("never throws when matchMedia is unavailable", async () => {
    // jsdom ships no matchMedia at all; simply do not stub it.
    await expect(initTheme()).resolves.toBeUndefined();
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(LIGHT.surface);
  });

  it("never throws when matchMedia itself explodes", async () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => {
        throw new Error("matchMedia unavailable");
      }),
    );
    await expect(initTheme()).resolves.toBeUndefined();
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(LIGHT.surface);
  });

  it("re-initializes cleanly without stacking duplicate listeners", async () => {
    const media = installMatchMediaStub(false);
    await initTheme();
    await initTheme();

    media.setDark(true);
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
    media.setDark(false);
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(LIGHT.surface);
  });
});

describe("theme preference override (M7 I2)", () => {
  beforeEach(() => {
    disposeTheme();
    rootStyle().cssText = "";
    localStorage.removeItem(THEME_STORAGE_KEY);
    setThemePreference("system");
    vi.unstubAllGlobals();
  });

  it("defaults to system and applies the OS scheme", async () => {
    installMatchMediaStub(true);
    await initTheme();

    expect(getThemePreference()).toBe("system");
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
    expect(currentScheme()).toBe("dark");
  });

  it("override pins light even when the OS is dark, and persists", async () => {
    installMatchMediaStub(true);
    await initTheme();

    setThemePreference("light");

    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(LIGHT.surface);
    expect(rootStyle().getPropertyValue("color-scheme")).toBe("light");
    expect(currentScheme()).toBe("light");
    expect(getThemePreference()).toBe("light");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("light");
  });

  it("override pins dark when the OS is light", async () => {
    installMatchMediaStub(false);
    await initTheme();

    setThemePreference("dark");

    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
    expect(rootStyle().getPropertyValue("color-scheme")).toBe("dark");
    expect(currentScheme()).toBe("dark");
  });

  it("ignores OS switches while overridden; system re-follows after reset", async () => {
    const media = installMatchMediaStub(false);
    await initTheme();

    setThemePreference("light");
    media.setDark(true);
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(LIGHT.surface);
    expect(currentScheme()).toBe("light");

    setThemePreference("system");
    // The stored override is gone, so the current OS state applies…
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
    expect(currentScheme()).toBe("dark");
    // …and the OS keeps driving live from here on.
    media.setDark(false);
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(LIGHT.surface);
    media.setDark(true);
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
    expect(getThemePreference()).toBe("system");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("system");
  });

  it("persists across re-initialization (initTheme re-applies the override)", async () => {
    installMatchMediaStub(false);
    await initTheme();
    setThemePreference("dark");

    await initTheme();

    expect(getThemePreference()).toBe("dark");
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
  });

  it("seeds the preference from localStorage at init", async () => {
    installMatchMediaStub(false); // OS light — override must win anyway
    localStorage.setItem(THEME_STORAGE_KEY, "dark");

    await initTheme();

    expect(getThemePreference()).toBe("dark");
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
  });

  it("falls back to system on a corrupt persisted value", async () => {
    installMatchMediaStub(true);
    localStorage.setItem(THEME_STORAGE_KEY, "sepia");

    await initTheme();

    expect(getThemePreference()).toBe("system");
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
  });

  it("honors a preference set before init", async () => {
    installMatchMediaStub(false);
    disposeTheme();
    setThemePreference("dark");

    await initTheme();

    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(DARK.surface);
  });

  it("setThemePreference is never-throwing and ignores invalid values", async () => {
    installMatchMediaStub(false);
    await initTheme();

    expect(() => setThemePreference("banana" as never)).not.toThrow();
    expect(getThemePreference()).toBe("system");
    expect(rootStyle().getPropertyValue("--m3-surface")).toBe(LIGHT.surface);
  });
});
