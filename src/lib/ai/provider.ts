/**
 * AI provider adapter surface + selection (M6, lane H1).
 *
 * Every backend (opencode server, OpenRouter direct) implements
 * {@link AiProvider}. UI/feature code never touches backend SDKs — it goes
 * through the supervisor, which selects an adapter via
 * {@link selectProvider}.
 */

import { AiError } from "./types";
import type { AiBackend, AiConfig, AiResult, ModelInfo } from "./types";

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

/**
 * Picks the provider for a request: the configured backend when available,
 * otherwise (config `allowFallback` !== false) the other backend when *it*
 * is available. Availability is probed through the adapters (cached by the
 * supervisor; this helper probes directly and is only used outside the
 * supervisor's request path — tests).
 *
 * Throws `AiError("unavailable")` when nothing is usable; the error names
 * the configured backend so the UI can point at settings.
 */
export async function selectProvider(
  config: Pick<AiConfig, "backend" | "allowFallback">,
  providers: ProviderSet,
): Promise<AiProvider> {
  const preferred = providers[config.backend];
  if (await preferred.available()) return preferred;

  const allowFallback = config.allowFallback !== false;
  const other: AiProvider =
    config.backend === "opencode" ? providers.openrouter : providers.opencode;
  if (allowFallback && (await other.available())) return other;

  throw new AiError(
    "unavailable",
    `AI backend "${config.backend}" is unavailable${allowFallback ? " (and fallback is unavailable)" : " (fallback disabled)"} — check AI settings`,
    { backend: config.backend },
  );
}
