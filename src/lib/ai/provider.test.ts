/**
 * Unit tests for AI provider selection: preferred-backend pick, fallback
 * behavior (enabled/disabled/other-side-down) and error shaping.
 */

import { describe, expect, it, vi } from "vitest";
import { DEFAULT_OPENCODE_URL, DEFAULT_OPENROUTER_MODEL, selectProvider } from "./provider";
import type { AiProbeResult, AiProvider, ProviderSet } from "./provider";
import { AiError } from "./types";
import type { AiBackend, AiResult } from "./types";

function mockProvider(id: AiBackend, probe: AiProbeResult): AiProvider {
  return {
    id,
    available: vi.fn(async () => probe.status !== "down"),
    check: vi.fn(async () => probe),
    listModels: vi.fn(async () => []),
    generate: vi.fn(async (): Promise<AiResult> => ({
      text: "t",
      model: id,
      backend: id,
      elapsedMs: 1,
    })),
  };
}

describe("selectProvider", () => {
  it("returns the configured backend when it is available", async () => {
    const providers: ProviderSet = {
      opencode: mockProvider("opencode", { status: "ok" }),
      openrouter: mockProvider("openrouter", { status: "ok" }),
    };
    const provider = await selectProvider({ backend: "openrouter" }, providers);
    expect(provider.id).toBe("openrouter");
  });

  it("falls back to the other backend when the preferred is unavailable (default)", async () => {
    const providers: ProviderSet = {
      opencode: mockProvider("opencode", { status: "down" }),
      openrouter: mockProvider("openrouter", { status: "ok" }),
    };
    const provider = await selectProvider({ backend: "opencode" }, providers);
    expect(provider.id).toBe("openrouter");
  });

  it("falls back in the other direction too (openrouter → opencode)", async () => {
    const providers: ProviderSet = {
      opencode: mockProvider("opencode", { status: "ok" }),
      openrouter: mockProvider("openrouter", { status: "down" }),
    };
    const provider = await selectProvider(
      { backend: "openrouter", allowFallback: true },
      providers,
    );
    expect(provider.id).toBe("opencode");
  });

  it("respects allowFallback: false even when the other side is fine", async () => {
    const providers: ProviderSet = {
      opencode: mockProvider("opencode", { status: "down" }),
      openrouter: mockProvider("openrouter", { status: "ok" }),
    };
    await expect(
      selectProvider({ backend: "opencode", allowFallback: false }, providers),
    ).rejects.toBeInstanceOf(AiError);
  });

  it("throws AiError unavailable when nothing is available", async () => {
    const providers: ProviderSet = {
      opencode: mockProvider("opencode", { status: "down" }),
      openrouter: mockProvider("openrouter", { status: "down" }),
    };
    const err = await selectProvider({ backend: "opencode" }, providers).catch(
      (e: unknown) => e,
    );
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).kind).toBe("unavailable");
    expect((err as AiError).backend).toBe("opencode");
    expect((err as AiError).message).toContain("fallback");
  });

  it("defaults: opencode URL and OpenRouter model placeholders are pinned", () => {
    expect(DEFAULT_OPENCODE_URL).toBe("http://127.0.0.1:4096");
    expect(DEFAULT_OPENROUTER_MODEL).toBe("anthropic/claude-sonnet-4.5");
  });
});
