/**
 * AI connection supervisor (M6, lane H1).
 *
 * One place that owns backend reachability state and request routing:
 *
 *   - Per-backend status (`unknown | checking | ok | down | unauthenticated`)
 *     exposed as a Svelte-store-contract observable ({@link backendStatus}
 *     on the singleton; `$supervisor.statuses` in components).
 *   - `check(backend?)`: probes the backends (opencode health endpoint /
 *     openrouter models fetch), coalescing concurrent checks per backend.
 *   - `ensureReady()`: picks the adapter for a request — configured backend
 *     when usable, otherwise the fallback when `allowFallback` (default
 *     true). Triggers a re-check when the last probe is older than
 *     {@link STALE_AFTER_MS} (60s).
 *   - Auto-retry with jittered exponential backoff (5s doubling, 5min cap):
 *     armed when a feature is requested while a backend is down. The loop
 *     only ever PROBES (probe-before-reconnect — no request is ever
 *     replayed automatically); a failed generate surfaces to the caller and
 *     the user retries manually.
 */

import type { AiBackend, AiConfig, AiResult } from "./types";
import { AiError } from "./types";
import type { AiGenerateRequest, AiProbeResult, AiProvider, ProviderSet } from "./provider";

/** A backend's observed connection state. */
export type ConnectionStatus = "unknown" | "checking" | "ok" | "down" | "unauthenticated";

/** Snapshot for one backend. */
export interface BackendStatus {
  status: ConnectionStatus;
  /** Epoch ms of the last completed probe (`null` = never probed). */
  lastCheck: number | null;
  /** Probe round-trip in ms (`null` when the probe never completed). */
  latencyMs: number | null;
  /** Safe (secret-free) failure description for the last failed probe. */
  error?: string;
}

/** Full status map (what subscribers see). */
export type BackendStatusMap = Record<AiBackend, BackendStatus>;

function initialStatus(): BackendStatus {
  return { status: "unknown", lastCheck: null, latencyMs: null };
}

function initialStatusMap(): BackendStatusMap {
  return { opencode: initialStatus(), openrouter: initialStatus() };
}

/** Injectable timer surface (tests use fake timers). */
export interface Scheduler {
  setTimeout(handler: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

const defaultScheduler: Scheduler = {
  setTimeout: (handler, ms) => setTimeout(handler, ms),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** Supervisor options (config getter + clock/timers injectable for tests). */
export interface SupervisorOptions {
  /** The two adapter instances. */
  providers: ProviderSet;
  /** Live non-secret config (backend preference, allowFallback). */
  config: () => Pick<AiConfig, "backend" | "allowFallback">;
  /** Epoch-ms clock (tests). */
  now?: () => number;
  /** Timer surface (tests). */
  scheduler?: Scheduler;
  /** Uniform [0,1) random for backoff jitter (tests; default Math.random). */
  jitter?: () => number;
}

/** First backoff delay (ms). */
export const RETRY_BASE_MS = 5_000;
/** Backoff cap (ms). */
export const RETRY_MAX_MS = 5 * 60_000;
/** A status older than this is re-probed before it is trusted (ms). */
export const STALE_AFTER_MS = 60_000;

/**
 * Connection supervisor. One instance per app; construct your own for
 * tests. Plain fields + manual listeners (`.ts` file — no runes here); the
 * `subscribe` method satisfies the Svelte store contract, so components
 * can read `$supervisor.statuses` reactively.
 */
export class ConnectionSupervisor {
  /** Reactive-by-subscription status map. */
  statuses: BackendStatusMap = initialStatusMap();

  readonly #providers: ProviderSet;
  readonly #getConfig: SupervisorOptions["config"];
  readonly #now: () => number;
  readonly #scheduler: Scheduler;
  readonly #jitter: () => number;

  readonly #listeners = new Set<(statuses: BackendStatusMap) => void>();
  /** In-flight checks per backend (coalescing). */
  readonly #checking = new Map<AiBackend, Promise<void>>();
  /** Consecutive failed probes per backend (backoff exponent). */
  readonly #attempts = new Map<AiBackend, number>();
  /** Armed backoff timers per backend. */
  readonly #timers = new Map<AiBackend, unknown>();

  constructor(options: SupervisorOptions) {
    this.#providers = options.providers;
    this.#getConfig = options.config;
    this.#now = options.now ?? Date.now;
    this.#scheduler = options.scheduler ?? defaultScheduler;
    this.#jitter = options.jitter ?? Math.random;
  }

  // -- store contract -------------------------------------------------------

  /** Svelte store contract: subscribe to the status map. */
  subscribe(listener: (statuses: BackendStatusMap) => void): () => void {
    this.#listeners.add(listener);
    listener(this.statuses);
    return () => this.#listeners.delete(listener);
  }

  #emit(): void {
    // Fresh object per emission so `$:` / `$derived` see a change.
    this.statuses = {
      opencode: { ...this.statuses.opencode },
      openrouter: { ...this.statuses.openrouter },
    };
    for (const listener of this.#listeners) listener(this.statuses);
  }

  #setStatus(backend: AiBackend, patch: Partial<BackendStatus>): void {
    this.statuses = {
      ...this.statuses,
      [backend]: { ...this.statuses[backend], ...patch },
    };
    for (const listener of this.#listeners) listener(this.statuses);
  }

  /** Snapshot getter (no subscription). */
  status(backend: AiBackend): BackendStatus {
    return this.statuses[backend];
  }

  /**
   * Records a transport-level liveness signal (M12: opencode SSE events).
   * Transport health is not data freshness — this only refreshes
   * `lastCheck` when the backend already reads ok, and asks for a real
   * probe when it reads down (probe-before-reconnect, so a server that
   * starts emitting events again recovers on the supervisor's normal
   * coalesced path instead of being trusted blindly).
   */
  noteTransportEvent(backend: AiBackend): void {
    const status = this.statuses[backend];
    if (status.status === "unknown" || status.status === "checking") return;
    if (status.status === "down") {
      void this.check(backend);
      return;
    }
    this.#setStatus(backend, { lastCheck: this.#now() });
  }

  // -- probing ----------------------------------------------------------------

  /** Probes one backend via its adapter's `check()` (or `available()`). */
  async #probe(provider: AiProvider): Promise<AiProbeResult> {
    if (provider.check) return provider.check();
    const ok = await provider.available();
    return ok ? { status: "ok" } : { status: "down", error: "backend unavailable" };
  }

  /**
   * Probes `backend` and records the result. Concurrent checks for the
   * same backend coalesce into one probe; a check while the backoff timer
   * is armed simply runs early (the timer is cleared, backoff re-arms from
   * the fresh result).
   */
  async check(backend: AiBackend): Promise<void> {
    const inFlight = this.#checking.get(backend);
    if (inFlight) return inFlight;

    const run = (async () => {
      this.#disarmRetry(backend);
      this.#setStatus(backend, { status: "checking", error: undefined });
      const started = this.#now();
      let result: AiProbeResult;
      try {
        result = await this.#probe(this.#providers[backend]);
      } catch (err) {
        result = {
          status: "down",
          error: err instanceof Error ? err.message : String(err),
        };
      }
      const latency = this.#now() - started;
      if (result.status === "ok") {
        this.#attempts.delete(backend);
        this.#setStatus(backend, {
          status: "ok",
          lastCheck: this.#now(),
          latencyMs: latency,
          error: undefined,
        });
      } else {
        const attempts = (this.#attempts.get(backend) ?? 0) + 1;
        this.#attempts.set(backend, attempts);
        this.#setStatus(backend, {
          status: result.status,
          lastCheck: this.#now(),
          latencyMs: latency,
          error: result.error,
        });
        // Down may fix itself (server starts up) → keep probing with
        // backoff. Unauthenticated needs a user action (credentials) —
        // retrying the probe would never recover, so it stays quiet.
        if (result.status === "down") this.#armRetry(backend, attempts);
      }
    })();

    this.#checking.set(backend, run);
    try {
      await run;
    } finally {
      // Only clear if this is still the run that owns the slot (a newer
      // coalesced call must not be forgotten by this one finishing).
      if (this.#checking.get(backend) === run) this.#checking.delete(backend);
    }
  }

  /** Probes both backends (settings "test connection", initial refresh). */
  async checkAll(): Promise<void> {
    await Promise.allSettled([
      this.check("opencode"),
      this.check("openrouter"),
    ]);
  }

  // -- backoff loop (probe-only, never replays requests) ---------------------

  /** Delay before retry number `attempt` (1-based), jittered ±10%. */
  #backoffDelay(attempt: number): number {
    const base = Math.min(RETRY_BASE_MS * 2 ** (attempt - 1), RETRY_MAX_MS);
    const jitterFactor = 1 + (this.#jitter() * 2 - 1) * 0.1; // 0.9 … 1.1
    return Math.max(1, Math.round(base * jitterFactor));
  }

  /** Arms the next probe for a down backend (no-op when already armed). */
  #armRetry(backend: AiBackend, attempt: number): void {
    if (this.#timers.has(backend)) return;
    const handle = this.#scheduler.setTimeout(() => {
      this.#timers.delete(backend);
      void this.check(backend);
    }, this.#backoffDelay(attempt));
    this.#timers.set(backend, handle);
  }

  /** Cancels an armed retry timer (check ran, or shutdown). */
  #disarmRetry(backend: AiBackend): void {
    const handle = this.#timers.get(backend);
    if (handle === undefined) return;
    this.#scheduler.clearTimeout(handle);
    this.#timers.delete(backend);
  }

  /** Stops every armed retry loop (test/teardown helper). */
  stopRetries(): void {
    for (const backend of [...this.#timers.keys()]) this.#disarmRetry(backend);
  }

  // -- request routing ---------------------------------------------------------

  /** True when `backend`'s status is fresh enough to trust without probing. */
  #fresh(backend: AiBackend): boolean {
    const status = this.statuses[backend];
    return (
      status.status !== "unknown" &&
      status.lastCheck !== null &&
      this.#now() - status.lastCheck < STALE_AFTER_MS
    );
  }

  /** The adapter for `backend`. */
  provider(backend: AiBackend): AiProvider {
    return this.#providers[backend];
  }

  /**
   * Ensures some backend is usable for a feature request and returns its
   * adapter: the configured backend when fresh-ok (probing first when the
   * status is stale/unknown), otherwise the fallback when
   * `allowFallback` !== false and the other backend is usable. While a
   * configured backend is down, arms the probe-only backoff loop.
   *
   * Throws `AiError` (`unavailable` / `unauthenticated`) when nothing is
   * usable — the CALLER decides whether to surface that to the user.
   * A failed request is NEVER retried here (no auto-replay): the user
   * retries manually.
   */
  async ensureReady(): Promise<AiProvider> {
    const config = this.#getConfig();
    const preferred = config.backend;

    if (!this.#fresh(preferred)) await this.check(preferred);
    const preferredStatus = this.statuses[preferred].status;
    if (preferredStatus === "ok") return this.#providers[preferred];

    const allowFallback = config.allowFallback !== false;
    const other: AiBackend = preferred === "opencode" ? "openrouter" : "opencode";
    if (allowFallback) {
      if (!this.#fresh(other)) await this.check(other);
      if (this.statuses[other].status === "ok") return this.#providers[other];
    }

    const preferredError = this.statuses[preferred].error;
    const failed =
      preferredStatus === "unauthenticated" || this.statuses[other].status === "unauthenticated";
    throw new AiError(
      failed ? "unauthenticated" : "unavailable",
      failed
        ? (preferredError ?? `AI backend "${preferred}" needs credentials — check AI settings`)
        : (preferredError ??
            `AI backend "${preferred}" is unavailable${allowFallback ? " and fallback is unavailable" : " (fallback disabled)"}`),
      { backend: preferred },
    );
  }

  /**
   * Routes one generation through the selected adapter. Failures propagate
   * verbatim — no automatic retry/replay. When the failure says the backend
   * went away, the status is flipped to `down` so the NEXT request
   * re-probes before using it again (probe-before-reconnect).
   */
  async generate(req: AiGenerateRequest): Promise<AiResult> {
    const provider = await this.ensureReady();
    try {
      return await provider.generate(req);
    } catch (err) {
      if (err instanceof AiError && err.kind === "unavailable") {
        this.#setStatus(provider.id, {
          status: "down",
          lastCheck: this.#now(),
          error: err.message,
        });
        this.#armRetry(provider.id, this.#attempts.get(provider.id) ?? 1);
      }
      throw err;
    }
  }
}
