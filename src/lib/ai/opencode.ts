/**
 * opencode backend adapter (M6, lane H1; M12 adds managed serve).
 *
 * Resolution order for the server base URL (the `resolve` option, wired
 * from config by the store):
 *   - `"managed"`  — the app owns the server: ask the Rust side for the
 *     managed `opencode serve` state and lazily start it when not running
 *     (t3-style: detect the CLI, spawn on a free port, done). Requires
 *     Tauri; the store only selects this mode when IPC is available.
 *   - `"disabled"` — opencode is switched off in settings: fail fast with
 *     an actionable message, never probing or spawning anything.
 *   - `"attach"`   — v1 behavior: probe the manual base URL from settings,
 *     then the default `http://127.0.0.1:4096`. Candidates are strictly
 *     identified (JSON `/api/info` or `/global/health`, 2s timeout) — a
 *     200 alone does not make an opencode server.
 *
 * All server access goes through `@opencode-ai/sdk`
 * (`createOpencodeClient`); the health/discovery probes use plain fetch
 * because the SDK surface has no health endpoint.
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
import type { AiResult, ModelInfo, OpencodeResolve } from "./types";

/** Health probe timeout (ms) — the server is local; slow means absent. */
export const HEALTH_TIMEOUT_MS = 2_000;

/** Title given to sessions mygitui creates on the server. */
export const SESSION_TITLE = "mygitui";

/** Minimal fetch surface used here (subset of DOM fetch; injectable). */
export type FetchLike = typeof fetch;

/**
 * Managed-serve surface (Rust IPC). Injectable so tests exercise the
 * managed path without a Tauri runtime.
 */
export interface ManagedServe {
  /** Current managed-server state (starts nothing). */
  status(): Promise<{ running: boolean; url: string | null; error: string | null }>;
  /** Starts the managed server (idempotent) / returns its state. */
  start(cwd?: string): Promise<{ running: boolean; url: string | null; error: string | null }>;
}

/** Options for building an {@link OpenCodeProvider}. */
export interface OpenCodeOptions {
  /**
   * Decides how to reach the server (see the module doc). Defaults to
   * `"attach"` (v1 behavior; tests).
   */
  resolve?: () => OpencodeResolve;
  /**
   * Attach mode: manual base URL (settings override). When unset,
   * {@link discover} probes {@link DEFAULT_OPENCODE_URL}. A setter so the
   * adapter survives settings changes without being rebuilt.
   */
  url?: string | (() => string | undefined | null);
  /**
   * Managed-mode bridge to the Rust serve registry. Required only when
   * `resolve` can return `"managed"`.
   */
  managed?: () => ManagedServe | null;
  /** Maps a session key (repo id) to its workspace directory for SDK calls. */
  directory?: (sessionKey?: string) => string | undefined;
  /**
   * Resolves the basic-auth password (keyring read), or `null` when none
   * is configured. Lazy so generate/check always see the current secret.
   */
  password?: () => Promise<string | null>;
  /** Selected model id, or `undefined` to use OpenCode's server default. */
  defaultModel?: string | (() => string | null | undefined);
  /** Injectable fetch (tests); defaults to `globalThis.fetch`. */
  fetchImpl?: FetchLike;
}

/**
 * Health/discovery probing, t3-style (versionProbe.ts): a candidate base
 * URL only counts as an opencode server when an endpoint answers
 * `application/json` with a body that decodes to a known shape —
 * `/api/info` `{version, pid}` for 2.x servers, `/global/health`
 * `{healthy: true, version}` for 1.x. A 200 alone is NOT enough: opencode
 * servers serve their web UI's HTML with a 200 on unknown paths, so a
 * lenient probe misclassifies any server (and unrelated local services)
 * as "opencode".
 */

/** Which server API generation answered the probe. */
export type OpencodeServerKind = "v1" | "v2";

/** A server that identified itself as opencode. */
export interface OpencodeServer {
  base: string;
  kind: OpencodeServerKind;
  /** Server-reported `x.y.z`, when parseable. */
  version: string | null;
}

/** Outcome of probing one candidate base URL. */
export type OpencodeProbe =
  | { state: "ok"; server: OpencodeServer }
  | { state: "unauthorized"; status: number }
  | { state: "absent"; status: number };

/** True when the content type advertises JSON (HTML web-UI replies fail). */
export function isJsonContentType(contentType: string | null): boolean {
  return contentType !== null && /application\/(?:json|\S*json)/i.test(contentType);
}

/** Decodes the v1 `/global/health` body; resolves the version or `null`. */
export function decodeHealthV1(text: string): string | null {
  try {
    const parsed: unknown = JSON.parse(text);
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      (parsed as { healthy?: unknown }).healthy === true &&
      typeof (parsed as { version?: unknown }).version === "string"
    ) {
      return (parsed as { version: string }).version;
    }
    return null;
  } catch {
    return null;
  }
}

/** Decodes the v2 `/api/info` body; resolves the version or `null`. */
export function decodeInfoV2(text: string): string | null {
  try {
    const parsed: unknown = JSON.parse(text);
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      typeof (parsed as { version?: unknown }).version === "string" &&
      typeof (parsed as { pid?: unknown }).pid === "number"
    ) {
      return (parsed as { version: string }).version;
    }
    return null;
  } catch {
    return null;
  }
}

/** One GET with an abort timeout; `null` on network failure. Never throws. */
async function fetchText(
  url: URL,
  fetchImpl: FetchLike,
  timeoutMs: number,
  password?: string | null,
): Promise<{ status: number; contentType: string | null; text: string } | null> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetchImpl(url, {
      signal: controller.signal,
      ...(password ? { headers: { Authorization: basicAuthHeader(password) } } : {}),
    });
    let text = "";
    try {
      text = await response.text();
    } catch {
      text = "";
    }
    const contentType = response.headers?.get?.("content-type") ?? null;
    return { status: response.status, contentType, text };
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Probes `base` for an opencode server, t3-order: `/api/info` (2.x) first,
 * then `/global/health` (1.x). A 401/403 on either path means an
 * opencode server (or something equally guarded) is listening — reported
 * as `unauthorized`. Only a 200 JSON reply with a decodable body
 * identifies the server (see the module probe doc).
 */
export async function probeOpencodeServer(
  base: string,
  fetchImpl: FetchLike = fetch,
  timeoutMs = HEALTH_TIMEOUT_MS,
  password?: string | null,
): Promise<OpencodeProbe> {
  let lastStatus = 0;
  let root: URL;
  try {
    root = new URL(base);
  } catch {
    return { state: "absent", status: 0 };
  }
  if (root.protocol !== "http:" && root.protocol !== "https:") {
    return { state: "absent", status: 0 };
  }
  for (const path of ["/api/info", "/global/health"] as const) {
    // Keep reverse-proxy path prefixes and routing queries from the configured URL.
    const endpoint = new URL(root);
    endpoint.pathname = `${endpoint.pathname.replace(/\/+$/, "")}${path}`;
    const reply = await fetchText(endpoint, fetchImpl, timeoutMs, password);
    if (!reply) return { state: "absent", status: 0 };
    lastStatus = reply.status;
    if (reply.status === 401 || reply.status === 403) {
      return { state: "unauthorized", status: reply.status };
    }
    if (reply.status === 200 && isJsonContentType(reply.contentType)) {
      const version = path === "/api/info" ? decodeInfoV2(reply.text) : decodeHealthV1(reply.text);
      if (version !== null) {
        return {
          state: "ok",
          server: { base, kind: path === "/api/info" ? "v2" : "v1", version },
        };
      }
    }
  }
  return { state: "absent", status: lastStatus };
}

/** What {@link discoverOpencode} found across its candidates. */
export interface Discovery {
  /** The first candidate that identified itself as opencode. */
  server: OpencodeServer | null;
  /** First candidate that answered 401/403 (server present, auth needed). */
  unauthorizedBase: string | null;
}

/**
 * Autodiscovers a running opencode server. Order: manual `url` (when set),
 * then {@link DEFAULT_OPENCODE_URL}; each candidate is strictly identified
 * by {@link probeOpencodeServer}.
 */
export async function discoverOpencode(
  manualUrl?: string | null,
  fetchImpl: FetchLike = fetch,
  password?: string | null,
): Promise<Discovery> {
  const candidates = manualUrl
    ? [...new Set([manualUrl, DEFAULT_OPENCODE_URL])]
    : [DEFAULT_OPENCODE_URL];
  let unauthorizedBase: string | null = null;
  for (const base of candidates) {
    const probe = await probeOpencodeServer(base, fetchImpl, HEALTH_TIMEOUT_MS, password);
    if (probe.state === "ok") return { server: probe.server, unauthorizedBase };
    if (probe.state === "unauthorized" && unauthorizedBase === null) {
      unauthorizedBase = base;
    }
  }
  return { server: null, unauthorizedBase };
}

/** Builds the `Authorization: Basic …` header value for a password. */
export function basicAuthHeader(password: string): string {
  // Basic auth is UTF-8 on OpenCode servers; browser btoa accepts only bytes.
  const bytes = new TextEncoder().encode(`opencode:${password}`);
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  const token = typeof btoa === "function" ? btoa(binary) : Buffer.from(bytes).toString("base64");
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
  #clientKey: string | null = null;
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

  /** Managed-serve bridge, or `null` when none is wired. */
  #managed(): ManagedServe | null {
    return this.#options.managed?.() ?? null;
  }

  /**
   * Managed mode: return the running server's URL, lazily starting it when
   * needed (the CLI-detection failure copy comes from the Rust side).
   */
  async #managedUrl(managed: ManagedServe): Promise<string> {
    const live = await managed.status().catch(() => null);
    if (live?.running && live.url) return live.url;
    const started = await managed.start(this.#options.directory?.()).catch(() => null);
    if (started?.running && started.url) return started.url;
    throw new AiError(
      "unavailable",
      started?.error ?? live?.error ?? "failed to reach the managed opencode server",
      { backend: "opencode" },
    );
  }

  /** Attach mode: strict autodiscovery over manual URL + default. */
  async #attachServer(): Promise<OpencodeServer> {
    const manual = this.#manualUrl();
    const password = (await this.#options.password?.().catch(() => null)) ?? null;
    const { server, unauthorizedBase } = await discoverOpencode(manual, this.#fetch, password);
    if (server) {
      if (server.kind === "v2") {
        throw new AiError(
          "unavailable",
          `opencode server at ${server.base} speaks the 2.x API (v${server.version ?? "?"}) — this build supports 1.x servers`,
          { backend: "opencode" },
        );
      }
      return server;
    }
    if (unauthorizedBase) {
      throw new AiError(
        "unauthenticated",
        `opencode server at ${unauthorizedBase} requires the basic-auth password (AI settings)`,
        { backend: "opencode" },
      );
    }
    throw new AiError(
      "unavailable",
      manual
        ? `opencode server not reachable at ${manual} (health probe failed) — start it, clear the URL, or switch to Managed in AI settings`
        : `no opencode server found (probed ${DEFAULT_OPENCODE_URL}) — start one or switch to Managed in AI settings`,
      { backend: "opencode" },
    );
  }

  /**
   * Resolves a server that identified itself as opencode per the
   * configured mode. Throws `AiError` (`unavailable`/`unauthenticated`)
   * with safe, actionable copy otherwise.
   */
  async #resolveServer(): Promise<OpencodeServer> {
    const mode = this.#options.resolve?.() ?? "attach";
    if (mode === "disabled") {
      throw new AiError("unavailable", "opencode is disabled in AI settings", {
        backend: "opencode",
      });
    }
    if (mode === "managed") {
      const managed = this.#managed();
      if (!managed) {
        throw new AiError(
          "unavailable",
          "managed opencode requires the desktop app — switch to attach mode or another backend",
          { backend: "opencode" },
        );
      }
      const url = await this.#managedUrl(managed);
      const password = (await this.#options.password?.().catch(() => null)) ?? null;
      const probe = await probeOpencodeServer(url, this.#fetch, HEALTH_TIMEOUT_MS, password);
      if (probe.state === "ok") {
        if (probe.server.kind === "v2") {
          throw new AiError(
            "unavailable",
            `the managed opencode serves the 2.x API (v${probe.server.version ?? "?"}) — this build supports 1.x servers; install opencode 1.x`,
            { backend: "opencode" },
          );
        }
        return probe.server;
      }
      if (probe.state === "unauthorized") {
        throw new AiError(
          "unauthenticated",
          "the managed opencode server rejected our request — clear or fix the stored password in AI settings",
          { backend: "opencode" },
        );
      }
      throw new AiError(
        "unavailable",
        "the managed opencode server did not answer a valid health probe — try Re-check in AI settings",
        { backend: "opencode" },
      );
    }
    return this.#attachServer();
  }

  /** SDK client for a base URL (rebuilt when the URL changes). */
  #clientFor(base: string, directory?: string): OpencodeClient {
    const clientKey = `${base}\0${directory ?? ""}`;
    if (this.#client && this.#clientKey === clientKey) return this.#client;
    this.#client = createOpencodeClient({
      baseUrl: base,
      ...(directory ? { directory } : {}),
      fetch: authedFetch(
        base,
        async () => (await this.#options.password?.()) ?? null,
        this.#fetch,
      ) as typeof fetch,
    });
    this.#clientKey = clientKey;
    return this.#client;
  }

  /**
   * Reachability probe: strict server identification + status mapping
   * (supervisor). `unauthenticated` means an opencode server answered but
   * rejected the request (basic auth).
   */
  async check(): Promise<AiProbeResult> {
    try {
      await this.#resolveServer();
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

  /**
   * Resolves the reachable server (never throws: `base` is `null` on any
   * failure). Consumers (the SSE liveness loop) only need the base URL.
   */
  async health(): Promise<{ base: string | null }> {
    try {
      return { base: (await this.#resolveServer()).base };
    } catch {
      return { base: null };
    }
  }

  async available(): Promise<boolean> {
    try {
      return (await this.health()).base !== null;
    } catch {
      return false;
    }
  }

  async listModels(): Promise<ModelInfo[]> {
    try {
      const base = await this.#resolveServer().then((s) => s.base);
      const response = await this.#clientFor(base, this.#options.directory?.()).config.providers();
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
    const directory = this.#options.directory?.(req.sessionKey);
    try {
      const base = await this.#resolveServer().then((s) => s.base);
      client = this.#clientFor(base, directory);
    } catch (err) {
      if (err instanceof AiError) throw err;
      throw toAiError(err);
    }

    const key = `${directory ?? ""}\0${req.sessionKey ?? "default"}`;
    const configuredModel = this.#options.defaultModel;
    const defaultModel =
      (typeof configuredModel === "function" ? configuredModel() : configuredModel)?.trim() ||
      undefined;
    const model = req.model ?? defaultModel;
    const body: {
      parts: Array<{ type: "text"; text: string }>;
      system?: string;
      model?: { providerID: string; modelID: string };
    } = { parts: [{ type: "text", text: req.prompt }] };
    if (req.system) body.system = req.system;
    if (model) {
      const [providerID, ...rest] = model.split("/");
      const modelID = rest.join("/");
      body.model =
        providerID && modelID
          ? { providerID, modelID }
          : { providerID: model, modelID: model };
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
        model: model ?? "opencode/default",
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
    this.#clientKey = null;
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
