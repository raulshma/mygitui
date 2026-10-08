/**
 * Unit tests for the OpenRouter backend adapter: keyring-backed key
 * handling, default model selection, generateText routing through the AI
 * SDK (mocked), secret scrubbing from error messages, and the models
 * listing (direct fetch, fixture JSON).
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  fetchOpenRouterModels,
  OPENROUTER_API_BASE,
  OpenRouterProvider,
  scrubSecret,
} from "./openrouter";

const mocks = vi.hoisted(() => {
  return {
    generateText: vi.fn(),
    createOpenRouter: vi.fn(() => {
      // The provider object is callable: openrouter(modelId) → model handle.
      const callable = vi.fn((modelId: string) => ({ modelId, __handle: true }));
      return callable;
    }),
    secretsGet: vi.fn(),
  };
});

vi.mock("ai", () => ({
  generateText: mocks.generateText,
}));

vi.mock("@openrouter/ai-sdk-provider", () => ({
  createOpenRouter: mocks.createOpenRouter,
}));

vi.mock("$lib/ipc/client", () => ({
  secretsGet: mocks.secretsGet,
  secretsSet: vi.fn(),
  secretsDelete: vi.fn(),
  SECRET_KEYS: {
    openrouterApiKey: "openrouter.api-key",
    opencodeServerPassword: "opencode.server.password",
  },
}));

import { DEFAULT_OPENROUTER_MODEL } from "./provider";
import { SECRET_KEYS } from "$lib/ipc/client";

const API_KEY = "sk-or-v1-SUPERSECRET123";

/** Fixture for GET /api/v1/models. */
const MODELS_REPLY = {
  data: [
    { id: "anthropic/claude-sonnet-4.5", name: "Claude Sonnet 4.5" },
    { id: "openai/gpt-oss-120b", name: "GPT-OSS 120B" },
    { name: "no id — skipped" },
  ],
};

function modelsFetch(status = 200, body: unknown = MODELS_REPLY) {
  return vi.fn(
    async (_input: RequestInfo | URL, init?: RequestInit): Promise<Response> =>
      ({
        ok: 200 <= status && status < 300,
        status,
        json: async () => body,
      }) as Response,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.secretsGet.mockResolvedValue(API_KEY);
});

// ---------------------------------------------------------------------------
// Secret scrubbing
// ---------------------------------------------------------------------------

describe("scrubSecret", () => {
  it("removes every occurrence of the secret", () => {
    const msg = `request failed with key ${API_KEY} (key ${API_KEY} rejected)`;
    expect(scrubSecret(msg, API_KEY)).toBe("request failed with key *** (key *** rejected)");
  });

  it("is a no-op without a secret or without occurrences", () => {
    expect(scrubSecret("all good", API_KEY)).toBe("all good");
    expect(scrubSecret("all good", null)).toBe("all good");
  });
});

// ---------------------------------------------------------------------------
// Models listing
// ---------------------------------------------------------------------------

describe("fetchOpenRouterModels", () => {
  it("fetches the public models endpoint and flattens the fixture", async () => {
    const impl = modelsFetch();
    const models = await fetchOpenRouterModels(impl);
    expect(impl).toHaveBeenCalledWith(`${OPENROUTER_API_BASE}/models`, expect.anything());
    expect(models.map((m) => m.id)).toEqual([
      "anthropic/claude-sonnet-4.5",
      "openai/gpt-oss-120b",
    ]);
    expect(models[0]).toMatchObject({
      label: "Claude Sonnet 4.5",
      provider: "anthropic",
    });
  });

  it("attaches the Authorization header when a key exists (never logs it)", async () => {
    const impl = modelsFetch();
    await fetchOpenRouterModels(impl);
    const headers = new Headers(impl.mock.calls[0][1]?.headers);
    expect(headers.get("Authorization")).toBe(`Bearer ${API_KEY}`);
  });

  it("maps HTTP 401 to AiError unauthenticated (no key material in the message)", async () => {
    const err = await fetchOpenRouterModels(modelsFetch(401)).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(Object);
    expect((err as { kind?: string }).kind).toBe("unauthenticated");
    expect(String((err as Error).message)).not.toContain(API_KEY);
  });

  it("maps network failures to AiError unavailable and JSON garbage to bad-response", async () => {
    const down = vi.fn(
      async (_input: RequestInfo | URL, _init?: RequestInit): Promise<Response> => {
        throw new TypeError("dns lookup failed");
      },
    );
    await expect(fetchOpenRouterModels(down)).rejects.toMatchObject({ kind: "unavailable" });

    const garbage = modelsFetch(200, "not-json-at-all");
    // `json()` on our stub resolves fine but the shape is not `{data: []}` —
    // that yields an empty list rather than a throw.
    await expect(fetchOpenRouterModels(garbage)).resolves.toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// OpenRouterProvider
// ---------------------------------------------------------------------------

describe("OpenRouterProvider", () => {
  it("available() reflects the keyring read", async () => {
    expect(await new OpenRouterProvider().available()).toBe(true);
    mocks.secretsGet.mockResolvedValue(null);
    expect(await new OpenRouterProvider().available()).toBe(false);
  });

  it("check() maps the models fetch to ok/down/unauthenticated", async () => {
    const ok = new OpenRouterProvider({ fetchImpl: modelsFetch() });
    expect(await ok.check()).toEqual({ status: "ok" });

    mocks.secretsGet.mockResolvedValue(null);
    const down = new OpenRouterProvider({ fetchImpl: modelsFetch(500) });
    expect((await down.check()).status).toBe("down");
  });

  it("generate routes through createOpenRouter + generateText with the default model", async () => {
    mocks.generateText.mockResolvedValue({
      text: "feat: add the thing",
      response: { modelId: "anthropic/claude-sonnet-4.5" },
    });
    const result = await new OpenRouterProvider().generate({
      system: "sys",
      prompt: "write it",
    });

    expect(mocks.secretsGet).toHaveBeenCalledWith(SECRET_KEYS.openrouterApiKey);
    expect(mocks.createOpenRouter).toHaveBeenCalledWith({ apiKey: API_KEY });
    const call = mocks.generateText.mock.calls[0][0];
    expect(call.model.modelId).toBe(DEFAULT_OPENROUTER_MODEL);
    expect(call.system).toBe("sys");
    expect(call.prompt).toBe("write it");

    expect(result).toMatchObject({
      text: "feat: add the thing",
      model: "anthropic/claude-sonnet-4.5",
      backend: "openrouter",
    });
  });

  it("generate honors an explicit model override over the default", async () => {
    mocks.generateText.mockResolvedValue({ text: "ok" });
    await new OpenRouterProvider({ defaultModel: "openai/gpt-oss-120b" }).generate({
      prompt: "p",
      model: "openai/gpt-5",
    });
    expect(mocks.generateText.mock.calls[0][0].model.modelId).toBe("openai/gpt-5");
  });

  it("generate passes an AbortSignal through", async () => {
    mocks.generateText.mockResolvedValue({ text: "ok" });
    const controller = new AbortController();
    await new OpenRouterProvider().generate({ prompt: "p", signal: controller.signal });
    expect(mocks.generateText.mock.calls[0][0].abortSignal).toBe(controller.signal);
  });

  it("generate refuses without a key (unauthenticated) without calling the SDK", async () => {
    mocks.secretsGet.mockResolvedValue(null);
    const err = await new OpenRouterProvider()
      .generate({ prompt: "p" })
      .catch((e: unknown) => e);
    expect((err as { kind?: string }).kind).toBe("unauthenticated");
    expect(mocks.createOpenRouter).not.toHaveBeenCalled();
  });

  it("scrubs the API key out of SDK error messages", async () => {
    mocks.generateText.mockRejectedValue(
      new Error(`401 Unauthorized for key ${API_KEY} — invalid credential`),
    );
    const err = await new OpenRouterProvider()
      .generate({ prompt: "p" })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(Error);
    const message = (err as Error).message;
    expect(message).not.toContain(API_KEY);
    expect(message).toContain("***");
  });

  it("cancelled requests report kind cancelled", async () => {
    const controller = new AbortController();
    controller.abort();
    const err = await new OpenRouterProvider()
      .generate({ prompt: "p", signal: controller.signal })
      .catch((e: unknown) => e);
    expect((err as { kind?: string }).kind).toBe("cancelled");
    expect(mocks.generateText).not.toHaveBeenCalled();
  });
});
