/**
 * AI layer shared types (M6, lane H1).
 *
 * Two backends, one surface:
 *   - `"opencode"`    — a local opencode server. Two modes (M12): managed
 *     (default) — the app detects the opencode CLI and spawns/owns an
 *     `opencode serve` child on a free 127.0.0.1 port; or attach — the
 *     app talks to a server the user already runs (default
 *     http://127.0.0.1:4096 or a manual URL). Optional basic-auth password
 *     lives in the OS keyring.
 *   - `"openrouter"`  — direct OpenRouter API access with a user API key
 *     (OS keyring), via the Vercel AI SDK + @openrouter/ai-sdk-provider.
 *
 * Secrets (keys, passwords) are NEVER stored in localStorage — only the
 * non-secret config below is. Non-secret config lives under the
 * `"mygitui.ai"` localStorage key (see ai.svelte.ts).
 */

/** Which AI backend a request should go to. */
export type AiBackend = "opencode" | "openrouter";

/** All backends, canonical order (config UI radio order). */
export const AI_BACKENDS: readonly AiBackend[] = ["opencode", "openrouter"] as const;

/**
 * How the opencode backend finds its server:
 *   - `"managed"` — mygitui detects the CLI and spawns the server itself
 *     (default; the t3-style "enable and it just works" path).
 *   - `"attach"`  — the user runs the server; we discover/attach to it.
 */
export type OpencodeMode = "managed" | "attach";

/** Non-secret AI configuration (persisted; localStorage `mygitui.ai`). */
export interface AiConfig {
  /** Preferred backend for new requests. */
  backend: AiBackend;
  /** opencode server mode; default {@link DEFAULT_OPENCODE_MODE}. */
  opencodeMode?: OpencodeMode;
  /**
   * Master switch for the opencode backend; default `true` (opt-out).
   * `false` mirrors t3's "disabled" state: nothing is probed or spawned
   * and features route to the fallback backend only.
   */
  opencodeEnabled?: boolean;
  /**
   * Attach mode: manual server base URL. When unset the supervisor
   * autodiscovers the default {@link DEFAULT_OPENCODE_URL}.
   */
  opencodeUrl?: string;
  /** OpenRouter model id (`provider/model`); default when generate has none. */
  openrouterModel?: string;
  /**
   * When the preferred backend is unavailable, allow silently using the
   * other one. Default `true`.
   */
  allowFallback?: boolean;
  /** Per-repository opt-in: AI features stay off until the user allows. */
  repoOptIn: Record<string, boolean>;
}

/** opencode mode used when config does not say (managed = just works). */
export const DEFAULT_OPENCODE_MODE: OpencodeMode = "managed";

/**
 * Resolves the effective opencode mode. Legacy configs (M6–M11) only set
 * `opencodeUrl` — an explicit manual URL implies attach; everything else
 * defaults to managed.
 */
export function resolveOpencodeMode(config: AiConfig): OpencodeMode {
  if (config.opencodeMode) return config.opencodeMode;
  if (config.opencodeUrl) return "attach";
  return DEFAULT_OPENCODE_MODE;
}

/**
 * What the opencode provider should do for the next request, decided by
 * the store from config (`disabled` short-circuits everything).
 */
export type OpencodeResolve = "disabled" | "managed" | "attach";

/** Resolves the provider behavior for opencode from config. */
export function resolveOpencode(config: AiConfig): OpencodeResolve {
  if (config.opencodeEnabled === false) return "disabled";
  return resolveOpencodeMode(config);
}

/** Default non-secret config (before the user changes anything). */
export function defaultAiConfig(): AiConfig {
  return { backend: "opencode", allowFallback: true, repoOptIn: {} };
}

/** The AI features mygitui can run, v1. */
export type FeatureKind =
  | "commit-message"
  | "pr-title-body"
  | "explain-hunk"
  | "review-staged"
  | "stash-message"
  | "branch-name";

/** A model offered by a backend (flattened; selection lists + defaults). */
export interface ModelInfo {
  /**
   * Canonical id passed to {@link AiProvider.generate} as `model`.
   * opencode: `"providerID/modelID"`; openrouter: the OpenRouter slug
   * (`"anthropic/claude-sonnet-4.5"`).
   */
  id: string;
  /** Owning provider id when the backend hosts several (opencode). */
  provider?: string;
  /** Human label for pickers (may equal `id`). */
  label?: string;
}

/** Successful generation. */
export interface AiResult {
  /** The generated text (backend-specific post-processing already applied). */
  text: string;
  /** Model that answered (resolved id, not just the requested one). */
  model: string;
  /** Backend that actually served the request (fallback aware). */
  backend: AiBackend;
  /** Wall-clock round trip in milliseconds. */
  elapsedMs: number;
}

/** Uniform failure for anything AI: which backend and a safe message. */
export class AiError extends Error {
  /** Backend that failed, when one was involved. */
  readonly backend: AiBackend | null;
  /** Static machine-readable kind (UI decides retry vs reconfigure). */
  readonly kind: AiErrorKind;
  /** Underlying error, when there was one (never serialized into `message`). */
  readonly cause?: unknown;

  constructor(
    kind: AiErrorKind,
    message: string,
    options?: { backend?: AiBackend | null; cause?: unknown },
  ) {
    super(message);
    this.name = "AiError";
    this.kind = kind;
    this.backend = options?.backend ?? null;
    this.cause = options?.cause;
  }
}

/** Coarse failure categories. */
export type AiErrorKind =
  /** Backend not reachable / not running. */
  | "unavailable"
  /** Credentials missing or rejected (never names the secret). */
  | "unauthenticated"
  /** User has not opted in for this repository. */
  | "opt-in"
  /** Backend answered but unusably (parse error, empty, non-2xx). */
  | "bad-response"
  /** Request cancelled by an AbortSignal. */
  | "cancelled"
  /** Anything else. */
  | "unknown";

/** True when `err` is an {@link AiError}. */
export function isAiError(err: unknown): err is AiError {
  return err instanceof AiError;
}

/** Safe message for toasts/logs: never leaks secret material. */
export function aiErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}
