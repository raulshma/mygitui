/**
 * OpenRouter backend adapter (M6, lane H1).
 *
 * Direct OpenRouter API access with the user's API key (OS keyring,
 * `secretsGet("openrouter.api-key")`) through the Vercel AI SDK:
 * `createOpenRouter({ apiKey })` + `generateText`. The model list comes
 * from the public `GET https://openrouter.ai/api/v1/models` endpoint
 * (implemented as a plain fetch — the AI SDK provider exposes no listing).
 *
 * Secret hygiene: the API key is fetched lazily per call, never cached in
 * module state, never written to localStorage, and scrubbed from any error
 * message before it can reach a toast or log (see {@link scrubSecret}).
 */

import { createOpenRouter } from "@openrouter/ai-sdk-provider";
import { generateText } from "ai";
import { secretsGet, SECRET_KEYS } from "$lib/ipc/client";
import { AiError } from "./types";
import {
  DEFAULT_OPENROUTER_MODEL,
  type AiGenerateRequest,
  type AiProvider,
  type AiProbeResult,
} from "./provider";
import type { AiResult, ModelInfo } from "./types";

/** Public OpenRouter API base (models listing + generation). */
export const OPENROUTER_API_BASE = "https://openrouter.ai/api/v1";

/** Injectable fetch surface (subset of DOM fetch; tests stub globalThis). */
export type FetchLike = typeof fetch;

/** Removes every occurrence of `secret` from `text` (defensive scrubbing). */
export function scrubSecret(text: string, secret: string | null | undefined): string {
  if (!secret || secret.length === 0) return text;
  return text.split(secret).join("***");
}

/** Reads the OpenRouter API key from the OS keyring (lazy, per call). */
export function apiKey(): Promise<string | null> {
  return secretsGet(SECRET_KEYS.openrouterApiKey);
}

/** True when the models reply looks like OpenRouter's `{data: [...]}`. */
function modelListFromReply(reply: unknown): ModelInfo[] {
  const data = (reply as { data?: unknown } | null)?.data;
  if (!Array.isArray(data)) return [];
  const out: ModelInfo[] = [];
  for (const entry of data) {
    const id = (entry as { id?: unknown } | null)?.id;
    if (typeof id !== "string" || id.length === 0) continue;
    const name = (entry as { name?: unknown } | null)?.name;
    out.push({
      id,
      label: typeof name === "string" && name.length > 0 ? name : id,
      provider: id.includes("/") ? id.slice(0, id.indexOf("/")) : undefined,
    });
  }
  return out;
}

/**
 * Lists OpenRouter models via the public models endpoint (no key needed;
 * an `Authorization` header is attached when a key exists anyway). Throws
 * `AiError` (`unavailable` / `bad-response`) on failure.
 */
export async function fetchOpenRouterModels(
  fetchImpl: FetchLike = fetch,
): Promise<ModelInfo[]> {
  let response: Response;
  try {
    const key = await apiKey();
    response = await fetchImpl(`${OPENROUTER_API_BASE}/models`, {
      headers: key ? { Authorization: `Bearer ${key}` } : undefined,
    });
  } catch (err) {
    throw new AiError("unavailable", "could not reach openrouter.ai to list models", {
      backend: "openrouter",
      cause: err,
    });
  }
  if (!response.ok) {
    throw new AiError(
      response.status === 401 || response.status === 403 ? "unauthenticated" : "bad-response",
      response.status === 401 || response.status === 403
        ? "openrouter rejected the API key (HTTP 401) — check it in AI settings"
        : `openrouter model list failed: HTTP ${response.status}`,
      { backend: "openrouter" },
    );
  }
  try {
    return modelListFromReply(await response.json());
  } catch (err) {
    throw new AiError("bad-response", "openrouter model list was not valid JSON", {
      backend: "openrouter",
      cause: err,
    });
  }
}

/** Options for building an {@link OpenRouterProvider}. */
export interface OpenRouterOptions {
  /** Overrides the keyring read (tests); `null` = no key configured. */
  keyOverride?: () => Promise<string | null>;
  /** Injectable fetch for the models listing (tests). */
  fetchImpl?: FetchLike;
  /** Overrides the default model id (tests). */
  defaultModel?: string;
}

/**
 * OpenRouter {@link AiProvider} implementation. Availability = an API key
 * exists in the keyring (generation itself needs no reachability pre-check;
 * the supervisor probes via the models endpoint).
 */
export class OpenRouterProvider implements AiProvider {
  readonly id = "openrouter" as const;

  readonly #options: OpenRouterOptions;

  constructor(options: OpenRouterOptions = {}) {
    this.#options = options;
  }

  #key(): Promise<string | null> {
    return this.#options.keyOverride
      ? this.#options.keyOverride()
      : apiKey();
  }

  async available(): Promise<boolean> {
    try {
      return (await this.#key()) !== null;
    } catch {
      return false;
    }
  }

  /** Supervisor probe: models fetch (401 maps to unauthenticated). */
  async check(): Promise<AiProbeResult> {
    try {
      await this.listModels();
      return { status: "ok" };
    } catch (err) {
      if (err instanceof AiError && err.kind === "unauthenticated") {
        return { status: "unauthenticated", error: err.message };
      }
      return {
        status: "down",
        error: err instanceof Error ? err.message : String(err),
      };
    }
  }

  async listModels(): Promise<ModelInfo[]> {
    return fetchOpenRouterModels(this.#options.fetchImpl ?? fetch.bind(globalThis));
  }

  async generate(req: AiGenerateRequest): Promise<AiResult> {
    const started = Date.now();
    const key = await this.#key();
    if (!key) {
      throw new AiError(
        "unauthenticated",
        "no OpenRouter API key configured — add one in AI settings",
        { backend: "openrouter" },
      );
    }
    if (req.signal?.aborted) {
      throw new AiError("cancelled", "AI request cancelled", { backend: "openrouter" });
    }

    const modelId = req.model ?? this.#options.defaultModel ?? DEFAULT_OPENROUTER_MODEL;
    const openrouter = createOpenRouter({ apiKey: key });
    try {
      const result = await generateText({
        model: openrouter(modelId),
        ...(req.system !== undefined ? { system: req.system } : {}),
        prompt: req.prompt,
        abortSignal: req.signal,
      });
      const text = (result as { text?: unknown }).text;
      if (typeof text !== "string" || text.trim().length === 0) {
        throw new AiError("bad-response", "openrouter returned no text output", {
          backend: "openrouter",
        });
      }
      const answered =
        (result as { response?: { modelId?: unknown } }).response?.modelId;
      return {
        text,
        model: typeof answered === "string" && answered ? answered : modelId,
        backend: "openrouter",
        elapsedMs: Date.now() - started,
      };
    } catch (err) {
      if (req.signal?.aborted) {
        throw new AiError("cancelled", "AI request cancelled", { backend: "openrouter" });
      }
      if (err instanceof AiError) throw err;
      // The SDK surfaces provider errors as Error subclasses whose message
      // may echo request material — scrub the key before it escapes.
      const raw = err instanceof Error ? err.message : String(err);
      throw new AiError("unknown", scrubSecret(raw, key).slice(0, 500), {
        backend: "openrouter",
        cause: err,
      });
    }
  }
}
