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
  decodeHealthV1,
  decodeInfoV2,
  discoverOpencode,
  HEALTH_TIMEOUT_MS,
  isJsonContentType,
  OpenCodeProvider,
  partsToText,
  probeOpencodeServer,
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

/** A fetch mock that answers the probe endpoints (and records requests).
 *
 * Defaults to a 1.x-shaped server: `/api/info` serves the web UI's HTML
 * with a 200 (what real 1.x servers do on unknown paths), `/global/health`
 * serves the JSON health body with the configured status.
 */
function healthFetch(
  status = 200,
  options?: { fail?: boolean; delayMs?: number; v2?: boolean },
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
    if (options?.v2 && url.endsWith("/api/info") && status === 200) {
      return jsonResponse(200, { version: "2.0.18", pid: 4242 });
    }
    if (url.endsWith("/api/info")) {
      // HTML fallback (web UI), like a real 1.x server.
      return htmlResponse(status);
    }
    if (status === 200) {
      return jsonResponse(200, { healthy: true, version: "1.18.35" });
    }
    return new Response("nope", { status });
  }) as FetchLike & { calls: Array<{ url: string; init?: RequestInit }> };
  impl.calls = calls;
  return impl;
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function htmlResponse(status: number): Response {
  return new Response("<html>opencode</html>", {
    status,
    headers: { "content-type": "text/html" },
  });
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

describe("probeOpencodeServer / discoverOpencode", () => {
  it("identifies a 1.x server via the JSON /global/health body", async () => {
    const impl = healthFetch(200);
    const probe = await probeOpencodeServer("http://127.0.0.1:4096", impl);
    expect(probe).toEqual({
      state: "ok",
      server: { base: "http://127.0.0.1:4096", kind: "v1", version: "1.18.35" },
    });
    // t3 order: /api/info first, /global/health second.
    expect(impl.calls[0].url).toBe("http://127.0.0.1:4096/api/info");
    expect(impl.calls[1].url).toBe("http://127.0.0.1:4096/global/health");
  });

  it("identifies a 2.x server via the JSON /api/info body", async () => {
    const impl = healthFetch(200, { v2: true });
    await expect(probeOpencodeServer("http://127.0.0.1:4097", impl)).resolves.toEqual({
      state: "ok",
      server: { base: "http://127.0.0.1:4097", kind: "v2", version: "2.0.18" },
    });
    // One probe only — /api/info answered, /global/health never fetched.
    expect(impl.calls).toHaveLength(1);
  });

  it("rejects HTML replies even with a 200 (a 200 alone is not opencode)", async () => {
    const htmlOnly: FetchLike = (async (input: RequestInfo | URL) => {
      void input;
      return htmlResponse(200);
    }) as FetchLike;
    const probe = await probeOpencodeServer("http://127.0.0.1:4098", htmlOnly);
    expect(probe).toMatchObject({ state: "absent" });
  });

  it("maps 401/403 to unauthorized and other statuses / network failure to absent", async () => {
    expect(await probeOpencodeServer("http://127.0.0.1:4096", healthFetch(401))).toMatchObject({
      state: "unauthorized",
      status: 401,
    });
    expect(await probeOpencodeServer("http://127.0.0.1:4096", healthFetch(500))).toMatchObject({
      state: "absent",
      status: 500,
    });
    await expect(
      probeOpencodeServer("http://127.0.0.1:4096", healthFetch(0, { fail: true })),
    ).resolves.toEqual({ state: "absent", status: 0 });
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
      const pending = probeOpencodeServer("http://127.0.0.1:4096", hanging);
      const asserted = expect(pending).resolves.toEqual({ state: "absent", status: 0 });
      await vi.advanceTimersByTimeAsync(HEALTH_TIMEOUT_MS + 1);
      await asserted;
    } finally {
      vi.useRealTimers();
    }
  });

  it("decode helpers only accept their exact shapes", () => {
    expect(isJsonContentType("application/json")).toBe(true);
    expect(isJsonContentType("text/html")).toBe(false);
    expect(isJsonContentType(null)).toBe(false);
    expect(decodeHealthV1('{"healthy":true,"version":"1.2.3"}')).toBe("1.2.3");
    expect(decodeHealthV1('{"healthy":false,"version":"1.2.3"}')).toBeNull();
    expect(decodeHealthV1('{"version":"1.2.3"}')).toBeNull();
    expect(decodeHealthV1("not json")).toBeNull();
    expect(decodeInfoV2('{"version":"2.0.1","pid":7}')).toBe("2.0.1");
    expect(decodeInfoV2('{"version":"2.0.1"}')).toBeNull();
    expect(decodeInfoV2('{"healthy":true,"version":"1.2.3"}')).toBeNull();
  });

  it("discover prefers the manual URL, falls back to the default, or gives up", async () => {
    const both = healthFetch(200);
    const found = await discoverOpencode("http://localhost:9999", both);
    expect(found.server?.base).toBe("http://localhost:9999");
    expect(found.server?.kind).toBe("v1");
    expect(found.unauthorizedBase).toBeNull();
    expect(both.calls[0].url).toBe("http://localhost:9999/api/info");

    // Manual fails, default answers.
    const mixed: FetchLike = (async (input: RequestInfo | URL) => {
      const url =
        typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      if (url.startsWith("http://localhost:9999/")) throw new TypeError("refused");
      return jsonResponse(200, { healthy: true, version: "1.18.35" });
    }) as FetchLike;
    const fallback = await discoverOpencode("http://localhost:9999", mixed);
    expect(fallback.server?.base).toBe("http://127.0.0.1:4096");

    const none = healthFetch(0, { fail: true });
    await expect(discoverOpencode(undefined, none)).resolves.toEqual({
      server: null,
      unauthorizedBase: null,
    });
  });

  it("discover reports a guarded server as unauthorized instead of absent", async () => {
    const guarded: FetchLike = (async () => new Response("no", { status: 401 })) as FetchLike;
    const result = await discoverOpencode("http://localhost:9999", guarded);
    expect(result.server).toBeNull();
    expect(result.unauthorizedBase).toBe("http://localhost:9999");
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
  });

  it("check() reports down when the endpoint never identifies as opencode", async () => {
    // A 500 (or an unrelated JSON/HTML service) is NOT an opencode server —
    // t3-style strictness: only a decodable health/info body counts.
    const provider = makeProvider(healthFetch(500));
    const result = await provider.check();
    expect(result.status).toBe("down");
    expect(result.error).toContain("no opencode server");
  });

  it("check() flags a 2.x server as unavailable with actionable copy", async () => {
    const provider = makeProvider(healthFetch(200, { v2: true }));
    const result = await provider.check();
    expect(result.status).toBe("down");
    expect(result.error).toContain("2.x");
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

// ---------------------------------------------------------------------------
// Managed mode (M12): app-owned server resolution
// ---------------------------------------------------------------------------

/** Injectable ManagedServe double recording start/status calls. */
function serveMock(
  statusResult: { running: boolean; url: string | null; error: string | null },
  startResult?: { running: boolean; url: string | null; error: string | null },
) {
  return {
    status: vi.fn(async () => statusResult),
    start: vi.fn(async () => startResult ?? statusResult),
  };
}

describe("OpenCodeProvider managed mode", () => {
  it("reuses the running managed server without starting it", async () => {
    const fetchMock = healthFetch(200);
    const serve = serveMock({ running: true, url: "http://127.0.0.1:51777", error: null });
    const provider = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => serve,
      fetchImpl: fetchMock,
    });
    const result = await provider.check();
    expect(result.status).to.equal("ok");
    expect(serve.start).not.toHaveBeenCalled();
    // Probe order: /api/info (HTML on 1.x) then /global/health (JSON).
    expect(fetchMock.calls[1]?.url).to.equal("http://127.0.0.1:51777/global/health");
  });

  it("lazily starts the managed server when none is running", async () => {
    const fetchMock = healthFetch(200);
    const serve = serveMock(
      { running: false, url: null, error: null },
      { running: true, url: "http://127.0.0.1:52000", error: null },
    );
    const provider = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => serve,
      fetchImpl: fetchMock,
    });
    const result = await provider.check();
    expect(serve.start).toHaveBeenCalledTimes(1);
    expect(result.status).to.equal("ok");
    expect(fetchMock.calls[1]?.url).to.equal("http://127.0.0.1:52000/global/health");
  });

  it("maps a guarded managed server to unauthenticated", async () => {
    const serve = serveMock({ running: true, url: "http://127.0.0.1:51999", error: null });
    const provider = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => serve,
      fetchImpl: healthFetch(401),
    });
    const result = await provider.check();
    expect(result.status).to.equal("unauthenticated");
    expect(result.error).to.contain("password");
  });

  it("flags a managed 2.x server as unavailable (this build speaks 1.x)", async () => {
    const serve = serveMock({ running: true, url: "http://127.0.0.1:51998", error: null });
    const provider = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => serve,
      fetchImpl: healthFetch(200, { v2: true }),
    });
    const result = await provider.check();
    expect(result.status).to.equal("down");
    expect(result.error).to.contain("2.x");
    expect(result.error).to.contain("1.x");
  });

  it("surfaces the start failure copy (CLI missing)", async () => {
    const serve = serveMock(
      { running: false, url: null, error: null },
      {
        running: false,
        url: null,
        error: "opencode not found on PATH — install opencode or pick another backend in AI settings",
      },
    );
    const provider = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => serve,
      fetchImpl: healthFetch(200),
    });
    const result = await provider.check();
    expect(result.status).to.equal("down");
    expect(result.error).to.contain("not found on PATH");
    await expect(provider.generate({ prompt: "hi" })).rejects.toMatchObject({
      kind: "unavailable",
    });
  });

  it("manages models through the managed server URL", async () => {
    FAKE_CLIENT.config.providers.mockResolvedValue({ data: PROVIDERS_REPLY });
    const serve = serveMock({ running: true, url: "http://127.0.0.1:51777", error: null });
    const provider = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => serve,
      fetchImpl: healthFetch(200),
    });
    const models = await provider.listModels();
    expect(models.map((m) => m.id)).to.include("anthropic/claude-sonnet-4-5");
    expect(createOpencodeClient).toHaveBeenLastCalledWith(
      expect.objectContaining({ baseUrl: "http://127.0.0.1:51777" }),
    );
  });

  it("fails fast when disabled — nothing probed, nothing spawned", async () => {
    const fetchMock = healthFetch(200);
    const serve = serveMock({ running: true, url: "http://127.0.0.1:51777", error: null });
    const provider = new OpenCodeProvider({
      resolve: () => "disabled",
      managed: () => serve,
      fetchImpl: fetchMock,
    });
    const result = await provider.check();
    expect(result.status).to.equal("down");
    expect(result.error).to.contain("disabled in AI settings");
    expect(fetchMock.calls).to.have.length(0);
    expect(serve.status).not.toHaveBeenCalled();
    await expect(provider.generate({ prompt: "hi" })).rejects.toMatchObject({
      kind: "unavailable",
    });
  });

  it("managed without a bridge explains the desktop-app requirement", async () => {
    const provider = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => null,
      fetchImpl: healthFetch(200),
    });
    const result = await provider.check();
    expect(result.status).to.equal("down");
    expect(result.error).to.contain("desktop app");
  });
});
