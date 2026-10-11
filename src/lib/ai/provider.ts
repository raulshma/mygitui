/**
 * AI provider adapter surface (M6, lane H1).
 *
 * Every backend (opencode server, OpenRouter direct) implements
 * {@link AiProvider}. UI/feature code never touches backend SDKs — request
 * routing lives in the connection supervisor (`ensureReady`), which picks
 * the adapter from the configured backend + fallback.
 *
 * Capabilities are OPTIONAL methods ({@link AiProvider.connection},
 * {@link AiProvider.subscribeLiveness}): a provider implements the ones it
 * can serve, and callers feature-detect (`if (provider.subscribeLiveness)`).
 * Absence is the capability signal — OpenRouter implements neither.
 */

import type { AiBackend, AiResult, ModelInfo } from "./types";

/** One generation request (prompt builders own the text; this owns routing). */
export interface AiGenerateRequest {
  /** System prompt (instructions); omitted when the backend has none. */
  system?: string;
  /** The user prompt (data + task). */
  prompt: string;
  /**
   * Backend-specific model id. When omitted the provider uses its default
   * (opencode: configured model or server default; openrouter:
   * {@link DEFAULT_OPENROUTER_MODEL} or the configured `openrouterModel`).
   */
  model?: string;
  /** Cancellation (wired to feature-run timeouts / future UI aborts). */
  signal?: AbortSignal;
  /**
   * Session affinity hint. opencode reuses one server session per key
   * (features pass the repoId); openrouter ignores it.
   */
  sessionKey?: string;
}

/** Result of a backend reachability/auth probe (supervisor input). */
export interface AiProbeResult {
  status: "ok" | "down" | "unauthenticated";
  /** Safe (secret-free) failure description when not `ok`. */
  error?: string;
}

/** Cancels a subscription (Svelte store contract `unsubscribe` shape). */
export type Unsubscribe = () => void;

/** A fully resolved opencode server connection, crossed whole. */
export interface ResolvedConnection {
  /** Server base URL (no stream path). */
  baseUrl: string;
  /**
   * Built `Authorization: Basic …` header value, password included
   * (`null` when the server needs none). Re-derived per resolution —
   * never cached across a managed-server restart.
   */
  authHeader: string | null;
  /** Server API generation (picks the SSE path: `/event` vs `/api/event`). */
  serverKind: "v1" | "v2";
}

/** Backend-agnostic AI adapter. */
export interface AiProvider {
  /** Which backend this adapter talks to. */
  readonly id: AiBackend;
  /**
   * Cheap reachability/readiness probe: opencode → server health endpoint;
   * openrouter → an API key is configured. Never throws.
   */
  available(): Promise<boolean>;
  /**
   * Richer probe used by the connection supervisor: distinguishes
   * down (unreachable) from unauthenticated (reachable, rejected).
   * Optional — the supervisor falls back to {@link available}.
   */
  check?(): Promise<AiProbeResult>;
  /**
   * Resolves the whole server connection (base URL, built auth header
   * incl. the current password, API generation) in one call. Optional
   * capability — OpenRouter has nothing to resolve. Rejects when the
   * backend is unusable; callers invoke it FRESH per (re)connection
   * attempt so a rotated password is never captured stale.
   */
  connection?(): Promise<ResolvedConnection>;
  /**
   * Provider-owned SSE liveness subscription. The provider resolves the
   * connection itself — fresh per subscription attempt, including the
   * v1/v2 stream-path choice — and invokes `handler(false)` per decoded
   * event (transport alive), `handler(true)` when the stream ends (the
   * caller resubscribes on the next healthy transition). Optional
   * capability — providers without a stream don't implement it.
   */
  subscribeLiveness?(handler: (down: boolean) => void): Unsubscribe;
  /** Models this backend currently offers (settings datalist). */
  listModels(): Promise<ModelInfo[]>;
  /** One-shot text generation. Throws {@link AiError}-shaped failures. */
  generate(req: AiGenerateRequest): Promise<AiResult>;
}

/** The set of adapter instances selection picks from. */
export interface ProviderSet {
  opencode: AiProvider;
  openrouter: AiProvider;
}

/** Default OpenRouter model (placeholder constant; user-configurable). */
export const DEFAULT_OPENROUTER_MODEL = "anthropic/claude-sonnet-4.5";

/** Default opencode server base URL (autodiscovery target). */
export const DEFAULT_OPENCODE_URL = "http://127.0.0.1:4096";
