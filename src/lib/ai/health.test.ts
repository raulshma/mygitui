/**
 * Unit tests for the AI health chip mapping (ai/health.ts): tone/label
 * per supervisor status combination, fallback interaction, tooltip lines.
 */

import { describe, expect, it } from "vitest";
import type { BackendStatus, BackendStatusMap } from "$lib/ai/connection";
import { aiHealth, backendLine } from "$lib/ai/health";

function status(
  s: BackendStatus["status"],
  over: Partial<BackendStatus> = {},
): BackendStatus {
  return { status: s, lastCheck: null, latencyMs: null, ...over };
}

function map(
  opencode: BackendStatus["status"],
  openrouter: BackendStatus["status"],
): BackendStatusMap {
  return { opencode: status(opencode), openrouter: status(openrouter) };
}

describe("backendLine", () => {
  it("includes latency for ok", () => {
    expect(backendLine("opencode", status("ok", { latencyMs: 42 }))).toBe(
      "opencode: ok (42 ms)",
    );
    expect(backendLine("opencode", status("ok", { latencyMs: null }))).toBe(
      "opencode: ok (? ms)",
    );
  });

  it("covers the other statuses", () => {
    expect(backendLine("openrouter", status("unknown"))).toBe("openrouter: not probed yet");
    expect(backendLine("openrouter", status("checking"))).toBe("openrouter: checking…");
    expect(backendLine("openrouter", status("unauthenticated"))).toBe(
      "openrouter: needs credentials",
    );
    expect(backendLine("openrouter", status("down", { error: "boom" }))).toBe(
      "openrouter: down — boom",
    );
    expect(backendLine("openrouter", status("down"))).toBe("openrouter: down");
  });
});

describe("aiHealth", () => {
  it("preferred ok → green connected", () => {
    const view = aiHealth(map("ok", "unknown"), "opencode", true);
    expect(view.tone).toBe("ok");
    expect(view.label).toBe("connected");
  });

  it("checking → amber checking…", () => {
    const view = aiHealth(map("checking", "unknown"), "opencode", true);
    expect(view.tone).toBe("degraded");
    expect(view.label).toBe("checking…");
  });

  it("unauthenticated → amber needs credentials", () => {
    const view = aiHealth(map("unauthenticated", "ok"), "opencode", true);
    expect(view.tone).toBe("degraded");
    expect(view.label).toBe("needs credentials");
  });

  it("preferred down + fallback ok + allowed → amber degraded (fallback)", () => {
    const view = aiHealth(map("down", "ok"), "opencode", true);
    expect(view.tone).toBe("degraded");
    expect(view.label).toBe("degraded (fallback)");
  });

  it("preferred down + fallback disallowed → red down", () => {
    const view = aiHealth(map("down", "ok"), "opencode", false);
    expect(view.tone).toBe("down");
    expect(view.label).toBe("down");
  });

  it("preferred down + fallback down → red down", () => {
    const view = aiHealth(map("down", "down"), "opencode", true);
    expect(view.tone).toBe("down");
  });

  it("never probed → gray not configured", () => {
    const view = aiHealth(map("unknown", "unknown"), "opencode", true);
    expect(view.tone).toBe("off");
    expect(view.label).toBe("not configured");
  });

  it("works for either preferred backend", () => {
    expect(aiHealth(map("down", "ok"), "openrouter", true).tone).toBe("ok");
    // Preferred openrouter down, but opencode may serve: degraded, not down.
    expect(aiHealth(map("ok", "down"), "openrouter", true).label).toBe(
      "degraded (fallback)",
    );
  });

  it("tooltip carries one line per backend", () => {
    const view = aiHealth(
      {
        opencode: status("ok", { latencyMs: 12 }),
        openrouter: status("down", { error: "no key" }),
      },
      "opencode",
      true,
    );
    expect(view.detail).toBe("opencode: ok (12 ms)\nopenrouter: down — no key");
  });
});
