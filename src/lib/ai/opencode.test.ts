/**
 * Unit tests for the opencode backend adapter: health probing (timeout +
 * autodiscovery), basic-auth header injection, response-part parsing,
 * model-list flattening, and generation against a mocked
 * `@opencode-ai/sdk` client (session reuse, 404 session recreation, error
 * mapping).
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  authedFetch,
  basicAuthHeader,
  discoverOpencode,
  HEALTH_TIMEOUT_MS,
  OpenCodeProvider,
  partsToText,
  probeHealth,
  providersToModels,
  SESSION_TITLE,
} from "./opencode";
import type { FetchLike } from "./opencode";
import { AiError } from "./types";

// ---------------------------------------------------------------------------
// @opencode-ai/sdk mock (fixture client shared by the generation tests)
// ---------------------------------------------------------------------------

const FAKE_CLIENT = vi.hoisted(() => {
  const client = {
    session: {
      create: vi.fn(),
      prompt: vi.fn(),
    },
    config: {
      providers: vi.fn(),
    },
  };
  return client;
});

vi.mock("@opencode-ai/sdk/client", () => ({
  createOpencodeClient: vi.fn(() => FAKE_CLIENT),
}));

import { createOpencodeClient } from "@opencode-ai/sdk/client";

/** A fetch mock that answers `/global/health` (and records requests). */
function healthFetch(
  status = 200,
  options?: { fail?: boolean; delayMs?: number },
): FetchLike & { calls: Array<{ url: string; init?: RequestInit }> } {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const impl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.toString()
          : input.url;
    calls.push({ url, init });
    if (options?.fail) throw new TypeError("fetch failed");
    if (options?.delayMs) {
      await new Promise((resolve) => setTimeout(resolve, options.delayMs));
    }
    return { ok: status >= 200 && status < 300, status } as Response;
  }) as FetchLike & { calls: Array<{ url: string; init?: RequestInit }> };
  impl.calls = calls;
  return impl;
}

const SESSION = { id: "sess-1", title: SESSION_TITLE };

const PROMPT_PARTS = [
  { id: "p1", type: "step-start" },
  { id: "p2", type: "text", text: "fix(ui): fix the " },
  { id: "p3", type: "reasoning", text: "hmm… (ignored)" },
  { id: "p4", type: "text", text: "thing\n\nBecause it broke." },
  { id: "p5", type: "tool", state: { status: "done" } },
];

const PROVIDERS_REPLY = {
  providers: [
    { id: "anthropic", models: [{ id: "claude-sonnet-4-5" }, { id: "claude-opus-4" }] },
    { id: "openai", models: { "gpt-5": { id: "gpt-5" }, "gpt-5-mini": { id: "gpt-5-mini" } } },
    { id: "no-models" },
    { models: [{ id: "orphan" }] }, // no provider id → skipped
  ],
  default: {},
};

beforeEach(() => {
  vi.clearAllMocks();
  FAKE_CLIENT.session.create.mockReset();
  FAKE_CLIENT.session.prompt.mockReset();
  FAKE_CLIENT.config.providers.mockReset();
});

// ---------------------------------------------------------------------------
// Health probing + discovery
// ---------------------------------------------------------------------------

describe("probeHealth / discoverOpencode", () => {
  it("answers ok with the HTTP status when the server responds", async () => {
    const impl = healthFetch(200);
    const result = await probeHealth("http://127.0.0.1:4096", impl);
    expect(result).toEqual({ ok: true, status: 200 });
    expect(impl.calls[0].url).toBe("http://127.0.0.1:4096/global/health");
  });

  it("counts any response (even 4xx/5xx) as reachable, network failure as down", async () => {
    expect((await probeHealth("http://127.0.0.1:4096", healthFetch(401))).ok).toBe(true);
    expect((await probeHealth("http://127.0.0.1:4096", healthFetch(500))).ok).toBe(true);
    await expect(
      probeHealth("http://127.0.0.1:4096", healthFetch(0, { fail: true })),
    ).resolves.toEqual({ ok: false, status: 0 });
  });

  it("aborts the probe after the 2s health timeout", async () => {
    vi.useFakeTimers();
    try {
      // A fetch that hangs until the signal aborts (like a real one would).
      const hanging: FetchLike = ((input: RequestInfo | URL, init?: RequestInit) =>
        new Promise((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () =>
            reject(new DOMException("aborted", "AbortError")),
          );
        })) as FetchLike;
      const pending = probeHealth("http://127.0.0.1:4096", hanging);
      const asserted = expect(pending).resolves.toEqual({ ok: false, status: 0 });
      await vi.advanceTimersByTimeAsync(HEALTH_TIMEOUT_MS + 1);
      await asserted;
    } finally {
      vi.useRealTimers();
    }
  });

  it("discover prefers the manual URL, falls back to the default, or gives up", async () => {
    const both = healthFetch(200);
    await expect(discoverOpencode("http://localhost:9999", both)).resolves.toBe(
      "http://localhost:9999",
    );
    expect(both.calls[0].url).toBe("http://localhost:9999/global/health");

    // Manual fails, default answers.
    const mixed: FetchLike = (async (input: RequestInfo | URL) => {
      const url =
        typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      if (url === "http://localhost:9999/global/health") throw new TypeError("refused");
      return { ok: true, status: 200 } as Response;
    }) as FetchLike;
    await expect(discoverOpencode("http://localhost:9999", mixed)).resolves.toBe(
      "http://127.0.0.1:4096",
    );

    const none = healthFetch(0, { fail: true });
    await expect(discoverOpencode(undefined, none)).resolves.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Basic auth
// ---------------------------------------------------------------------------

describe("basic auth injection", () => {
  it("builds a Basic header over 'opencode:<password>'", () => {
    expect(basicAuthHeader("hunter2")).toBe(
      `Basic ${btoa("opencode:hunter2")}`,
    );
  });

  it("authedFetch adds the header only to the configured base", async () => {
    const inner = vi.fn(
      async (_input: RequestInfo | URL, init?: RequestInit): Promise<Response> =>
        ({ ok: true, status: 200 }) as Response,
    );
    const wrapped = authedFetch("http://127.0.0.1:4096", async () => "hunter2", inner as unknown as FetchLike);

    await wrapped("http://127.0.0.1:4096/session", { headers: { "X-Keep": "yes" } });
    await wrapped("http://example.com/other");

    expect(inner).toHaveBeenCalledTimes(2);
    const [authedUrl, authedInit] = inner.mock.calls[0];
    expect(authedUrl).toBe("http://127.0.0.1:4096/session");
    const headers = new Headers(authedInit?.headers);
    expect(headers.get("Authorization")).toBe(`Basic ${btoa("opencode:hunter2")}`);
    expect(headers.get("X-Keep")).toBe("yes");

    const [otherUrl, otherInit] = inner.mock.calls[1];
    expect(otherUrl).toBe("http://example.com/other");
    expect(new Headers(otherInit?.headers).get("Authorization")).toBeNull();
  });

  it("authedFetch passes requests through when no password is configured", async () => {
    const inner = vi.fn(
      async (_input: RequestInfo | URL, init?: RequestInit): Promise<Response> =>
        ({ ok: true, status: 200 }) as Response,
    );
    const wrapped = authedFetch("http://127.0.0.1:4096", async () => null, inner as unknown as FetchLike);
    await wrapped("http://127.0.0.1:4096/session");
    const init = inner.mock.calls[0][1];
    expect(new Headers(init?.headers).get("Authorization")).toBeNull();
  });

  it("authedFetch merges headers from a Request object (SDK passes Requests)", async () => {
    const inner = vi.fn(
      async (_input: RequestInfo | URL, init?: RequestInit): Promise<Response> =>
        ({ ok: true, status: 200 }) as Response,
    );
    const wrapped = authedFetch("http://127.0.0.1:4096", async () => "pw", inner as unknown as FetchLike);
    const request = new Request("http://127.0.0.1:4096/session", {
      headers: { "Content-Type": "application/json" },
    });
    await wrapped(request);
    const headers = new Headers(inner.mock.calls[0][1]?.headers);
    expect(headers.get("Content-Type")).toBe("application/json");
    expect(headers.get("Authorization")).toBe(`Basic ${btoa("opencode:pw")}`);
  });
});

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

describe("partsToText", () => {
  it("concatenates only text parts and trims the result", () => {
    expect(partsToText(PROMPT_PARTS)).toBe("fix(ui): fix the thing\n\nBecause it broke.");
  });

  it("returns null for no text parts / non-array input / empty text", () => {
    expect(partsToText([{ type: "reasoning", text: "hmm" }])).toBeNull();
    expect(partsToText(undefined)).toBeNull();
    expect(partsToText("nope")).toBeNull();
    expect(partsToText([{ type: "text", text: "   " }])).toBeNull();
  });
});

describe("providersToModels", () => {
  it("flattens provider/model pairs from both list and record shapes", () => {
    expect(providersToModels(PROVIDERS_REPLY)).toEqual([
      { id: "anthropic/claude-sonnet-4-5", provider: "anthropic", label: "anthropic/claude-sonnet-4-5" },
      { id: "anthropic/claude-opus-4", provider: "anthropic", label: "anthropic/claude-opus-4" },
      { id: "openai/gpt-5", provider: "openai", label: "openai/gpt-5" },
      { id: "openai/gpt-5-mini", provider: "openai", label: "openai/gpt-5-mini" },
    ]);
  });

  it("tolerates malformed replies", () => {
    expect(providersToModels(undefined)).toEqual([]);
    expect(providersToModels({})).toEqual([]);
    expect(providersToModels({ providers: "nope" })).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// OpenCodeProvider against the mocked SDK
// ---------------------------------------------------------------------------

function makeProvider(fetchImpl?: FetchLike, password?: string): OpenCodeProvider {
  return new OpenCodeProvider({
    fetchImpl: fetchImpl ?? healthFetch(200),
    ...(password !== undefined
      ? { password: async () => password }
      : {}),
  });
}

describe("OpenCodeProvider", () => {
  it("available() is true when the health probe answers and false otherwise", async () => {
    expect(await makeProvider().available()).toBe(true);
    expect(await makeProvider(healthFetch(0, { fail: true })).available()).toBe(false);
  });

  it("check() maps 401/403 health answers to unauthenticated", async () => {
    const provider = makeProvider(healthFetch(401));
    expect(await provider.check()).toMatchObject({ status: "unauthenticated" });
    const provider403 = makeProvider(healthFetch(403));
    expect(await provider403.check()).toMatchObject({ status: "unauthenticated" });
    const provider500 = makeProvider(healthFetch(500));
    expect(await provider500.check()).toMatchObject({ status: "ok" }); // reachable
  });

  it("check() reports down when no server is found", async () => {
    const provider = makeProvider(healthFetch(0, { fail: true }));
    const result = await provider.check();
    expect(result.status).toBe("down");
    expect(result.error).toContain("no opencode server");
  });

  it("listModels flattens /config/providers via the SDK client", async () => {
    FAKE_CLIENT.config.providers.mockResolvedValue({ data: PROVIDERS_REPLY, error: undefined });
    const models = await makeProvider().listModels();
    expect(models.map((m) => m.id)).toEqual([
      "anthropic/claude-sonnet-4-5",
      "anthropic/claude-opus-4",
      "openai/gpt-5",
      "openai/gpt-5-mini",
    ]);
    expect(createOpencodeClient).toHaveBeenCalledWith(
      expect.objectContaining({ baseUrl: "http://127.0.0.1:4096" }),
    );
  });

  it("listModels maps SDK errors to AiError", async () => {
    FAKE_CLIENT.config.providers.mockRejectedValue(new Error("socket hang up"));
    const err = await makeProvider().listModels().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).backend).toBe("opencode");
    expect((err as AiError).message).toContain("socket hang up");
  });

  it("generate creates a titled session once and reuses it (repoId session map)", async () => {
    FAKE_CLIENT.session.create.mockResolvedValue({ data: SESSION, error: undefined });
    FAKE_CLIENT.session.prompt.mockResolvedValue({ data: { parts: PROMPT_PARTS }, error: undefined });

    const provider = makeProvider();
    const first = await provider.generate({
      system: "sys",
      prompt: "user prompt",
      model: "anthropic/claude-sonnet-4-5",
      sessionKey: "repo-1",
    });
    const second = await provider.generate({ prompt: "again", sessionKey: "repo-1" });

    expect(FAKE_CLIENT.session.create).toHaveBeenCalledTimes(1);
    expect(FAKE_CLIENT.session.create).toHaveBeenCalledWith({
      body: { title: SESSION_TITLE },
    });
    expect(FAKE_CLIENT.session.prompt).toHaveBeenCalledTimes(2);
    expect(FAKE_CLIENT.session.prompt.mock.calls[0][0]).toMatchObject({
      path: { id: "sess-1" },
      body: {
        parts: [{ type: "text", text: "user prompt" }],
        system: "sys",
        model: { providerID: "anthropic", modelID: "claude-sonnet-4-5" },
      },
    });
    expect(FAKE_CLIENT.session.prompt.mock.calls[1][0]).toMatchObject({
      path: { id: "sess-1" },
    });

    expect(first).toMatchObject({
      text: "fix(ui): fix the thing\n\nBecause it broke.",
      backend: "opencode",
      model: "anthropic/claude-sonnet-4-5",
    });
    expect(second.text).toBe(first.text);
  });

  it("generate recreates the session once when the server forgot it", async () => {
    FAKE_CLIENT.session.create
      .mockResolvedValueOnce({ data: { id: "sess-stale" }, error: undefined })
      .mockResolvedValueOnce({ data: { id: "sess-fresh" }, error: undefined });
    FAKE_CLIENT.session.prompt
      .mockResolvedValueOnce({ data: undefined, error: { message: "session not found" } }) // 404-ish
      .mockResolvedValueOnce({ data: { parts: PROMPT_PARTS }, error: undefined });

    const provider = makeProvider();
    const result = await provider.generate({ prompt: "hello", sessionKey: "repo-2" });

    expect(FAKE_CLIENT.session.create).toHaveBeenCalledTimes(2);
    expect(FAKE_CLIENT.session.prompt).toHaveBeenCalledTimes(2);
    expect(FAKE_CLIENT.session.prompt.mock.calls[1][0]).toMatchObject({
      path: { id: "sess-fresh" },
    });
    expect(result.text).toContain("fix(ui)");
  });

  it("generate throws AiError bad-response when the prompt fails and the session was new", async () => {
    FAKE_CLIENT.session.create.mockResolvedValue({ data: SESSION, error: undefined });
    FAKE_CLIENT.session.prompt.mockResolvedValue({
      data: undefined,
      error: { message: "provider quota exceeded" },
    });

    const err = await makeProvider()
      .generate({ prompt: "hello", sessionKey: "repo-3" })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).kind).toBe("bad-response");
    expect((err as AiError).message).toContain("provider quota exceeded");
  });

  it("generate throws AiError bad-response when the answer has no text", async () => {
    FAKE_CLIENT.session.create.mockResolvedValue({ data: SESSION, error: undefined });
    FAKE_CLIENT.session.prompt.mockResolvedValue({
      data: { parts: [{ type: "reasoning", text: "…" }] },
      error: undefined,
    });
    await expect(
      makeProvider().generate({ prompt: "hello", sessionKey: "repo-4" }),
    ).rejects.toMatchObject({ kind: "bad-response" });
  });

  it("generate surfaces discovery failures as AiError unavailable", async () => {
    const err = await makeProvider(healthFetch(0, { fail: true }))
      .generate({ prompt: "hello" })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).kind).toBe("unavailable");
  });

  it("respects a manual URL override from settings", async () => {
    const impl = healthFetch(200);
    const provider = new OpenCodeProvider({ url: "http://localhost:9999", fetchImpl: impl });
    FAKE_CLIENT.config.providers.mockResolvedValue({ data: PROVIDERS_REPLY, error: undefined });
    await provider.listModels();
    expect(createOpencodeClient).toHaveBeenLastCalledWith(
      expect.objectContaining({ baseUrl: "http://localhost:9999" }),
    );
  });
});
