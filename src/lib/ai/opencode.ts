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
 *   - `"attach"`   — probe the manual base URL from settings, then the
 *     default `http://127.0.0.1:4096`. Candidates are strictly identified
 *     (JSON `/api/info` or `/global/health`, 2s timeout) — a 200 alone
 *     does not make an opencode server.
 *
 * Both server generations are supported (t3-style runtime routing by probe,
 * see `probeOpencodeServer`): 1.x through `@opencode-ai/sdk`
 * (`createOpencodeClient`, sync prompt round trip) and 2.x through
 * `@opencode/client` (promise client, temp-session prompt + `/api/event`
 * wait — 2.x `generate` endpoints 403 on the free tier, so one-shot
 * generation prompts in a temporary session like t3's `textGeneration`).
 * The health/discovery probes use plain fetch because neither SDK surface
 * has a health endpoint.
 *
 * Auth: every request carries `Authorization: Basic base64("opencode:" +
 * password)` — injected by wrapping the fetch instance handed to the SDKs,
 * so no secret is stored on the client objects or logged anywhere. In
 * managed mode a 2.x server's generated `OPENCODE_PASSWORD` (Rust serve
 * registry) takes precedence over the keyring password; 1.x managed
 * servers carry no password and the keyring value is used as-is.
 *
 * SSE liveness: the provider owns the event-stream subscription
 * ({@link OpenCodeProvider.subscribeLiveness}) — it resolves the whole
 * connection fresh per attempt (rotated managed passwords included) and
 * picks the generation-specific stream path (`/event` v1, `/api/event`
 * v2); `opencodeEvents.ts` only frames the SSE bytes.
 *
 * Sessions: 1.x sessions are reused per `sessionKey` (features pass the
 * repoId), titled "mygitui". 2.x generations run in a temporary session
 * that is removed afterwards (free-tier-safe). Text generation on 1.x is
 * the synchronous prompt round trip (`POST /session/{id}/message`): the
 * response already contains the assistant parts, so no event stream is
 * needed for one-shot features.
 *
 * Every SDK/fetch call is wrapped; failures surface as
 * `AiError{backend: "opencode"}` with safe (secret-free) messages.
 */

// Client-only subpath: the package root also re-exports ./server.js, whose
// node-only deps (`which`, cross-spawn) read bare `process` at module scope
// and crash the webview bundle.
//
// Both SDK modules are imported dynamically (see the factories below):
// statically they would be parsed on every app launch — including launches
// that never touch an AI feature — because the providers are constructed
// eagerly by the AI store.
import type { OpencodeClient } from "@opencode-ai/sdk/client";
import type { OpenCode as OpenCodeV2Namespace } from "@opencode/client";
import { DEFAULT_OPENCODE_URL } from "./provider";
import type {
  AiGenerateRequest,
  AiProvider,
  AiProbeResult,
  ResolvedConnection,
  Unsubscribe,
} from "./provider";
import { subscribeOpencodeEvents, type OpencodeEventSession } from "./opencodeEvents";
import { AiError } from "./types";
import type { AiResult, ModelInfo, OpencodeResolve } from "./types";

/** v2 promise-client type (resolved through {@link v2MakeFactory}). */
export type OpenCodeV2Client = ReturnType<
  typeof OpenCodeV2Namespace.make
>;

/** Memoized SDK factory (dynamic import resolves once, then caches). */
let createOpencodeClient: typeof import("@opencode-ai/sdk/client")["createOpencodeClient"] | null =
  null;

/** Resolves `createOpencodeClient`, importing the SDK on first use. */
async function sdkClientFactory(): Promise<
  NonNullable<typeof createOpencodeClient>
> {
  if (!createOpencodeClient) {
    const sdk = await import("@opencode-ai/sdk/client");
    createOpencodeClient = sdk.createOpencodeClient;
  }
  return createOpencodeClient;
}

/** Memoized v2 client factory (dynamic import resolves once, then caches). */
let makeV2Client: typeof OpenCodeV2Namespace.make | null = null;

/** Resolves the v2 `OpenCode.make`, importing `@opencode/client` on first use. */
async function v2MakeFactory(): Promise<NonNullable<typeof makeV2Client>> {
  if (!makeV2Client) {
    const mod = await import("@opencode/client");
    makeV2Client = mod.OpenCode.make;
  }
  return makeV2Client;
}

/** Test/seeding helper: forgets both memoized SDK factories. */
export function __resetOpencodeClientFactories(): void {
  createOpencodeClient = null;
  makeV2Client = null;
}

/** Health probe timeout (ms) — the server is local; slow means absent. */
export const HEALTH_TIMEOUT_MS = 2_000;

/** v2 one-shot generation timeout (ms) — t3's `GENERATION_TIMEOUT`. */
export const V2_GENERATION_TIMEOUT_MS = 3 * 60_000;

/**
 * v2 model-list retry budget (ms) — t3's `makeOpenCode2ModelLoader`: a
 * server loads its catalog lazily and lists nothing for its first few
 * hundred milliseconds (a managed spawn is always fresh), so an empty
 * list is re-read for a while instead of surfacing as "no models".
 */
export const V2_MODEL_LIST_TIMEOUT_MS = 5_000;

/** Spacing (ms) between empty v2 model-list re-reads — t3's `250 millis`. */
export const V2_MODEL_LIST_RETRY_MS = 250;

/**
 * v2 one-shot permissions (t3's `GENERATION_PERMISSIONS`): nothing in a
 * text generation needs a tool, so every tool asks and asks are rejected
 * as they arrive. Denying outright gets the session refused on free-tier
 * models; an unanswered ask would hang the wait.
 */
const V2_GENERATION_PERMISSIONS = [
  { action: "*", resource: "*", effect: "ask" },
] as const;

/** Title given to sessions mygitui creates on the server. */
export const SESSION_TITLE = "mygitui";

/** Minimal fetch surface used here (subset of DOM fetch; injectable). */
export type FetchLike = typeof fetch;

/**
 * Managed-serve state (Rust IPC). `password` is the generated
 * `OPENCODE_PASSWORD` of a running managed 2.x server (`null`/absent on
 * 1.x servers, which carry no password). In-memory only, never persisted.
 */
export interface ManagedServeState {
  running: boolean;
  url: string | null;
  error: string | null;
  password?: string | null;
}

/**
 * Managed-serve surface (Rust IPC). Injectable so tests exercise the
 * managed path without a Tauri runtime.
 */
export interface ManagedServe {
  /** Current managed-server state (starts nothing). */
  status(): Promise<ManagedServeState>;
  /** Starts the managed server (idempotent) / returns its state. */
  start(cwd?: string): Promise<ManagedServeState>;
}

/** Options for building an {@link OpenCodeProvider}. */
export interface OpenCodeOptions {
  /**
   * Decides how to reach the server (see the module doc). Defaults to
   * `"attach"` (tests).
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
 * then `/global/health` (1.x). A 401/403 with a JSON body means an
 * opencode server is listening but needs the basic-auth password —
 * reported as `unauthorized`. A 401/403 without JSON (an unrelated
 * guarded service's login page) is NOT opencode: the probe continues to
 * the next path instead of misidentifying it. Only a 200 JSON reply
 * with a decodable body identifies the server (see the module probe doc).
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
      if (isJsonContentType(reply.contentType)) {
        return { state: "unauthorized", status: reply.status };
      }
      continue;
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
 * Normalizes a workspace directory for server calls. OpenCode 1.x resolves
 * the `x-opencode-directory` value with forward slashes only: a Windows
 * backslash path (git2's `C:\repo`, or Rust's canonicalized `\\?\C:\repo`)
 * is mis-parsed into a garbage project directory and every prompt fails
 * with a 500 "Unexpected server error". Verified against opencode 1.18.35:
 * `C:/repo` works, `C:\repo` and `\\?\C:\repo` do not.
 */
export function normalizeDirectory(directory: string | null | undefined): string | undefined {
  if (!directory) return undefined;
  const stripped = directory.replace(/^\\\\\?\\/, "");
  const normalized = stripped.replace(/\\/g, "/");
  return normalized.length > 0 ? normalized : undefined;
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
 * Parses a `provider/model` slug into its parts (`null` when malformed).
 * Shared by both generations: v1 splits the same way for its prompt body,
 * v2 needs a `Model.Ref`-shaped `{providerID, id}` (t3's
 * `parseOpenCodeModelSlug`).
 */
export function parseModelSlug(slug: string | null | undefined): {
  providerID: string;
  modelID: string;
} | null {
  if (typeof slug !== "string") return null;
  const trimmed = slug.trim();
  const separator = trimmed.indexOf("/");
  if (separator <= 0 || separator === trimmed.length - 1) return null;
  return {
    providerID: trimmed.slice(0, separator),
    modelID: trimmed.slice(separator + 1),
  };
}

/**
 * Flattens a v2 `model.list` reply into a {@link ModelInfo} list
 * (t3's `loadOpenCode2Catalog` shape: `{providerID, id, name?}` entries;
 * `providerNames` is decoration from the optional `provider.list` read).
 * Ids stay `providerID/id` so selection/splitting matches v1.
 */
export function v2ModelsToInfos(
  models: ReadonlyArray<{
    providerID?: unknown;
    id?: unknown;
    name?: unknown;
  }>,
  providerNames?: ReadonlyMap<string, string>,
): ModelInfo[] {
  const out: ModelInfo[] = [];
  for (const model of models) {
    const providerID = (model as { providerID?: unknown })?.providerID;
    const modelID = (model as { id?: unknown })?.id;
    if (typeof providerID !== "string" || providerID.length === 0) continue;
    if (typeof modelID !== "string" || modelID.length === 0) continue;
    const providerName = providerNames?.get(providerID);
    const name = (model as { name?: unknown })?.name;
    const label =
      providerName && typeof name === "string" && name.length > 0
        ? `${name} (${providerID}/${modelID})`
        : `${providerID}/${modelID}`;
    out.push({ id: `${providerID}/${modelID}`, provider: providerID, label });
  }
  return out;
}

/** A resolved server plus the password that authenticates it. */
export interface ResolvedOpencodeServer {
  server: OpencodeServer;
  /** Basic-auth password (`null` when none). Never logged. */
  password: string | null;
}

/**
 * Shared tail of attach/managed server resolution: accept a probed server
 * of either generation, else throw the actionable `AiError` for the
 * outcome. The copy thunks keep the attach ("where it probed") and
 * managed ("try Re-check") wording site-specific and only evaluate on
 * the failing branch.
 */
function requireServer(
  outcome:
    | { state: "ok"; server: OpencodeServer }
    | { state: "unauthorized" }
    | { state: "absent" },
  copy: {
    unauthorized: () => string;
    absent: () => string;
  },
): OpencodeServer {
  if (outcome.state === "ok") return outcome.server;
  if (outcome.state === "unauthorized") {
    throw new AiError("unauthenticated", copy.unauthorized(), {
      backend: "opencode",
    });
  }
  throw new AiError("unavailable", copy.absent(), { backend: "opencode" });
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
  /** Lazily built v1 SDK client (rebuilt when the base URL changes). */
  #client: OpencodeClient | null = null;
  #clientKey: string | null = null;
  /** Lazily built v2 promise client (rebuilt when the base URL changes). */
  #clientV2: OpenCodeV2Client | null = null;
  #clientKeyV2: string | null = null;
  /** sessionKey → opencode session id, v1 only (in-memory only). */
  readonly #sessions = new Map<string, string>();
  /**
   * Managed 2.x server password from the Rust serve registry (in-memory
   * only). Refreshed on every managed resolution; v1 managed servers
   * carry none (`null`).
   */
  #managedPassword: string | null = null;
  /**
   * Last non-empty v2 model list (in-memory only). t3's loader keeps the
   * last loaded catalog standing in when a fresh read still lists nothing
   * after the retry budget, so the picker never flickers to empty.
   */
  #lastV2Models: ModelInfo[] = [];

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

  /** Keyring password (attach auth, and managed fallback on 1.x). */
  async #keyringPassword(): Promise<string | null> {
    return (await this.#options.password?.().catch(() => null)) ?? null;
  }

  /**
   * Password for requests to `base`: the managed 2.x password when in
   * managed mode and one is known, else the keyring password. Dynamic so
   * SDK fetch wrappers always send the current secret without rebuilding
   * clients when it changes.
   */
  async #authPassword(): Promise<string | null> {
    if ((this.#options.resolve?.() ?? "attach") === "managed" && this.#managedPassword) {
      return this.#managedPassword;
    }
    return this.#keyringPassword();
  }

  /**
   * Managed mode: return the running server's state, lazily starting it when
   * needed (the CLI-detection failure copy comes from the Rust side).
   * Remembers the managed 2.x password for subsequent requests. Reuses
   * {@link ManagedServeState} (narrowed to a live URL) instead of a second
   * ad-hoc `{url, password}` shape.
   */
  async #managedConnection(managed: ManagedServe): Promise<ManagedServeState & { url: string }> {
    const live = await managed.status().catch(() => null);
    if (live?.running && live.url) {
      this.#managedPassword = live.password ?? null;
      return { ...live, url: live.url, password: this.#managedPassword };
    }
    const started = await managed.start(this.#options.directory?.()).catch(() => null);
    if (started?.running && started.url) {
      this.#managedPassword = started.password ?? null;
      return { ...started, url: started.url, password: this.#managedPassword };
    }
    this.#managedPassword = null;
    throw new AiError(
      "unavailable",
      started?.error ?? live?.error ?? "failed to reach the managed opencode server",
      { backend: "opencode" },
    );
  }

  /** Attach mode: strict autodiscovery over manual URL + default. */
  async #attachServer(): Promise<ResolvedOpencodeServer> {
    const manual = this.#manualUrl();
    const password = await this.#keyringPassword();
    const { server, unauthorizedBase } = await discoverOpencode(manual, this.#fetch, password);
    return {
      server: requireServer(
        server
          ? { state: "ok", server }
          : unauthorizedBase !== null
            ? { state: "unauthorized" }
            : { state: "absent" },
        {
          unauthorized: () =>
            `opencode server at ${unauthorizedBase} requires the basic-auth password (AI settings)`,
          absent: () =>
            manual
              ? `opencode server not reachable at ${manual} (health probe failed) — start it, clear the URL, or switch to Managed in AI settings`
              : `no opencode server found (probed ${DEFAULT_OPENCODE_URL}) — start one or switch to Managed in AI settings`,
        },
      ),
      password,
    };
  }

  /**
   * Resolves a server that identified itself as opencode per the
   * configured mode, plus the password that authenticates it. Throws
   * `AiError` (`unavailable`/`unauthenticated`) with safe, actionable
   * copy otherwise.
   */
  async #resolveServer(): Promise<ResolvedOpencodeServer> {
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
      const { url, password: managedPassword } = await this.#managedConnection(managed);
      // A managed 2.x server only accepts its generated password; a
      // managed 1.x server carries none, so the keyring value applies.
      const password = managedPassword ?? (await this.#keyringPassword());
      const probe = await probeOpencodeServer(url, this.#fetch, HEALTH_TIMEOUT_MS, password);
      return {
        server: requireServer(
          probe.state === "ok"
            ? probe
            : probe.state === "unauthorized"
              ? { state: "unauthorized" }
              : { state: "absent" },
          {
            unauthorized: () =>
              managedPassword
                ? "the managed opencode server rejected our request — restart it with Re-check in AI settings"
                : "the managed opencode server rejected our request — clear or fix the stored password in AI settings",
            absent: () =>
              "the managed opencode server did not answer a valid health probe — try Re-check in AI settings",
          },
        ),
        password,
      };
    }
    return this.#attachServer();
  }

  /** v1 SDK client for a base URL (rebuilt when the URL changes). */
  async #clientFor(base: string, rawDirectory?: string): Promise<OpencodeClient> {
    // Backslash directories (Windows paths, \\?\ canonical forms) break the
    // server's project resolution — see normalizeDirectory.
    const directory = normalizeDirectory(rawDirectory);
    const clientKey = `${base}\0${directory ?? ""}`;
    if (this.#client && this.#clientKey === clientKey) return this.#client;
    const createOpencodeClient = await sdkClientFactory();
    this.#client = createOpencodeClient({
      baseUrl: base,
      ...(directory ? { directory } : {}),
      fetch: authedFetch(base, () => this.#authPassword(), this.#fetch) as typeof fetch,
    });
    this.#clientKey = clientKey;
    return this.#client;
  }

  /** v2 promise client for a base URL (rebuilt when the URL changes). */
  async #clientForV2(base: string): Promise<OpenCodeV2Client> {
    if (this.#clientV2 && this.#clientKeyV2 === base) return this.#clientV2;
    const make = await v2MakeFactory();
    this.#clientV2 = make({
      baseUrl: base,
      fetch: authedFetch(base, () => this.#authPassword(), this.#fetch) as typeof fetch,
    });
    this.#clientKeyV2 = base;
    return this.#clientV2;
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
   * Resolves the whole server connection — base URL, built auth header
   * (password included), API generation — in one call. Rejects with the
   * actionable `AiError` on any failure. Callers (the SSE liveness
   * subscription) invoke it FRESH per (re)connection attempt: managed
   * servers rotate their generated password on restart, so the header is
   * re-derived from the current serve-registry/keyring state, never cached
   * across a restart.
   */
  async connection(): Promise<ResolvedConnection> {
    const resolved = await this.#resolveServer();
    return {
      baseUrl: resolved.server.base,
      authHeader: resolved.password ? basicAuthHeader(resolved.password) : null,
      serverKind: resolved.server.kind,
    };
  }

  /**
   * Provider-owned SSE liveness ({@link AiProvider.subscribeLiveness}):
   * resolves the connection fresh per subscription attempt (so a restart's
   * rotated password is picked up), picks the stream path by server
   * generation (1.x `/event`, 2.x `/api/event`), and maps transport
   * signals onto `handler` — `false` per decoded event, `true` when the
   * stream ends or the connection fails. Never reconnects itself: the
   * store drops the subscription and re-subscribes on the supervisor's
   * next ok transition.
   */
  subscribeLiveness(handler: (down: boolean) => void): Unsubscribe {
    let session: OpencodeEventSession | null = null;
    let stopped = false;
    void (async () => {
      let conn: ResolvedConnection;
      try {
        conn = await this.connection();
      } catch {
        // Nothing to subscribe to (down/unauthenticated): report down; the
        // store resubscribes after the next successful probe.
        handler(true);
        return;
      }
      if (stopped) return;
      // 1.x servers stream at /event; 2.x moved the stream to /api/event.
      const eventPath = conn.serverKind === "v2" ? "/api/event" : "/event";
      session = subscribeOpencodeEvents({
        url: new URL(eventPath, conn.baseUrl).toString(),
        ...(conn.authHeader ? { authHeader: conn.authHeader } : {}),
        onEvent: () => handler(false),
        onEnded: () => handler(true),
        fetchImpl: this.#fetch,
      });
      if (stopped) session.stop();
    })();
    return () => {
      stopped = true;
      session?.stop();
    };
  }

  async available(): Promise<boolean> {
    try {
      await this.#resolveServer();
      return true;
    } catch {
      return false;
    }
  }

  async listModels(): Promise<ModelInfo[]> {
    try {
      const resolved = await this.#resolveServer();
      if (resolved.server.kind === "v2") {
        return await this.#listModelsV2(resolved.server.base, this.#options.directory?.());
      }
      const client = await this.#clientFor(resolved.server.base, this.#options.directory?.());
      const response = await client.config.providers();
      if (response.error) {
        throw new AiError(
          "bad-response",
          `opencode rejected the provider list request: ${summarizeError(response.error)}`,
          { backend: "opencode" },
        );
      }
      return providersToModels(response.data);
    } catch (err) {
      if (err instanceof AiError) throw err;
      throw toAiError(err);
    }
  }

  /**
   * v2 model catalog (t3's `loadOpenCode2Catalog` + `makeOpenCode2ModelLoader`):
   * `model.list` plus the optional `provider.list` read for display names.
   * A provider read that never answers must not hold the models back, so it
   * is bounded and its failure only drops the name decoration. An empty
   * model list is re-read for {@link V2_MODEL_LIST_TIMEOUT_MS} (the server
   * loads its catalog lazily — a freshly spawned managed server always
   * lists nothing at first); when the budget expires the last non-empty
   * list stands in, if any.
   */
  async #listModelsV2(base: string, rawDirectory?: string): Promise<ModelInfo[]> {
    const client = await this.#clientForV2(base);
    const directory = normalizeDirectory(rawDirectory);
    const location = directory ? { location: { directory } } : undefined;
    let models: Array<{ providerID: string; id: string; name: string }> = [];
    const deadline = Date.now() + V2_MODEL_LIST_TIMEOUT_MS;
    for (;;) {
      try {
        const out = await client.model.list(location);
        models = (out.data ?? []) as typeof models;
      } catch (err) {
        throw v2ErrorToAiError(err, "model.list");
      }
      if (models.length > 0 || Date.now() >= deadline) break;
      await delay(V2_MODEL_LIST_RETRY_MS);
    }
    let providerNames: Map<string, string> | undefined;
    try {
      const providers = await withTimeout(
        client.provider.list(location),
        3_000,
        () => undefined,
      );
      if (providers) {
        providerNames = new Map(
          (providers.data as Array<{ id: string; name: string }>).map((p) => [p.id, p.name]),
        );
      }
    } catch {
      providerNames = undefined;
    }
    const infos = v2ModelsToInfos(models, providerNames);
    if (infos.length > 0) {
      this.#lastV2Models = infos;
      return infos;
    }
    return this.#lastV2Models;
  }

  /**
   * Returns (creating if needed) the v1 session id for `key`. Sessions are
   * titled "mygitui"; a session that vanished server-side (404 on prompt)
   * is recreated once by the caller path.
   */
  async #sessionId(client: OpencodeClient, key: string): Promise<string> {
    const known = this.#sessions.get(key);
    if (known) return known;
    const response = await client.session.create({ body: { title: SESSION_TITLE } });
    if (response.error || !response.data?.id) {
      throw new AiError(
        "bad-response",
        `opencode refused to create a session${response.error ? `: ${summarizeError(response.error)}` : ""}`,
        { backend: "opencode" },
      );
    }
    this.#sessions.set(key, response.data.id);
    return response.data.id;
  }

  async generate(req: AiGenerateRequest): Promise<AiResult> {
    const started = Date.now();
    const directory = this.#options.directory?.(req.sessionKey);
    let resolved: ResolvedOpencodeServer;
    try {
      resolved = await this.#resolveServer();
    } catch (err) {
      if (err instanceof AiError) throw err;
      throw toAiError(err);
    }

    const configuredModel = this.#options.defaultModel;
    const defaultModel =
      (typeof configuredModel === "function" ? configuredModel() : configuredModel)?.trim() ||
      undefined;
    const model = req.model ?? defaultModel ?? "opencode/default";

    try {
      if (resolved.server.kind === "v2") {
        const text = await this.#generateV2(resolved.server.base, directory, model, req);
        return { text, model, backend: "opencode", elapsedMs: Date.now() - started };
      }
      const text = await this.#generateV1(resolved.server.base, directory, model, req);
      return { text, model, backend: "opencode", elapsedMs: Date.now() - started };
    } catch (err) {
      if (req.signal?.aborted) {
        throw new AiError("cancelled", "AI request cancelled", { backend: "opencode" });
      }
      if (err instanceof AiError) throw err;
      throw toAiError(err);
    }
  }

  /** v1 one-shot generation: shared titled session + sync prompt round trip. */
  async #generateV1(
    base: string,
    directory: string | undefined,
    model: string,
    req: AiGenerateRequest,
  ): Promise<string> {
    const client = await this.#clientFor(base, directory);
    const key = `${normalizeDirectory(directory) ?? ""}\0${req.sessionKey ?? "default"}`;
    const body: {
      parts: Array<{ type: "text"; text: string }>;
      system?: string;
      model?: { providerID: string; modelID: string };
    } = { parts: [{ type: "text", text: req.prompt }] };
    if (req.system) body.system = req.system;
    if (model && model !== "opencode/default") {
      const [providerID, ...rest] = model.split("/");
      const modelID = rest.join("/");
      body.model =
        providerID && modelID
          ? { providerID, modelID }
          : { providerID: model, modelID: model };
    }

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
      // 200 with no text usually carries the failure on info.error (e.g.
      // the model's provider has no API key on the server) — surface it
      // instead of a generic "no text" that hides the actual cause.
      const info = describeInfoError(response.data?.info);
      if (info) {
        throw new AiError(
          info.kind === "unauthenticated" ? "unauthenticated" : "bad-response",
          `opencode model failed: ${info.message} — check the model/API key in AI settings or opencode`,
          { backend: "opencode" },
        );
      }
      throw new AiError(
        "bad-response",
        "opencode returned no text output for the prompt",
        { backend: "opencode" },
      );
    }
    return text;
  }

  /**
   * v2 one-shot generation (t3's `textGeneration`): the `generate`
   * endpoints 403 on the free tier, so each request prompts in a temporary
   * session and waits on `/api/event` for its execution end, then removes
   * the session. Permission asks are rejected (nothing in a generation
   * needs a tool).
   */
  async #generateV2(
    base: string,
    rawDirectory: string | undefined,
    model: string,
    req: AiGenerateRequest,
  ): Promise<string> {
    const client = await this.#clientForV2(base);
    const directory = normalizeDirectory(rawDirectory);
    const parsed =
      model === "opencode/default" ? null : parseModelSlug(model);
    if (model !== "opencode/default" && !parsed) {
      throw new AiError(
        "bad-response",
        `OpenCode model '${model}' must use provider/model format`,
        { backend: "opencode" },
      );
    }
    // v2 prompts carry no `system` field — fold it into the text.
    const text = req.system ? `${req.system}\n\n${req.prompt}` : req.prompt;

    let sessionID: string;
    try {
      const session = await client.session.create({
        title: SESSION_TITLE,
        ...(directory ? { location: { directory } } : {}),
        ...(parsed ? { model: { providerID: parsed.providerID, id: parsed.modelID } } : {}),
        permissions: [...V2_GENERATION_PERMISSIONS],
      });
      sessionID = session.id;
    } catch (err) {
      throw v2ErrorToAiError(err, "session.create");
    }

    try {
      return await this.#awaitV2Execution(client, sessionID, text, req.signal);
    } finally {
      await client.session.remove({ sessionID }).catch(() => {});
    }
  }

  /**
   * Subscribes to the v2 `/api/event` stream before prompting (the stream
   * is volatile — the reply only arrives on it), prompts, then waits for
   * this session's execution end while collecting its `session.text.ended`
   * parts.
   */
  async #awaitV2Execution(
    client: OpenCodeV2Client,
    sessionID: string,
    text: string,
    signal?: AbortSignal,
  ): Promise<string> {
    if (signal?.aborted) {
      throw new AiError("cancelled", "AI request cancelled", { backend: "opencode" });
    }
    const controller = new AbortController();
    const onAbort = (): void => controller.abort();
    signal?.addEventListener("abort", onAbort, { once: true });
    let settled = false;
    const finish = (): void => {
      if (settled) return;
      settled = true;
      controller.abort();
      signal?.removeEventListener("abort", onAbort);
    };

    const outcome = new Promise<string>((resolve, reject) => {
      void (async () => {
        const texts = new Map<string, string>();
        try {
          // Subscribed first: the reply arrives only on this stream.
          const events = client.event.subscribe({ signal: controller.signal });
          client.session
            .prompt({ sessionID, text }, { signal: controller.signal })
            .catch((err: unknown) => {
              finish();
              reject(v2ErrorToAiError(err, "session.prompt"));
            });
          for await (const event of events) {
            if (settled) return;
            const data = (event as { data?: { sessionID?: string } }).data;
            if (!data || data.sessionID !== sessionID) continue;
            switch (event.type) {
              case "session.text.ended": {
                const d = data as { assistantMessageID: string; ordinal: number; text: string };
                texts.set(`${d.assistantMessageID}:${d.ordinal}`, d.text);
                break;
              }
              case "permission.asked": {
                const d = data as { id: string; sessionID: string };
                await client.permission
                  .reply({
                    sessionID: d.sessionID,
                    requestID: d.id,
                    decision: "reject",
                    message: "Tools are not available while generating text.",
                  })
                  .catch(() => {});
                break;
              }
              case "session.execution.succeeded": {
                const joined = [...texts.values()].join("\n").trim();
                if (joined.length === 0) {
                  reject(
                    new AiError("bad-response", "opencode returned no text output for the prompt", {
                      backend: "opencode",
                    }),
                  );
                } else {
                  resolve(joined);
                }
                return;
              }
              case "session.execution.failed": {
                const d = data as { error?: unknown };
                reject(
                  new AiError(
                    "bad-response",
                    `opencode prompt failed: ${summarizeError(d.error ?? "execution failed")} — check the model/API key in AI settings or opencode`,
                    { backend: "opencode" },
                  ),
                );
                return;
              }
              case "session.execution.interrupted": {
                reject(new AiError("cancelled", "AI request cancelled", { backend: "opencode" }));
                return;
              }
              default:
                break;
            }
          }
          reject(
            new AiError("bad-response", "The OpenCode event stream ended before the reply arrived.", {
              backend: "opencode",
            }),
          );
        } catch (err) {
          if (controller.signal.aborted || signal?.aborted) {
            reject(new AiError("cancelled", "AI request cancelled", { backend: "opencode" }));
          } else {
            reject(err instanceof AiError ? err : v2ErrorToAiError(err, "event.subscribe"));
          }
        }
      })();
    });

    try {
      return await withTimeout(outcome, V2_GENERATION_TIMEOUT_MS, () =>
        client.session.interrupt({ sessionID }).catch(() => {}),
      );
    } catch (err) {
      if (err instanceof AiError && err.message.includes("did not finish generating in time")) {
        throw err;
      }
      throw err;
    } finally {
      finish();
    }
  }

  /** Test/teardown helper: forgets cached sessions and both SDK clients. */
  reset(): void {
    this.#sessions.clear();
    this.#client = null;
    this.#clientKey = null;
    this.#clientV2 = null;
    this.#clientKeyV2 = null;
    this.#managedPassword = null;
    this.#lastV2Models = [];
  }
}

/** Resolves after `ms` (spacing for the v2 model-list empty retry). */
function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Last-error text from an SDK error object (safe, no request dump). */
function summarizeError(err: unknown): string {
  if (typeof err === "string") return err.slice(0, 300);
  // Non-2xx bodies decode to NamedError POJOs (`{name, data: {message, ref}}`)
  // — there is no top-level `.message`, so read both shapes.
  const shape = err as
    | { name?: unknown; message?: unknown; data?: { message?: unknown; ref?: unknown } }
    | null;
  const message =
    typeof shape?.message === "string" && shape.message.length > 0
      ? shape.message
      : typeof shape?.data?.message === "string" && shape.data.message.length > 0
        ? shape.data.message
        : null;
  const name = typeof shape?.name === "string" && shape.name.length > 0 ? shape.name : null;
  const ref = typeof shape?.data?.ref === "string" && shape.data.ref.length > 0 ? shape.data.ref : null;
  let out = message ?? name ?? "unknown server error";
  if (name && message) out = `${name}: ${message}`;
  if (ref) out = `${out} (ref ${ref})`;
  return out.slice(0, 300);
}

/**
 * Renders the `info.error` field of a successful prompt reply (200 with a
 * failed model call). `null` when the reply carries no error info.
 */
function describeInfoError(
  info: unknown,
): { kind: "unauthenticated" | "bad-response"; message: string } | null {
  const error = (info as { error?: unknown } | null)?.error;
  if (typeof error !== "object" || error === null) return null;
  const shape = error as { name?: unknown; data?: { message?: unknown } };
  const message =
    typeof shape.data?.message === "string" && shape.data.message.length > 0
      ? shape.data.message
      : typeof shape.name === "string"
        ? shape.name
        : null;
  if (message === null) return null;
  return {
    kind: shape.name === "ProviderAuthError" ? "unauthenticated" : "bad-response",
    message,
  };
}

/** Maps an arbitrary thrown value to an {@link AiError} for this backend. */
function toAiError(err: unknown): AiError {
  if (err instanceof AiError) return err;
  const message = err instanceof Error ? err.message : String(err);
  return new AiError("unavailable", `opencode request failed: ${message}`, {
    backend: "opencode",
    cause: err,
  });
}

/**
 * Maps a v2 promise-client failure to an {@link AiError}. Auth-shaped
 * failures (401/`UnauthorizedError`) become `unauthenticated` so the
 * supervisor shows the password state; transport failures stay
 * `unavailable`; everything else is `bad-response` with the operation
 * named but no secret material.
 */
export function v2ErrorToAiError(err: unknown, operation: string): AiError {
  if (err instanceof AiError) return err;
  const shape = err as {
    name?: unknown;
    message?: unknown;
    status?: unknown;
    cause?: { status?: unknown };
  } | null;
  const name = typeof shape?.name === "string" ? shape.name : null;
  const status =
    typeof shape?.status === "number"
      ? shape.status
      : typeof shape?.cause?.status === "number"
        ? shape.cause.status
        : null;
  const message = typeof shape?.message === "string" ? shape.message : String(err);
  if (status === 401 || name === "UnauthorizedError" || /unauthorized|401/i.test(message)) {
    return new AiError("unauthenticated", "the opencode server rejected the password", {
      backend: "opencode",
      cause: err,
    });
  }
  if (/transport|fetch failed|load failed|network|ECONNREFUSED|not reachable/i.test(message)) {
    return new AiError("unavailable", `opencode request failed: ${message.slice(0, 200)}`, {
      backend: "opencode",
      cause: err,
    });
  }
  return new AiError(
    "bad-response",
    `opencode ${operation} failed: ${summarizeError(err)}`,
    { backend: "opencode", cause: err },
  );
}

/**
 * Awaits `promise`, running `onTimeout` (e.g. session interrupt) and
 * rejecting when `ms` elapses first. Resolves with `undefined` when
 * `onTimeout` handles the wait (interrupt path resolves via the outcome).
 */
export function withTimeout<T>(
  promise: Promise<T>,
  ms: number,
  onTimeout?: () => unknown,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      try {
        void onTimeout?.();
      } catch {
        // Interrupt is best-effort; the timeout error below carries the failure.
      }
      reject(
        new AiError("unavailable", "OpenCode did not finish generating in time.", {
          backend: "opencode",
        }),
      );
    }, ms);
  });
  return Promise.race([promise, timeout]).finally(() => {
    if (timer !== null) clearTimeout(timer);
  }) as Promise<T>;
}
