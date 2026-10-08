/**
 * opencode backend adapter (M6, lane H1).
 *
 * v1 is ATTACH-only: mygitui never spawns an opencode server — it talks to
 * one the user already runs. Autodiscovery probes the default
 * `http://127.0.0.1:4096` (`GET /global/health`, 2s timeout); a manual base
 * URL from settings wins when configured. All server access goes through
 * `@opencode-ai/sdk` (`createOpencodeClient`); the health/discovery probes
 * use plain fetch because the SDK surface has no health endpoint.
 *
 * Auth: when a password is configured (keyring key
 * `opencode.server.password`), every request carries
 * `Authorization: Basic base64("opencode:" + password)` — injected by
 * wrapping the fetch instance handed to the SDK, so no secret is stored on
 * the client object or logged anywhere.
 *
 * Sessions are reused per `sessionKey` (features pass the repoId): one
 * in-memory map sessionKey → session id, sessions titled "mygitui". Text
 * generation is the synchronous prompt round trip
 * (`POST /session/{id}/message`): the response already contains the
 * assistant parts, so no event stream is needed for one-shot features.
 *
 * Every SDK/fetch call is wrapped; failures surface as
 * `AiError{backend: "opencode"}` with safe (secret-free) messages.
 */

// Client-only subpath: the package root also re-exports ./server.js, whose
// node-only deps (`which`, cross-spawn) read bare `process` at module scope
// and crash the webview bundle.
import { createOpencodeClient } from "@opencode-ai/sdk/client";
import type { OpencodeClient } from "@opencode-ai/sdk/client";
import { DEFAULT_OPENCODE_URL } from "./provider";
import type { AiGenerateRequest, AiProvider, AiProbeResult } from "./provider";
import { AiError } from "./types";
import type { AiResult, ModelInfo } from "./types";

/** Health probe timeout (ms) — the server is local; slow means absent. */
export const HEALTH_TIMEOUT_MS = 2_000;

/** Title given to sessions mygitui creates on the server. */
export const SESSION_TITLE = "mygitui";

/** Minimal fetch surface used here (subset of DOM fetch; injectable). */
export type FetchLike = typeof fetch;

/** Options for building an {@link OpenCodeProvider}. */
export interface OpenCodeOptions {
  /**
   * Manual base URL (settings override). When unset, {@link discover}
   * probes {@link DEFAULT_OPENCODE_URL}. A setter so the adapter survives
   * settings changes without being rebuilt.
   */
  url?: string | (() => string | undefined | null);
  /**
   * Resolves the basic-auth password (keyring read), or `null` when none
   * is configured. Lazy so generate/check always see the current secret.
   */
  password?: () => Promise<string | null>;
  /** Injectable fetch (tests); defaults to `globalThis.fetch`. */
  fetchImpl?: FetchLike;
}

/**
 * Probes `GET {base}/global/health` with a 2s timeout. Resolves `true` when
 * the endpoint answers (any status counts as "server present" — a 4xx/5xx
 * from a live server still means reachable; 401 maps to unauthenticated at
 * the supervisor layer). Never throws.
 */
export async function probeHealth(
  base: string,
  fetchImpl: FetchLike = fetch,
  timeoutMs = HEALTH_TIMEOUT_MS,
): Promise<{ ok: boolean; status: number }> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetchImpl(new URL("/global/health", base), {
      signal: controller.signal,
    });
    return { ok: true, status: response.status };
  } catch {
    return { ok: false, status: 0 };
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Autodiscovers a running opencode server. Order: manual `url` (when set),
 * then {@link DEFAULT_OPENCODE_URL}. Resolves the working base URL or
 * `null` when nothing answers within the health timeout.
 */
export async function discoverOpencode(
  manualUrl?: string | null,
  fetchImpl: FetchLike = fetch,
): Promise<string | null> {
  const candidates = manualUrl ? [manualUrl, DEFAULT_OPENCODE_URL] : [DEFAULT_OPENCODE_URL];
  for (const base of candidates) {
    if (await probeHealth(base, fetchImpl).then((r) => r.ok)) return base;
  }
  return null;
}

/** Builds the `Authorization: Basic …` header value for a password. */
export function basicAuthHeader(password: string): string {
  // opencode's server-password mode checks basic auth; the username is
  // conventional ("opencode"), the password is the secret.
  const token = typeof btoa === "function" ? btoa(`opencode:${password}`) : Buffer.from(`opencode:${password}`).toString("base64");
  return `Basic ${token}`;
}

/**
 * Wraps `fetch` so every request to the opencode base carries the basic
 * auth header. Request/init headers are merged (the SDK passes a `Request`
 * with its own headers); requests to other origins pass through untouched.
 */
export function authedFetch(
  base: string,
  password: () => Promise<string | null>,
  fetchImpl: FetchLike,
): FetchLike {
  return async (input, init?) => {
    const url =
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.toString()
          : input.url;
    const sameOrigin = url.startsWith(base);
    const secret = sameOrigin ? await password() : null;
    if (!sameOrigin || !secret) return fetchImpl(input, init);
    const headers = new Headers(input instanceof Request ? input.headers : undefined);
    for (const [key, value] of new Headers(init?.headers)) headers.set(key, value);
    headers.set("Authorization", basicAuthHeader(secret));
    return fetchImpl(input, { ...init, headers });
  };
}

/** True when `part` looks like an opencode text part. */
function isTextPart(part: unknown): part is { type: "text"; text: string } {
  return (
    typeof part === "object" &&
    part !== null &&
    (part as { type?: unknown }).type === "text" &&
    typeof (part as { text?: unknown }).text === "string"
  );
}

/**
 * Concatenates the text parts of an opencode message-parts array (the
 * sync prompt response's `parts`). Non-text parts (tool calls, reasoning,
 * step markers) are ignored; `null` when no text parts exist.
 */
export function partsToText(parts: unknown): string | null {
  if (!Array.isArray(parts)) return null;
  const chunks = parts.filter(isTextPart).map((p) => p.text);
  const joined = chunks.join("").trim();
  return joined.length > 0 ? joined : null;
}

/** Flattens the `/config/providers` reply into {@link ModelInfo} list. */
export function providersToModels(reply: unknown): ModelInfo[] {
  const out: ModelInfo[] = [];
  const providers = (reply as { providers?: unknown } | null)?.providers;
  if (!Array.isArray(providers)) return out;
  for (const provider of providers) {
    const providerID = (provider as { id?: unknown })?.id;
    if (typeof providerID !== "string" || providerID.length === 0) continue;
    const models = (provider as { models?: unknown })?.models;
    const entries = Array.isArray(models)
      ? models.map((m) => (m as { id?: unknown })?.id)
      : models && typeof models === "object"
        ? Object.keys(models as Record<string, unknown>)
        : [];
    for (const modelID of entries) {
      if (typeof modelID !== "string" || modelID.length === 0) continue;
      out.push({
        id: `${providerID}/${modelID}`,
        provider: providerID,
        label: `${providerID}/${modelID}`,
      });
    }
  }
  return out;
}

/**
 * opencode {@link AiProvider} implementation. One instance per app is
 * enough (URL/password are read through setters, sessions keyed per
 * `sessionKey`).
 */
export class OpenCodeProvider implements AiProvider {
  readonly id = "opencode" as const;

  readonly #options: OpenCodeOptions;
  readonly #fetch: FetchLike;
  /** Lazily built SDK client (rebuilt when the base URL changes). */
  #client: OpencodeClient | null = null;
  #clientBase: string | null = null;
  /** sessionKey → opencode session id (in-memory only). */
  readonly #sessions = new Map<string, string>();

  constructor(options: OpenCodeOptions = {}) {
    this.#options = options;
    this.#fetch = options.fetchImpl ?? fetch.bind(globalThis);
  }

  /** Current manual base URL (settings), if any. */
  #manualUrl(): string | undefined {
    const url = this.#options.url;
    if (typeof url === "function") return url() ?? undefined;
    return url ?? undefined;
  }

  /** Resolves a reachable base URL: manual setting first, then autodiscovery. */
  async #baseUrl(): Promise<string> {
    const manual = this.#manualUrl();
    const base = await discoverOpencode(manual, this.#fetch);
    if (!base) {
      throw new AiError(
        "unavailable",
        manual
          ? `opencode server not reachable at ${manual} (health probe failed)`
          : `no opencode server found (probed ${DEFAULT_OPENCODE_URL}) — start one or set the URL in AI settings`,
        { backend: "opencode" },
      );
    }
    return base;
  }

  /** SDK client for a base URL (rebuilt when the URL changes). */
  #clientFor(base: string): OpencodeClient {
    if (this.#client && this.#clientBase === base) return this.#client;
    this.#client = createOpencodeClient({
      baseUrl: base,
      fetch: authedFetch(
        base,
        async () => (await this.#options.password?.()) ?? null,
        this.#fetch,
      ) as typeof fetch,
    });
    this.#clientBase = base;
    return this.#client;
  }

  /** Reachability probe: health endpoint + status mapping (supervisor). */
  async check(): Promise<AiProbeResult> {
    try {
      const health = await this.health();
      if (!health.ok) {
        return { status: "down", error: `no opencode server at ${health.base ?? DEFAULT_OPENCODE_URL}` };
      }
      if (health.status === 401 || health.status === 403) {
        return {
          status: "unauthenticated",
          error: "opencode server requires the basic-auth password (AI settings)",
        };
      }
      return { status: "ok" };
    } catch (err) {
      return {
        status: "down",
        error: err instanceof Error ? err.message : String(err),
      };
    }
  }

  /** Raw health probe: resolves the reachable base and HTTP status. */
  async health(): Promise<{ ok: boolean; base: string | null; status: number }> {
    const base = await this.#baseUrl();
    const result = await probeHealth(base, this.#fetch);
    return { ok: result.ok, base, status: result.status };
  }

  async available(): Promise<boolean> {
    try {
      return (await this.health()).ok;
    } catch {
      return false;
    }
  }

  async listModels(): Promise<ModelInfo[]> {
    try {
      const base = await this.#baseUrl();
      const response = await this.#clientFor(base).config.providers();
      if (response.error) {
        throw new AiError("bad-response", "opencode rejected the provider list request", {
          backend: "opencode",
        });
      }
      return providersToModels(response.data);
    } catch (err) {
      if (err instanceof AiError) throw err;
      throw toAiError(err);
    }
  }

  /**
   * Returns (creating if needed) the session id for `key`. Sessions are
   * titled "mygitui"; a session that vanished server-side (404 on prompt)
   * is recreated once by the caller path.
   */
  async #sessionId(client: OpencodeClient, key: string): Promise<string> {
    const known = this.#sessions.get(key);
    if (known) return known;
    const response = await client.session.create({ body: { title: SESSION_TITLE } });
    if (response.error || !response.data?.id) {
      throw new AiError("bad-response", "opencode refused to create a session", {
        backend: "opencode",
      });
    }
    this.#sessions.set(key, response.data.id);
    return response.data.id;
  }

  async generate(req: AiGenerateRequest): Promise<AiResult> {
    const started = Date.now();
    let client: OpencodeClient;
    try {
      const base = await this.#baseUrl();
      client = this.#clientFor(base);
    } catch (err) {
      if (err instanceof AiError) throw err;
      throw toAiError(err);
    }

    const key = req.sessionKey ?? "default";
    const body: {
      parts: Array<{ type: "text"; text: string }>;
      system?: string;
      model?: { providerID: string; modelID: string };
    } = { parts: [{ type: "text", text: req.prompt }] };
    if (req.system) body.system = req.system;
    if (req.model) {
      const [providerID, ...rest] = req.model.split("/");
      const modelID = rest.join("/");
      body.model =
        providerID && modelID
          ? { providerID, modelID }
          : { providerID: req.model, modelID: req.model };
    }

    try {
      let sessionId = await this.#sessionId(client, key);
      let response = await client.session.prompt({
        path: { id: sessionId },
        body,
      });
      // Session known to us but gone server-side (restart): recreate once.
      if (response.error && this.#sessions.has(key)) {
        this.#sessions.delete(key);
        sessionId = await this.#sessionId(client, key);
        response = await client.session.prompt({
          path: { id: sessionId },
          body,
        });
      }
      if (response.error) {
        throw new AiError(
          "bad-response",
          `opencode prompt failed: ${summarizeError(response.error)}`,
          { backend: "opencode" },
        );
      }
      const text = partsToText(response.data?.parts);
      if (text === null) {
        throw new AiError(
          "bad-response",
          "opencode returned no text output for the prompt",
          { backend: "opencode" },
        );
      }
      return {
        text,
        model: req.model ?? "opencode/default",
        backend: "opencode",
        elapsedMs: Date.now() - started,
      };
    } catch (err) {
      if (req.signal?.aborted) {
        throw new AiError("cancelled", "AI request cancelled", { backend: "opencode" });
      }
      if (err instanceof AiError) throw err;
      throw toAiError(err);
    }
  }

  /** Test/teardown helper: forgets cached sessions and the SDK client. */
  reset(): void {
    this.#sessions.clear();
    this.#client = null;
    this.#clientBase = null;
  }
}

/** Last-error text from an SDK error object (safe, no request dump). */
function summarizeError(err: unknown): string {
  if (typeof err === "string") return err.slice(0, 300);
  const message = (err as { message?: unknown } | null)?.message;
  if (typeof message === "string" && message.length > 0) return message.slice(0, 300);
  return "unknown server error";
}

/** Maps an arbitrary thrown value to an {@link AiError} for this backend. */
function toAiError(err: unknown): AiError {
  const message = err instanceof Error ? err.message : String(err);
  return new AiError("unavailable", `opencode request failed: ${message}`, {
    backend: "opencode",
    cause: err,
  });
}
