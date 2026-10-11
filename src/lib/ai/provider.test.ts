/**
 * Unit tests for the AI provider surface: the pinned default constants and
 * the capability contract — optional methods (`connection`,
 * `subscribeLiveness`) are implemented ONLY by providers that can serve
 * them; absence is the capability signal (OpenRouter implements neither).
 */

import { describe, expect, it } from "vitest";
import { DEFAULT_OPENCODE_URL, DEFAULT_OPENROUTER_MODEL } from "./provider";
import { OpenRouterProvider } from "./openrouter";

describe("provider surface", () => {
  it("defaults: opencode URL and OpenRouter model placeholders are pinned", () => {
    expect(DEFAULT_OPENCODE_URL).toBe("http://127.0.0.1:4096");
    expect(DEFAULT_OPENROUTER_MODEL).toBe("anthropic/claude-sonnet-4.5");
  });

  it("OpenRouterProvider implements neither optional capability", () => {
    const provider = new OpenRouterProvider();
    // Absence is the capability signal: no stubs, no null-returning methods.
    expect("connection" in provider).toBe(false);
    expect("subscribeLiveness" in provider).toBe(false);
  });
});
