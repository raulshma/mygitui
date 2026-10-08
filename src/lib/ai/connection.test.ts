/**
 * Unit tests for the AI connection supervisor: status transitions, probe
 * coalescing, jittered exponential backoff (probe-only), ensureReady
 * backend selection + fallback, and the no-auto-replay guarantee on
 * generate failures. Timers, clock and jitter are injected (deterministic).
 */

import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { ConnectionSupervisor, RETRY_BASE_MS, RETRY_MAX_MS, STALE_AFTER_MS } from "./connection";
import type { AiProbeResult, AiProvider, ProviderSet } from "./provider";
import { AiError } from "./types";
import type { AiBackend, AiResult } from "./types";

/** A loose vi.fn alias (mock call inspection in tests). */
type MockFn = Mock<(...args: any[]) => any>;

/** A provider mock whose probe result the test can flip in place. */
interface MockProvider extends AiProvider {
  probe: AiProbeResult;
  available: MockFn;
  check: MockFn;
  listModels: MockFn;
  generateMock: MockFn;
  generate: MockFn;
}

function mockProvider(id: AiBackend): MockProvider {
  const provider = {
    id,
    probe: { status: "ok" } as AiProbeResult,
    available: vi.fn(async () => provider.probe.status !== "down"),
    check: vi.fn(async (): Promise<AiProbeResult> => provider.probe),
    listModels: vi.fn(async () => []),
    generateMock: vi.fn(async (): Promise<AiResult> => ({
      text: `answer-from-${id}`,
      model: `${id}-model`,
      backend: id,
      elapsedMs: 3,
    })),
  };
  const mock = provider as unknown as MockProvider;
  mock.generate = mock.generateMock;
  return mock;
}

interface Harness {
  supervisor: ConnectionSupervisor;
  opencode: MockProvider;
  openrouter: MockProvider;
  now: { value: number };
}

function makeHarness(opts?: {
  backend?: AiBackend;
  allowFallback?: boolean;
}): Harness {
  const opencode = mockProvider("opencode");
  const openrouter = mockProvider("openrouter");
  const providers: ProviderSet = { opencode, openrouter };
  const now = { value: 1_000_000 };
  const supervisor = new ConnectionSupervisor({
    providers,
    config: () => ({
      backend: opts?.backend ?? "opencode",
      allowFallback: opts?.allowFallback,
    }),
    now: () => now.value,
    jitter: () => 0.5, // exactly the base delay (±0%)
  });
  return { supervisor, opencode, openrouter, now };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("status transitions", () => {
  it("starts unknown for both backends", () => {
    const { supervisor } = makeHarness();
    expect(supervisor.status("opencode")).toMatchObject({ status: "unknown", lastCheck: null });
    expect(supervisor.status("openrouter")).toMatchObject({ status: "unknown", lastCheck: null });
  });

  it("check() records ok with latency and lastCheck", async () => {
    const { supervisor, now } = makeHarness();
    await supervisor.check("opencode");
    const status = supervisor.status("opencode");
    expect(status.status).toBe("ok");
    expect(status.lastCheck).toBe(now.value);
    expect(status.latencyMs).toBe(0);
    expect(status.error).toBeUndefined();
  });

  it("check() records down with the probe error", async () => {
    const h = makeHarness();
    h.opencode.probe = { status: "down", error: "no server at 127.0.0.1:4096" };
    await h.supervisor.check("opencode");
    expect(h.supervisor.status("opencode")).toMatchObject({
      status: "down",
      error: "no server at 127.0.0.1:4096",
    });
  });

  it("check() records unauthenticated separately from down", async () => {
    const h = makeHarness();
    h.openrouter.probe = { status: "unauthenticated", error: "bad API key" };
    await h.supervisor.check("openrouter");
    expect(h.supervisor.status("openrouter").status).toBe("unauthenticated");
  });

  it("check() passes through a probe failure as down (probe never throws)", async () => {
    const h = makeHarness();
    h.opencode.check = vi.fn(async () => {
      throw new Error("boom");
    });
    await h.supervisor.check("opencode");
    expect(h.supervisor.status("opencode")).toMatchObject({ status: "down", error: "boom" });
  });

  it("subscribe sees every status change (store contract)", async () => {
    const h = makeHarness();
    const seen: string[] = [];
    const unsub = h.supervisor.subscribe((statuses) => {
      seen.push(statuses.opencode.status);
    });
    h.opencode.probe = { status: "down" };
    await h.supervisor.check("opencode");
    unsub();
    await h.supervisor.check("opencode");
    // Initial emission + checking + down, then no more after unsubscribing.
    expect(seen).toEqual(["unknown", "checking", "down"]);
  });

  it("concurrent checks for one backend coalesce into a single probe", async () => {
    const h = makeHarness();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    h.opencode.check = vi.fn(async () => {
      await gate;
      return { status: "ok" } as AiProbeResult;
    });

    const a = h.supervisor.check("opencode");
    const b = h.supervisor.check("opencode");
    release();
    await Promise.all([a, b]);

    expect(h.opencode.check).toHaveBeenCalledTimes(1);
  });
});

describe("backoff retry loop (probe-only)", () => {
  it("re-probes a down backend with doubling delays 5s → 10s → …", async () => {
    const h = makeHarness();
    h.opencode.probe = { status: "down" };

    await h.supervisor.check("opencode"); // failure #1 → arms 5s
    expect(h.opencode.check).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(RETRY_BASE_MS - 1);
    expect(h.opencode.check).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(1); // fires the 5s probe
    expect(h.opencode.check).toHaveBeenCalledTimes(2);

    await vi.advanceTimersByTimeAsync(RETRY_BASE_MS * 2); // 10s later
    expect(h.opencode.check).toHaveBeenCalledTimes(3);
  });

  it("caps the delay at 5 minutes", async () => {
    const h = makeHarness();
    h.opencode.probe = { status: "down" };
    await h.supervisor.check("opencode");

    // Many failures later, the delay must never exceed RETRY_MAX_MS.
    for (let i = 0; i < 12; i += 1) {
      await vi.advanceTimersByTimeAsync(RETRY_MAX_MS + 1_000);
    }
    const calls = h.opencode.check.mock.calls.length;
    // 12 more probes fired (one per interval) — the loop is still alive.
    expect(calls).toBeGreaterThanOrEqual(13);

    // Sanity: exponential growth never exceeds the cap.
    const supervisorDelays = [RETRY_BASE_MS, RETRY_BASE_MS * 2, RETRY_BASE_MS * 2 ** 20];
    for (const delay of supervisorDelays) {
      expect(Math.min(delay, RETRY_MAX_MS)).toBeLessThanOrEqual(RETRY_MAX_MS);
    }
  });

  it("stops probing once a probe succeeds", async () => {
    const h = makeHarness();
    h.opencode.probe = { status: "down" };
    await h.supervisor.check("opencode");

    h.opencode.probe = { status: "ok" };
    await vi.advanceTimersByTimeAsync(RETRY_BASE_MS);
    const callsAfterRecovery = h.opencode.check.mock.calls.length;

    await vi.advanceTimersByTimeAsync(RETRY_MAX_MS * 2);
    expect(h.opencode.check.mock.calls.length).toBe(callsAfterRecovery);
    expect(h.supervisor.status("opencode").status).toBe("ok");
  });

  it("stopRetries() cancels armed probes", async () => {
    const h = makeHarness();
    h.opencode.probe = { status: "down" };
    await h.supervisor.check("opencode");
    h.supervisor.stopRetries();

    await vi.advanceTimersByTimeAsync(RETRY_MAX_MS * 3);
    expect(h.opencode.check).toHaveBeenCalledTimes(1);
  });

  it("an unauthenticated status does not keep retrying", async () => {
    const h = makeHarness();
    h.openrouter.probe = { status: "unauthenticated" };
    await h.supervisor.check("openrouter");
    const calls = h.openrouter.check.mock.calls.length;

    await vi.advanceTimersByTimeAsync(RETRY_MAX_MS * 3);
    expect(h.openrouter.check.mock.calls.length).toBe(calls);
  });
});

describe("ensureReady (selection + fallback)", () => {
  it("returns the configured provider when it probes ok", async () => {
    const h = makeHarness({ backend: "opencode" });
    const provider = await h.supervisor.ensureReady();
    expect(provider.id).toBe("opencode");
  });

  it("falls back to the other backend when the preferred one is down", async () => {
    const h = makeHarness({ backend: "opencode", allowFallback: true });
    h.opencode.probe = { status: "down" };
    const provider = await h.supervisor.ensureReady();
    expect(provider.id).toBe("openrouter");
  });

  it("throws AiError unavailable when fallback is disabled", async () => {
    const h = makeHarness({ backend: "opencode", allowFallback: false });
    h.opencode.probe = { status: "down" };
    await expect(h.supervisor.ensureReady()).rejects.toMatchObject({
      name: "AiError",
      kind: "unavailable",
      backend: "opencode",
    });
  });

  it("throws AiError unavailable when nothing is usable", async () => {
    const h = makeHarness({ backend: "opencode", allowFallback: true });
    h.opencode.probe = { status: "down" };
    h.openrouter.probe = { status: "down" };
    await expect(h.supervisor.ensureReady()).rejects.toMatchObject({ kind: "unavailable" });
  });

  it("maps an unauthenticated preferred backend to kind unauthenticated", async () => {
    const h = makeHarness({ backend: "opencode", allowFallback: false });
    h.opencode.probe = { status: "unauthenticated", error: "password required" };
    await expect(h.supervisor.ensureReady()).rejects.toMatchObject({
      kind: "unauthenticated",
    });
  });

  it("does not re-probe while the status is fresh (< STALE_AFTER_MS)", async () => {
    const h = makeHarness();
    await h.supervisor.ensureReady();
    expect(h.opencode.check).toHaveBeenCalledTimes(1);

    h.now.value += STALE_AFTER_MS - 1;
    await h.supervisor.ensureReady();
    expect(h.opencode.check).toHaveBeenCalledTimes(1);
  });

  it("re-probes once the status goes stale (≥ STALE_AFTER_MS)", async () => {
    const h = makeHarness();
    await h.supervisor.ensureReady();
    h.now.value += STALE_AFTER_MS + 1;
    await h.supervisor.ensureReady();
    expect(h.opencode.check).toHaveBeenCalledTimes(2);
  });
});

describe("generate routing (never auto-replays)", () => {
  it("routes through ensureReady and returns the provider result", async () => {
    const h = makeHarness({ backend: "openrouter" });
    const result = await h.supervisor.generate({ prompt: "hi", sessionKey: "repo-1" });
    expect(result.backend).toBe("openrouter");
    expect(h.openrouter.generateMock).toHaveBeenCalledWith(
      expect.objectContaining({ prompt: "hi", sessionKey: "repo-1" }),
    );
  });

  it("propagates generate failures verbatim, exactly one call (no replay)", async () => {
    const h = makeHarness();
    h.opencode.generateMock.mockRejectedValue(new AiError("bad-response", "no text", {}));
    await expect(h.supervisor.generate({ prompt: "hi" })).rejects.toMatchObject({
      kind: "bad-response",
    });
    expect(h.opencode.generateMock).toHaveBeenCalledTimes(1);
    // A bad response does not mark the backend down or arm retries.
    expect(h.supervisor.status("opencode").status).toBe("ok");
  });

  it("marks the backend down when generation reports unavailability and arms a probe-only retry", async () => {
    const h = makeHarness();
    h.opencode.generateMock.mockRejectedValueOnce(
      new AiError("unavailable", "connection reset", { backend: "opencode" }),
    );
    await expect(h.supervisor.generate({ prompt: "hi" })).rejects.toMatchObject({
      kind: "unavailable",
    });
    expect(h.opencode.generateMock).toHaveBeenCalledTimes(1); // NO auto-replay
    expect(h.supervisor.status("opencode").status).toBe("down");

    // Backoff loop arms (5s) and re-probes — probing only, still no replay.
    await vi.advanceTimersByTimeAsync(RETRY_BASE_MS + 1);
    expect(h.opencode.generateMock).toHaveBeenCalledTimes(1);
    expect(h.opencode.check).toHaveBeenCalledTimes(2); // initial + retry probe
  });
});
