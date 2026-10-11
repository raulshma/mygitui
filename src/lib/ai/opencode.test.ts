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
  normalizeDirectory,
  OpenCodeProvider,
  parseModelSlug,
  partsToText,
  probeOpencodeServer,
  providersToModels,
  SESSION_TITLE,
  v2ErrorToAiError,
  v2ModelsToInfos,
  V2_MODEL_LIST_RETRY_MS,
  V2_MODEL_LIST_TIMEOUT_MS,
  withTimeout,
  __resetOpencodeClientFactories,
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

// ---------------------------------------------------------------------------
// @opencode/client (v2 promise client) mock — fake per-test via FAKE_V2.
// ---------------------------------------------------------------------------

const FAKE_V2 = vi.hoisted(() => {
  const client = {
    server: { info: vi.fn() },
    model: { list: vi.fn() },
    provider: { list: vi.fn() },
    session: {
      create: vi.fn(),
      prompt: vi.fn(),
      remove: vi.fn(),
      interrupt: vi.fn(),
    },
    permission: { reply: vi.fn() },
    event: { subscribe: vi.fn() },
  };
  return client;
});

vi.mock("@opencode/client", () => ({
  OpenCode: { make: vi.fn(() => FAKE_V2) },
}));

import { createOpencodeClient } from "@opencode-ai/sdk/client";
import { OpenCode as OpenCodeV2 } from "@opencode/client";

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
      // HTML fallback (web UI), like a real 1.x server. A guarded opencode
      // answers 401/403 as JSON, so error statuses stay JSON here.
      if (status === 401 || status === 403) {
        return jsonResponse(status, { error: "unauthorized" });
      }
      return htmlResponse(status);
    }
    if (status === 200) {
      return jsonResponse(200, { healthy: true, version: "1.18.35" });
    }
    if (status === 401 || status === 403) {
      return jsonResponse(status, { error: "unauthorized" });
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
  __resetOpencodeClientFactories();
  FAKE_CLIENT.session.create.mockReset();
  FAKE_CLIENT.session.prompt.mockReset();
  FAKE_CLIENT.config.providers.mockReset();
  FAKE_V2.server.info.mockReset();
  FAKE_V2.model.list.mockReset();
  FAKE_V2.provider.list.mockReset();
  FAKE_V2.session.create.mockReset();
  FAKE_V2.session.prompt.mockReset();
  FAKE_V2.session.remove.mockReset();
  FAKE_V2.session.interrupt.mockReset();
  FAKE_V2.permission.reply.mockReset();
  FAKE_V2.event.subscribe.mockReset();
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

  it("ignores non-JSON 401/403 (unrelated guarded service is not opencode)", async () => {
    const guardedHtml: FetchLike = (async () =>
      new Response("<html>login</html>", {
        status: 401,
        headers: { "content-type": "text/html" },
      })) as FetchLike;
    await expect(probeOpencodeServer("http://127.0.0.1:4096", guardedHtml)).resolves.toMatchObject({
      state: "absent",
    });
    const guardedPlain: FetchLike = (async () => new Response("no", { status: 403 })) as FetchLike;
    await expect(probeOpencodeServer("http://127.0.0.1:4096", guardedPlain)).resolves.toMatchObject({
      state: "absent",
    });
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
    const guarded: FetchLike = (async () =>
      new Response(JSON.stringify({ error: "unauthorized" }), {
        status: 401,
        headers: { "content-type": "application/json" },
      })) as FetchLike;
    const result = await discoverOpencode("http://localhost:9999", guarded);
    expect(result.server).toBeNull();
    expect(result.unauthorizedBase).toBe("http://localhost:9999");
  });

  it("discover skips a non-JSON guarded service (absent, not unauthorized)", async () => {
    const guarded: FetchLike = (async () => new Response("no", { status: 401 })) as FetchLike;
    const result = await discoverOpencode("http://localhost:9999", guarded);
    expect(result.server).toBeNull();
    expect(result.unauthorizedBase).toBeNull();
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

  it("check() accepts a 2.x server (both generations supported)", async () => {
    const provider = makeProvider(healthFetch(200, { v2: true }));
    const result = await provider.check();
    expect(result.status).toBe("ok");
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
  statusResult: { running: boolean; url: string | null; error: string | null; password?: string | null },
  startResult?: { running: boolean; url: string | null; error: string | null; password?: string | null },
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

  it("accepts a managed 2.x server (both generations supported)", async () => {
    const serve = serveMock({ running: true, url: "http://127.0.0.1:51998", error: null });
    const provider = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => serve,
      fetchImpl: healthFetch(200, { v2: true }),
    });
    const result = await provider.check();
    expect(result.status).to.equal("ok");
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

// ---------------------------------------------------------------------------
// Windows directory normalization + error surfacing
// ---------------------------------------------------------------------------

describe("normalizeDirectory", () => {
  it("converts backslash Windows paths to forward slashes", () => {
    expect(normalizeDirectory("C:\\Code\\Projects\\mygitui")).toBe(
      "C:/Code/Projects/mygitui",
    );
  });

  it("strips the Rust canonicalize \\\\?\\ prefix before normalizing", () => {
    expect(normalizeDirectory("\\\\?\\C:\\Code\\repo")).toBe("C:/Code/repo");
  });

  it("leaves forward-slash and POSIX paths untouched", () => {
    expect(normalizeDirectory("C:/Code/repo")).toBe("C:/Code/repo");
    expect(normalizeDirectory("/home/user/repo")).toBe("/home/user/repo");
  });

  it("returns undefined for empty/absent input", () => {
    expect(normalizeDirectory(undefined)).toBeUndefined();
    expect(normalizeDirectory(null)).toBeUndefined();
    expect(normalizeDirectory("")).toBeUndefined();
  });
});

describe("directory header + error surfacing in generate", () => {
  it("passes a forward-slash directory to the SDK (backslashes break 1.x servers)", async () => {
    FAKE_CLIENT.session.create.mockResolvedValue({ data: SESSION, error: undefined });
    FAKE_CLIENT.session.prompt.mockResolvedValue({
      data: { parts: PROMPT_PARTS },
      error: undefined,
    });
    // The live store maps sessionKey → tab root, which on Windows is a
    // backslash path straight from git2.
    const providerWithDir = new OpenCodeProvider({
      fetchImpl: healthFetch(200),
      directory: () => "C:\\Code\\Projects\\mygitui",
    });
    await providerWithDir.generate({ prompt: "hello", sessionKey: "repo-1" });
    expect(createOpencodeClient).toHaveBeenLastCalledWith(
      expect.objectContaining({ directory: "C:/Code/Projects/mygitui" }),
    );
  });

  it("summarizes NamedError POJOs ({name, data:{message, ref}}) instead of 'unknown server error'", async () => {
    FAKE_CLIENT.session.create.mockResolvedValue({ data: SESSION, error: undefined });
    FAKE_CLIENT.session.prompt.mockResolvedValue({
      data: undefined,
      error: { name: "UnknownError", data: { message: "Unexpected server error.", ref: "err_cbdb30af" } },
    });
    const err = await makeProvider()
      .generate({ prompt: "hello", sessionKey: "repo-5" })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).message).toContain("Unexpected server error.");
    expect((err as AiError).message).toContain("UnknownError");
    expect((err as AiError).message).toContain("err_cbdb30af");
  });

  it("surfaces info.error from a textless 200 reply (provider auth failure)", async () => {
    FAKE_CLIENT.session.create.mockResolvedValue({ data: SESSION, error: undefined });
    FAKE_CLIENT.session.prompt.mockResolvedValue({
      data: {
        info: {
          error: {
            name: "ProviderAuthError",
            data: { providerID: "anthropic", message: "no API key configured" },
          },
        },
        parts: [{ type: "step-start" }],
      },
      error: undefined,
    });
    const err = await makeProvider()
      .generate({ prompt: "hello", sessionKey: "repo-6" })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).kind).toBe("unauthenticated");
    expect((err as AiError).message).toContain("no API key configured");
    expect((err as AiError).message).toContain("AI settings");
  });

  it("keeps the generic no-text error when info carries no error", async () => {
    FAKE_CLIENT.session.create.mockResolvedValue({ data: SESSION, error: undefined });
    FAKE_CLIENT.session.prompt.mockResolvedValue({
      data: { info: {}, parts: [] },
      error: undefined,
    });
    await expect(
      makeProvider().generate({ prompt: "hello", sessionKey: "repo-7" }),
    ).rejects.toMatchObject({
      kind: "bad-response",
      message: expect.stringContaining("no text output"),
    });
  });
});

// ---------------------------------------------------------------------------
// v2 helpers (t3-style runtime routing)
// ---------------------------------------------------------------------------

describe("parseModelSlug / v2ModelsToInfos", () => {
  it("splits provider/model slugs", () => {
    expect(parseModelSlug("anthropic/claude-sonnet-4-5")).toEqual({
      providerID: "anthropic",
      modelID: "claude-sonnet-4-5",
    });
    expect(parseModelSlug("openai/gpt-5/mini")).toEqual({
      providerID: "openai",
      modelID: "gpt-5/mini",
    });
  });

  it("rejects malformed slugs", () => {
    expect(parseModelSlug(undefined)).toBeNull();
    expect(parseModelSlug("no-slash")).toBeNull();
    expect(parseModelSlug("/leading")).toBeNull();
    expect(parseModelSlug("trailing/")).toBeNull();
    expect(parseModelSlug("  ")).toBeNull();
  });

  it("flattens v2 model.list entries, decorating with provider names", () => {
    expect(
      v2ModelsToInfos(
        [
          { providerID: "anthropic", id: "claude-sonnet-4-5", name: "Claude Sonnet 4.5" },
          { providerID: "opencode", id: "big-pickle", name: "Big Pickle" },
          { providerID: "", id: "orphan" },
          { providerID: "x" },
        ],
        new Map([["anthropic", "Anthropic"]]),
      ),
    ).toEqual([
      {
        id: "anthropic/claude-sonnet-4-5",
        provider: "anthropic",
        label: "Claude Sonnet 4.5 (anthropic/claude-sonnet-4-5)",
      },
      { id: "opencode/big-pickle", provider: "opencode", label: "opencode/big-pickle" },
    ]);
  });

  it("tolerates empty input", () => {
    expect(v2ModelsToInfos([])).toEqual([]);
  });
});

describe("v2ErrorToAiError / withTimeout", () => {
  it("maps 401/UnauthorizedError to unauthenticated", () => {
    expect(v2ErrorToAiError({ name: "UnauthorizedError" }, "model.list")).toMatchObject({
      kind: "unauthenticated",
    });
    expect(v2ErrorToAiError({ status: 401 }, "session.create")).toMatchObject({
      kind: "unauthenticated",
    });
    expect(v2ErrorToAiError(new AiError("bad-response", "x", {}) as unknown, "y")).toMatchObject({
      kind: "bad-response",
    });
  });

  it("maps transport failures to unavailable", () => {
    expect(v2ErrorToAiError(new Error("fetch failed"), "model.list")).toMatchObject({
      kind: "unavailable",
    });
  });

  it("maps other failures to bad-response naming the operation", () => {
    const err = v2ErrorToAiError(new Error("boom"), "session.prompt");
    expect(err.kind).toBe("bad-response");
    expect(err.message).toContain("session.prompt");
  });

  it("withTimeout resolves before the deadline", async () => {
    await expect(withTimeout(Promise.resolve("ok"), 1000)).resolves.toBe("ok");
  });

  it("withTimeout rejects after the deadline and runs onTimeout", async () => {
    const onTimeout = vi.fn();
    await expect(
      withTimeout(new Promise(() => {}), 10, onTimeout),
    ).rejects.toMatchObject({ kind: "unavailable" });
    expect(onTimeout).toHaveBeenCalledTimes(1);
  });
});

// ---------------------------------------------------------------------------
// v2 provider paths (promise client mock)
// ---------------------------------------------------------------------------

/** One v2 SSE text part + execution end, optionally with a permission ask. */
function v2Events(options?: { askPermission?: boolean }) {
  const events: Array<{ type: string; data: unknown }> = [];
  if (options?.askPermission) {
    events.push({
      type: "permission.asked",
      data: { id: "req-1", sessionID: "sess-v2", action: "shell", resources: [] },
    });
  }
  events.push(
    {
      type: "session.text.ended",
      data: { sessionID: "sess-v2", assistantMessageID: "msg-1", ordinal: 0, text: "v2 says hi" },
    },
    { type: "session.execution.succeeded", data: { sessionID: "sess-v2" } },
  );
  return (async function* () {
    for (const event of events) yield event;
  })();
}

function makeV2Provider(fetchImpl?: FetchLike, password?: string): OpenCodeProvider {
  return new OpenCodeProvider({
    fetchImpl: fetchImpl ?? healthFetch(200, { v2: true }),
    ...(password !== undefined ? { password: async () => password } : {}),
  });
}

describe("OpenCodeProvider v2", () => {
  it("listModels reads model.list + provider.list via the v2 client", async () => {
    FAKE_V2.model.list.mockResolvedValue({
      location: { directory: "C:/repo" },
      data: [
        { providerID: "anthropic", id: "claude-sonnet-4-5", name: "Claude Sonnet 4.5" },
        { providerID: "opencode", id: "big-pickle", name: "Big Pickle" },
      ],
    });
    FAKE_V2.provider.list.mockResolvedValue({
      location: { directory: "C:/repo" },
      data: [{ id: "anthropic", name: "Anthropic" }],
    });
    const models = await makeV2Provider().listModels();
    expect(models).toEqual([
      {
        id: "anthropic/claude-sonnet-4-5",
        provider: "anthropic",
        label: "Claude Sonnet 4.5 (anthropic/claude-sonnet-4-5)",
      },
      { id: "opencode/big-pickle", provider: "opencode", label: "opencode/big-pickle" },
    ]);
    expect(OpenCodeV2.make).toHaveBeenCalledWith(
      expect.objectContaining({ baseUrl: "http://127.0.0.1:4096" }),
    );
    expect(FAKE_V2.model.list).toHaveBeenCalledTimes(1);
  });

  it("listModels still lists models when provider.list fails", async () => {
    FAKE_V2.model.list.mockResolvedValue({
      location: { directory: "C:/repo" },
      data: [{ providerID: "a", id: "m", name: "M" }],
    });
    FAKE_V2.provider.list.mockRejectedValue(new Error("slow"));
    const models = await makeV2Provider().listModels();
    expect(models).toEqual([{ id: "a/m", provider: "a", label: "a/m" }]);
  });

  it("re-reads an empty v2 model list until models arrive (t3 lazy catalog)", async () => {
    vi.useFakeTimers();
    try {
      FAKE_V2.model.list
        .mockResolvedValueOnce({ data: [] })
        .mockResolvedValueOnce({
          data: [{ providerID: "a", id: "m", name: "M" }],
        });
      FAKE_V2.provider.list.mockResolvedValue({ data: [] });
      const pending = makeV2Provider().listModels();
      await vi.advanceTimersByTimeAsync(V2_MODEL_LIST_RETRY_MS + 50);
      await expect(pending).resolves.toEqual([{ id: "a/m", provider: "a", label: "a/m" }]);
      expect(FAKE_V2.model.list).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the last non-empty v2 list when the catalog stays empty", async () => {
    vi.useFakeTimers();
    try {
      FAKE_V2.model.list.mockResolvedValue({
        data: [{ providerID: "a", id: "m", name: "M" }],
      });
      FAKE_V2.provider.list.mockResolvedValue({ data: [] });
      const provider = makeV2Provider();
      await expect(provider.listModels()).resolves.toEqual([
        { id: "a/m", provider: "a", label: "a/m" },
      ]);
      FAKE_V2.model.list.mockResolvedValue({ data: [] });
      const pending = provider.listModels();
      await vi.advanceTimersByTimeAsync(V2_MODEL_LIST_TIMEOUT_MS + 1_000);
      await expect(pending).resolves.toEqual([{ id: "a/m", provider: "a", label: "a/m" }]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("listModels maps v2 client errors to AiError", async () => {
    FAKE_V2.model.list.mockRejectedValue({ name: "UnauthorizedError" });
    const err = await makeV2Provider().listModels().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).kind).toBe("unauthenticated");
  });

  it("generate prompts in a temp session and waits on the event stream", async () => {
    FAKE_V2.session.create.mockResolvedValue({ id: "sess-v2" });
    FAKE_V2.session.prompt.mockResolvedValue({ id: "msg-1" });
    FAKE_V2.session.remove.mockResolvedValue(undefined);
    FAKE_V2.event.subscribe.mockReturnValue(v2Events());

    const result = await makeV2Provider().generate({
      system: "sys",
      prompt: "hello",
      model: "anthropic/claude-sonnet-4-5",
      sessionKey: "repo-v2",
    });
    expect(result).toMatchObject({ text: "v2 says hi", backend: "opencode" });
    expect(FAKE_V2.session.create).toHaveBeenCalledWith(
      expect.objectContaining({
        title: SESSION_TITLE,
        model: { providerID: "anthropic", id: "claude-sonnet-4-5" },
      }),
    );
    // system folds into the prompt text (v2 has no system field).
    expect(FAKE_V2.session.prompt).toHaveBeenCalledWith(
      expect.objectContaining({ sessionID: "sess-v2", text: "sys\n\nhello" }),
      expect.anything(),
    );
    expect(FAKE_V2.session.remove).toHaveBeenCalledWith({ sessionID: "sess-v2" });
  });

  it("generate rejects permission asks and uses the server default model when unset", async () => {
    FAKE_V2.session.create.mockResolvedValue({ id: "sess-v2" });
    FAKE_V2.session.prompt.mockResolvedValue({ id: "msg-1" });
    FAKE_V2.session.remove.mockResolvedValue(undefined);
    FAKE_V2.permission.reply.mockResolvedValue(undefined);
    FAKE_V2.event.subscribe.mockReturnValue(v2Events({ askPermission: true }));

    const result = await makeV2Provider().generate({ prompt: "hi" });
    expect(result.text).toBe("v2 says hi");
    expect(FAKE_V2.permission.reply).toHaveBeenCalledWith(
      expect.objectContaining({ decision: "reject" }),
    );
    const createArg = FAKE_V2.session.create.mock.calls[0][0];
    expect(createArg).not.toHaveProperty("model");
  });

  it("generate rejects malformed model slugs", async () => {
    const err = await makeV2Provider()
      .generate({ prompt: "hi", model: "no-slash" })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).kind).toBe("bad-response");
    expect((err as AiError).message).toContain("provider/model");
    expect(FAKE_V2.session.create).not.toHaveBeenCalled();
  });

  it("generate surfaces execution failures and cleans up the session", async () => {
    FAKE_V2.session.create.mockResolvedValue({ id: "sess-v2" });
    FAKE_V2.session.prompt.mockResolvedValue({ id: "msg-1" });
    FAKE_V2.session.remove.mockResolvedValue(undefined);
    FAKE_V2.event.subscribe.mockReturnValue(
      (async function* () {
        yield {
          type: "session.execution.failed",
          data: { sessionID: "sess-v2", error: { message: "no auth" } },
        };
      })(),
    );
    const err = await makeV2Provider()
      .generate({ prompt: "hi" })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).kind).toBe("bad-response");
    expect(FAKE_V2.session.remove).toHaveBeenCalledWith({ sessionID: "sess-v2" });
  });

  it("generate maps session.create auth failures to unauthenticated", async () => {
    FAKE_V2.session.create.mockRejectedValue({ name: "UnauthorizedError" });
    const err = await makeV2Provider().generate({ prompt: "hi" }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AiError);
    expect((err as AiError).kind).toBe("unauthenticated");
  });

  it("connection() returns the whole resolved server (base, built auth header, kind)", async () => {
    const conn = await makeV2Provider(healthFetch(200, { v2: true }), "pw").connection();
    expect(conn).toEqual({
      baseUrl: "http://127.0.0.1:4096",
      authHeader: `Basic ${btoa("opencode:pw")}`,
      serverKind: "v2",
    });
    // v1 servers carry no header when no password is configured.
    const v1 = await makeProvider(healthFetch(200)).connection();
    expect(v1).toEqual({
      baseUrl: "http://127.0.0.1:4096",
      authHeader: null,
      serverKind: "v1",
    });
    // Down: the AiError propagates (callers re-attempt per reconnection).
    await expect(
      makeV2Provider(healthFetch(0, { fail: true })).connection(),
    ).rejects.toBeInstanceOf(AiError);
  });

  it("managed mode prefers the generated 2.x password over the keyring", async () => {
    FAKE_V2.model.list.mockResolvedValue({ location: { directory: "x" }, data: [] });
    FAKE_V2.provider.list.mockResolvedValue({ location: { directory: "x" }, data: [] });
    const serve = serveMock({
      running: true,
      url: "http://127.0.0.1:51998",
      error: null,
      password: "managed-pw",
    });
    const provider = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => serve,
      password: async () => "keyring-pw",
      fetchImpl: healthFetch(200, { v2: true }),
    });
    const conn = await provider.connection();
    expect(conn).toEqual({
      baseUrl: "http://127.0.0.1:51998",
      authHeader: `Basic ${btoa("opencode:managed-pw")}`,
      serverKind: "v2",
    });
  });
});

// ---------------------------------------------------------------------------
// Provider-owned SSE liveness (subscribeLiveness)
// ---------------------------------------------------------------------------

/** An abort-aware SSE response: the stream errors when the signal fires
 * (what a real fetch's body does), so stop() surfaces as an ended stream. */
function sseResponse(frames: string[], signal?: AbortSignal | null): Response {
  const encoder = new TextEncoder();
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const frame of frames) controller.enqueue(encoder.encode(frame));
      signal?.addEventListener("abort", () =>
        controller.error(new DOMException("aborted", "AbortError")),
      );
    },
  });
  return new Response(stream, { status: 200 });
}

describe("OpenCodeProvider.subscribeLiveness", () => {
  it("streams a 1.x server at /event: handler(false) per event, handler(true) on stop", async () => {
    let call = 0;
    const urls: string[] = [];
    const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
      urls.push(String(input));
      call += 1;
      if (call <= 2) return jsonResponse(200, { healthy: true, version: "1.18.35" });
      return sseResponse(
        ['data: {"type":"session.updated","properties":{}}\n\n'],
        init?.signal,
      );
    }) as unknown as FetchLike;

    const signals: boolean[] = [];
    const provider = new OpenCodeProvider({ fetchImpl });
    const unsub = provider.subscribeLiveness((down) => signals.push(down));
    await vi.waitFor(() => expect(signals).toEqual([false]));
    // The path choice is the provider's: 1.x → /event.
    expect(urls[urls.length - 1]).toBe("http://127.0.0.1:4096/event");
    unsub();
    await vi.waitFor(() => expect(signals).toEqual([false, true]));
  });

  it("streams a 2.x server at /api/event and carries the built auth header", async () => {
    const inits: RequestInit[] = [];
    let call = 0;
    const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
      void input;
      inits.push(init ?? {});
      call += 1;
      if (call === 1) return jsonResponse(200, { version: "2.0.18", pid: 9 });
      return sseResponse(['data: {"type":"v2.event","data":{}}\n\n'], init?.signal);
    }) as unknown as FetchLike;

    const signals: boolean[] = [];
    const provider = new OpenCodeProvider({
      fetchImpl,
      password: async () => "sekrit",
    });
    const unsub = provider.subscribeLiveness((down) => signals.push(down));
    await vi.waitFor(() => expect(signals).toEqual([false]));
    // 2.x → /api/event, with the basic-auth header derived by the provider.
    expect(inits.length).toBe(2);
    expect(new Headers(inits[1].headers).get("Authorization")).toBe(
      `Basic ${btoa("opencode:sekrit")}`,
    );
    unsub();
  });

  it("reports down when the connection cannot be resolved (no stream fetch)", async () => {
    const fetchImpl = (async () => {
      throw new TypeError("fetch failed");
    }) as unknown as FetchLike;
    const signals: boolean[] = [];
    const provider = new OpenCodeProvider({ fetchImpl });
    const unsub = provider.subscribeLiveness((down) => signals.push(down));
    await vi.waitFor(() => expect(signals).toEqual([true]));
    unsub();
  });

  it("resolves the connection FRESH per attempt (restart-rotated password re-derived)", async () => {
    let registry = {
      running: true,
      url: "http://127.0.0.1:53001",
      error: null as string | null,
      password: "generated-one",
    };
    const serve = {
      status: vi.fn(async () => registry),
      start: vi.fn(async () => registry),
    };
    const streamAuth: string[] = [];
    let call = 0;
    const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
      void input;
      call += 1;
      if (call % 2 === 1) {
        // Probe /api/info → identifies a 2.x managed server.
        return jsonResponse(200, { version: "2.0.18", pid: 9 });
      }
      streamAuth.push(new Headers(init?.headers).get("Authorization") ?? "");
      return sseResponse([], init?.signal);
    }) as unknown as FetchLike;

    const provider = new OpenCodeProvider({
      resolve: () => "managed",
      managed: () => serve as never,
      fetchImpl,
    });

    const first = provider.subscribeLiveness(() => {});
    await vi.waitFor(() => expect(streamAuth.length).toBe(1));
    first();

    // Restart: new port + newly generated password in the serve registry.
    registry = {
      running: true,
      url: "http://127.0.0.1:53002",
      error: null,
      password: "generated-two",
    };
    const second = provider.subscribeLiveness(() => {});
    await vi.waitFor(() => expect(streamAuth.length).toBe(2));
    second();

    expect(streamAuth[0]).toBe(`Basic ${btoa("opencode:generated-one")}`);
    expect(streamAuth[1]).toBe(`Basic ${btoa("opencode:generated-two")}`);
  });
});
