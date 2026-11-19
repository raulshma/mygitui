/**
 * AI store (Svelte 5 runes) — config, opt-in gating, per-repo run state.
 *
 * Non-secret config persists to localStorage under `mygitui.ai`
 * (`{backend, opencodeUrl, opencodeModel, openrouterModel, allowFallback, repoOptIn}`).
 * SECRETS (OpenRouter API key, opencode basic-auth password) are NEVER
 * written to localStorage — they live in the OS keyring via the
 * `secrets_set`/`secrets_get` IPC commands; this store only forwards
 * save/clear calls.
 *
 * Per-repo generate state (`busy`/`error`/`last`) drives the feature
 * buttons' spinners and inline errors. `run` refuses to fire unless the
 * repo is opted in (the first-use dialog lives in the feature buttons) —
 * except for a one-shot run (`{ oneShot: true }`), which carries the
 * dialog's "Just once" consent for that single run without persisting it.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Tests construct isolated instances with an in-memory storage and mock
 * providers; the {@link ai} singleton wires the real backends.
 */

import {
  secretsDelete,
  secretsGet,
  secretsSet,
  SECRET_KEYS,
  opencodeServeStart,
  opencodeServeStatus,
  opencodeServeStop,
} from "$lib/ipc/client";
import { isTauri } from "$lib/entry/dragdrop";
import { SSH_SCHEME, tabStore } from "$lib/stores/tabs.svelte";
import { ConnectionSupervisor } from "./connection";
import type { SupervisorOptions } from "./connection";
import { OpenCodeProvider } from "./opencode";
import type { ManagedServe } from "./opencode";
import { OpenRouterProvider } from "./openrouter";
import { runFeature } from "./features";
import type { AiRunHooks, FeatureCtx, FeatureDeps, FeatureGitClient, FeatureOutcome } from "./features";
import { defaultAiConfig, AiError, isAiError, resolveAgenticBudget, resolveOpencode } from "./types";
import type { AiAgenticBudget, AiBackend, AiConfig, AiResult, FeatureKind, OpencodeMode } from "./types";
import type { ProviderSet, ResolvedConnection, Unsubscribe } from "./provider";
import { STALE_AFTER_MS } from "./connection";
import { AiJobStore } from "./jobs.svelte";
import type { AiJobTicket } from "./jobs.svelte";
import type { AgenticRunController } from "./agenticRun.svelte";
import { modelChoice } from "./modelChoice.svelte";
import { ChatStore } from "./chat.svelte";
import type { ChatContextSelection, ChatMessage } from "./chat.svelte";
import { aiFeatureLabel, featureWantsStreaming } from "./registry";
import type { AiFeatureTag } from "./registry";

import { describeError } from "$lib/errors";

/** localStorage key for the non-secret AI config. */
export const AI_STORAGE_KEY = "mygitui.ai";

/** Minimal storage surface this store needs (subset of DOM `Storage`). */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem?(key: string): void;
}

/**
 * Workspace for a managed-serve start. The opencode CLI is spawned
 * locally, so a remote tab's `ssh://` root is not a usable workspace —
 * pass `undefined` and the Rust side falls back to its default scratch
 * workspace (the same path it takes when no repository is active).
 */
export function localServeCwd(root: string | null | undefined): string | undefined {
  return typeof root === "string" && !root.startsWith(SSH_SCHEME) ? root : undefined;
}

/**
 * Managed opencode serve bridge (Rust IPC). The provider only receives it
 * inside a Tauri webview; everywhere else managed mode degrades to attach.
 */
const managedServeBridge: ManagedServe = {
  status: () => opencodeServeStatus(),
  start: (cwd) => opencodeServeStart(localServeCwd(cwd ?? tabStore.active?.root)),
};

function opencodeDirectory(sessionKey?: string): string | undefined {
  if (!sessionKey) return tabStore.active?.root;
  return tabStore.tabs.find((tab) => tab.id === sessionKey)?.root;
}

/** Per-repo state of a feature run. */
export interface AiGenerateState {
  busy: boolean;
  /** Safe error message from the last failed run (`null` when none). */
  error: string | null;
  /** Last successful outcome (`null` until one lands). */
  last: FeatureOutcome | null;
}

function freshState(): AiGenerateState {
  return { busy: false, error: null, last: null };
}

/** Per-run options. */
export interface AiRunOptions {
  /**
   * Consent for exactly this run without persisting the opt-in ("Just
   * once" in the first-use dialog — the dialog itself is the consent).
   * The run's own Retry keeps the consent; anything re-asking the
   * dialog does not.
   */
  oneShot?: boolean;
  /**
   * Per-run model override from the launcher's model picker (item 55).
   * Merged over `ctx.model` (picker wins — ctx.model is a runner-internal
   * default) and forwarded to the provider request, which resolves the
   * override over its configured settings default. Empty/undefined runs
   * on the configured default.
   */
  model?: string;
}

/** Input for one agentic run ({@link AiStore.runAgentic}). */
export interface AiAgenticRunInput {
  /** Repository the run acts on (workspace + opt-in + job key). */
  repoId: string;
  /** The task prompt. */
  prompt: string;
  /** Optional instructions — folded into the text for sessions this run creates. */
  system?: string;
  /** Per-run model override (picker choice; reused sessions switch first). */
  model?: string;
  /**
   * Reuse a persistent session (chat threads bind one 1:1); absent =
   * throwaway create+remove. A session-lost failure re-creates one and
   * the result carries the NEW id to persist.
   */
  sessionID?: string;
  /**
   * CREATE-and-keep: with no `sessionID`, the run makes its own session
   * and leaves it alive (the caller binds the returned id — the FIRST
   * message of a persistent thread). The chat seam always sets this.
   */
  keepSession?: boolean;
  /** Which job label this run carries ("chat" now, diagnose in chunk 4). */
  feature: AiFeatureTag;
  /**
   * One-shot consent ("Just once") covering THIS run without persisting
   * the repo opt-in — same semantics as single-shot's AiRunOptions.
   */
  oneShot?: boolean;
  /**
   * Extra observer for the run's controller (fires synchronously at
   * dispatch — the chat uses it to mark the run as started, so post-start
   * refusals surface honestly instead of degrading to single-shot).
   */
  onController?: (controller: AgenticRunController) => void;
  /** Extra stream observer (the fallback helper forwards the caller's sink). */
  onDelta?: (delta: string) => void;
  /**
   * Adopt an already-begun ticket (chat turns own their job lifecycle —
   * begin, settle, partial text): the runner then only routes and reports;
   * the caller settles. Absent: the runner begins and settles its own
   * ticket (chunk 2 behavior).
   */
  ticket?: AiJobTicket;
}

/** Shared frozen default for {@link AiStore.peekState} reads (never mutated). */
const FRESH_STATE: AiGenerateState = freshState();

/** Injectable dependencies (tests). */
export interface AiStoreDeps {
  /** Storage for the non-secret config; `null` disables persistence. */
  storage?: StorageLike | null;
  /** Provider overrides (tests mock the backends). */
  providers?: Partial<ProviderSet>;
  /** Job store override (tests isolate the run history). */
  jobs?: AiJobStore;
  /** Chat store override (tests isolate assistant conversations). */
  chat?: ChatStore;
  /** Supervisor option overrides (tests). */
  supervisor?: Partial<Pick<SupervisorOptions, "now" | "scheduler" | "jitter">>;
  /** Set `false` to disable the opencode SSE liveness stream (tests). */
  events?: false;
  /**
   * Overrides the `isTauri()` desktop-environment check (tests). Gates the
   * SSE liveness loop and the managed-mode wiring: `() => true` simulates
   * the real app (liveness runs against whatever providers are wired),
   * `() => false` forces the browser/test posture.
   */
  isTauri?: () => boolean;
}

/** Parses persisted config; drops malformed entries, fills defaults. */
export function parseAiConfig(raw: string | null): AiConfig {
  const base = defaultAiConfig();
  if (!raw) return base;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return base;
  }
  if (typeof parsed !== "object" || parsed === null) return base;
  const obj = parsed as Record<string, unknown>;
  const config: AiConfig = base;
  if (obj.backend === "opencode" || obj.backend === "openrouter") {
    config.backend = obj.backend;
  }
  if (typeof obj.opencodeUrl === "string" && obj.opencodeUrl.length > 0) {
    config.opencodeUrl = obj.opencodeUrl;
  }
  if (typeof obj.opencodeModel === "string" && obj.opencodeModel.length > 0) {
    config.opencodeModel = obj.opencodeModel;
  }
  if (typeof obj.openrouterModel === "string" && obj.openrouterModel.length > 0) {
    config.openrouterModel = obj.openrouterModel;
  }
  if (typeof obj.allowFallback === "boolean") {
    config.allowFallback = obj.allowFallback;
  }
  if (obj.opencodeMode === "managed" || obj.opencodeMode === "attach") {
    config.opencodeMode = obj.opencodeMode;
  }
  if (typeof obj.opencodeEnabled === "boolean") {
    config.opencodeEnabled = obj.opencodeEnabled;
  }
  if (typeof obj.enabled === "boolean" && !obj.enabled) {
    // Master switch is opt-out: only persist an explicit "off".
    config.enabled = false;
  }
  if (typeof obj.statusBar === "boolean" && !obj.statusBar) {
    config.statusBar = false;
  }
  if (typeof obj.agenticBudget === "object" && obj.agenticBudget !== null) {
    const raw = obj.agenticBudget as Record<string, unknown>;
    const budget: AiAgenticBudget = {};
    if (typeof raw.maxWallClockMs === "number" && raw.maxWallClockMs > 0) {
      budget.maxWallClockMs = raw.maxWallClockMs;
    }
    if (typeof raw.maxToolCalls === "number" && raw.maxToolCalls > 0) {
      budget.maxToolCalls = raw.maxToolCalls;
    }
    if (budget.maxWallClockMs !== undefined || budget.maxToolCalls !== undefined) {
      config.agenticBudget = budget;
    }
  }
  if (typeof obj.features === "object" && obj.features !== null) {
    for (const [kind, value] of Object.entries(obj.features as Record<string, unknown>)) {
      if (typeof value === "boolean") {
        (config.features ??= {})[kind] = value;
      }
    }
  }
  if (typeof obj.repoOptIn === "object" && obj.repoOptIn !== null) {
    for (const [repoId, value] of Object.entries(obj.repoOptIn as Record<string, unknown>)) {
      if (typeof value === "boolean") config.repoOptIn[repoId] = value;
    }
  }
  return config;
}

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export class AiStore {
  /** Non-secret configuration; persisted on every change. */
  config: AiConfig = $state(defaultAiConfig());
  /** Per-repo feature-run state (`busy`/`error`/`last`). */
  generateState: Record<string, AiGenerateState> = $state({});
  /**
   * The transparency engine (M13): every `run` becomes an observable
   * `AiJob` — status-bar rows, cancel, activity history. Shares this
   * store's storage so tests isolate both with one injectable.
   */
  readonly jobs: AiJobStore;
  /**
   * The repo-aware assistant (M13): per-root conversations, own busy slot
   * (a chat never blocks feature runs), same master-switch + opt-in gates.
   */
  readonly chat: ChatStore;

  readonly #storage: StorageLike | null;
  readonly #supervisor: ConnectionSupervisor;

  constructor(deps: AiStoreDeps = {}) {
    this.#storage = deps.storage === undefined ? defaultStorage() : deps.storage;
    this.config = parseAiConfig(this.#storage?.getItem(AI_STORAGE_KEY) ?? null);
    this.jobs = deps.jobs ?? new AiJobStore({ storage: this.#storage });
    // Injectable desktop check (tests): gates the liveness loop and the
    // managed-mode wiring below.
    const live = deps.isTauri ?? isTauri;

    const providers: ProviderSet = {
      opencode:
        deps.providers?.opencode ??
        new OpenCodeProvider({
          // Live getters: settings changes apply without rebuilding.
          resolve: () => {
            const resolved = resolveOpencode(this.config);
            // Outside Tauri there is no serve registry: managed degrades
            // to the v1 attach autodiscovery (browser dev, tests).
            return resolved === "managed" && !live() ? "attach" : resolved;
          },
          url: () => this.config.opencodeUrl ?? undefined,
          defaultModel: () => this.config.opencodeModel,
          // Agentic-run budget caps (M13), read live per run so settings
          // changes apply without rebuilding the provider.
          agenticBudget: () => resolveAgenticBudget(this.config),
          managed: () => (live() ? managedServeBridge : null),
          directory: opencodeDirectory,
          password: () => secretsGet(SECRET_KEYS.opencodeServerPassword),
        }),
      openrouter: deps.providers?.openrouter ?? new OpenRouterProvider(),
    };
    this.#supervisor = new ConnectionSupervisor({
      providers,
      config: () => this.config,
      ...deps.supervisor,
    });
    this.chat =
      deps.chat ??
      new ChatStore({
        storage: this.#storage,
        jobs: this.jobs,
        router: this.#supervisor,
        // Chat respects the same gates as feature runs: the master switch
        // and per-repo opt-in, read live so settings apply instantly.
        enabled: () => this.isEnabled(),
        isOptedIn: (repoId) => this.isOptedIn(repoId),
        // Agentic chat (M13 chunk 3): the turn's ticket is adopted by the
        // runner (caller owns begin + settle); the thread binds one
        // opencode session 1:1 and the per-repo model choice applies.
        agentic: (input) =>
          this.runAgentic({
            repoId: input.repoId,
            prompt: input.prompt,
            system: input.system,
            model: input.model,
            sessionID: input.sessionID,
            // Chat threads are persistent: the FIRST agentic message
            // creates the session and binds the returned id.
            keepSession: true,
            feature: "chat",
            ticket: input.ticket,
            onController: input.onController,
          }),
        deleteSession: (sessionID) => this.#removeAgenticSession(sessionID),
        model: (repoId) => modelChoice.overrideFor(repoId),
      });

    // M12: opencode SSE liveness — while the server streams its event
    // endpoint, every decoded event refreshes transport freshness via the
    // supervisor. The subscription is provider-owned
    // (`subscribeLiveness`): the v1/v2 stream path and the auth header are
    // resolved INSIDE the provider, fresh per (re)connection attempt, so
    // a managed-server restart's rotated password is never captured stale.
    // Retry stays with the supervisor: the stream never reconnects itself;
    // the next successful probe re-subscribes. Gated to the real desktop
    // app (injectable, see `live`): tests/browser must not fetch or spawn
    // anything at module import.
    if (deps.events !== false && live()) {
      this.#supervisor.subscribe((statuses) => {
        if (statuses.opencode.status === "ok") this.#ensureEvents();
      });
      // M12: proactive startup probe (managed mode also spawns the server
      // here), so the health chip reflects reality immediately instead of
      // staying gray "not configured" until the first AI click.
      void this.supervisor.checkAll().catch(() => {
        // Probe failures are recorded on the supervisor; nothing to do.
      });
    }
  }

  /** Live SSE liveness subscription, when one is running (else `null`). */
  #unsubLiveness: Unsubscribe | null = null;

  /** Starts the SSE liveness loop once per "opencode became ok" transition. */
  #ensureEvents(): void {
    if (this.#unsubLiveness) return;
    // Capability-gated: providers without the liveness seam (OpenRouter,
    // test doubles) simply never stream. The provider resolves the
    // connection ITSELF — fresh per (re)connection attempt, auth header
    // re-derived — so nothing secret is frozen into a closure here.
    const provider = this.#supervisor.provider("opencode");
    if (!provider.subscribeLiveness) return;
    this.#unsubLiveness = provider.subscribeLiveness((down) => {
      if (down) {
        // Stream ended (or never opened): drop it; the next ok transition
        // resubscribes — with a freshly resolved connection.
        this.#unsubLiveness = null;
      } else {
        // Decoded event: transport alive → refresh supervisor freshness.
        this.#supervisor.noteTransportEvent("opencode");
      }
    });
  }

  // -- configuration ---------------------------------------------------------

  /** The connection supervisor (status store + request routing). */
  get supervisor(): ConnectionSupervisor {
    return this.#supervisor;
  }

  #persist(): void {
    if (!this.#storage) return;
    try {
      this.#storage.setItem(AI_STORAGE_KEY, JSON.stringify(this.config));
    } catch {
      // Storage may be full or unavailable; keep the in-memory config usable.
    }
  }

  /** Switches the preferred backend. */
  setBackend(backend: AiBackend): void {
    this.config.backend = backend;
    this.#persist();
  }

  /** Sets the manual opencode URL (empty/whitespace clears it). */
  setOpencodeUrl(url: string | undefined): void {
    const trimmed = url?.trim();
    if (trimmed) this.config.opencodeUrl = trimmed;
    else delete this.config.opencodeUrl;
    this.#persist();
  }

  /** Sets the default OpenCode model (empty/whitespace uses the server default). */
  setOpencodeModel(model: string | undefined): void {
    const trimmed = model?.trim();
    if (trimmed) this.config.opencodeModel = trimmed;
    else delete this.config.opencodeModel;
    this.#persist();
  }

  /** Sets how the opencode server is reached ("managed" | "attach"). */
  setOpencodeMode(mode: OpencodeMode): void {
    this.config.opencodeMode = mode;
    this.#persist();
  }

  /**
   * Enables/disables the opencode backend (default on). Disabling stops
   * the managed server immediately; enabling re-probes (managed mode
   * spawns on demand through the supervisor check).
   */
  async setOpencodeEnabled(enabled: boolean): Promise<void> {
    if (enabled) delete this.config.opencodeEnabled;
    else this.config.opencodeEnabled = false;
    this.#persist();
    if (enabled) {
      await this.supervisor.check("opencode").catch(() => {});
    } else {
      await opencodeServeStop().catch(() => {});
      await this.supervisor.check("opencode").catch(() => {});
    }
  }

  /** Sets the default OpenRouter model (empty/whitespace clears it). */
  setOpenrouterModel(model: string | undefined): void {
    const trimmed = model?.trim();
    if (trimmed) this.config.openrouterModel = trimmed;
    else delete this.config.openrouterModel;
    this.#persist();
  }

  /** Toggles falling back to the other backend when the preferred is down. */
  setAllowFallback(allow: boolean): void {
    this.config.allowFallback = allow;
    this.#persist();
  }

  // -- master + per-feature switches (M13) --------------------------------------

  /** True when AI is on at all (`enabled` is opt-out; default on). */
  isEnabled(): boolean {
    return this.config.enabled !== false;
  }

  /**
   * Master switch (the "turn it off" control): hides every AI affordance
   * and cancels in-flight jobs. Enabling re-arms nothing automatically —
   * the next run probes backends through the supervisor as usual.
   */
  setEnabled(enabled: boolean): void {
    if (enabled) delete this.config.enabled;
    else this.config.enabled = false;
    this.#persist();
    if (!enabled) this.jobs.cancelAll();
  }

  /**
   * True when `kind` is on (master switch AND per-feature toggle). Takes
   * the wide tag so `"chat"` reads the same per-feature fields.
   */
  isFeatureEnabled(kind: AiFeatureTag): boolean {
    return this.isEnabled() && this.config.features?.[kind] !== false;
  }

  /** Per-feature toggle (settings renders these from `AI_FEATURES`). */
  setFeatureEnabled(kind: FeatureKind, enabled: boolean): void {
    const features = (this.config.features ??= {});
    if (enabled) delete features[kind];
    else features[kind] = false;
    this.#persist();
  }

  /** Live AI job rows in the status bar (default on). */
  isStatusBarEnabled(): boolean {
    return this.config.statusBar !== false;
  }

  setStatusBarEnabled(enabled: boolean): void {
    if (enabled) delete this.config.statusBar;
    else this.config.statusBar = false;
    this.#persist();
  }

  /**
   * Sets the agentic-run budget caps (M13); `undefined` restores the
   * defaults. Read live by the opencode provider per agentic run —
   * see `resolveAgenticBudget`.
   */
  setAgenticBudget(budget: AiAgenticBudget | undefined): void {
    if (budget === undefined) delete this.config.agenticBudget;
    else this.config.agenticBudget = budget;
    this.#persist();
  }

  // -- secrets (OS keyring; NEVER localStorage) --------------------------------

  /** Stores the OpenRouter API key in the OS keyring. */
  async saveOpenrouterKey(key: string): Promise<void> {
    await secretsSet(SECRET_KEYS.openrouterApiKey, key);
  }

  /** Stores the opencode basic-auth password in the OS keyring. */
  async saveOpencodePassword(password: string): Promise<void> {
    await secretsSet(SECRET_KEYS.opencodeServerPassword, password);
  }

  /** Removes the OpenRouter API key from the OS keyring. */
  async clearOpenrouterKey(): Promise<void> {
    await secretsDelete(SECRET_KEYS.openrouterApiKey);
  }

  /** Removes the opencode basic-auth password from the OS keyring. */
  async clearOpencodePassword(): Promise<void> {
    await secretsDelete(SECRET_KEYS.opencodeServerPassword);
  }

  /** Resolves a secret by key (settings prefill never displays it back). */
  async readSecret(key: string): Promise<string | null> {
    return secretsGet(key);
  }

  // -- per-repo opt-in -----------------------------------------------------------

  /** True when AI features are allowed for `repoId` (default: no). */
  isOptedIn(repoId: string): boolean {
    return this.config.repoOptIn[repoId] === true;
  }

  /** Allows or revokes AI features for `repoId` (persisted). */
  setOptIn(repoId: string, allow: boolean): void {
    if (allow) this.config.repoOptIn[repoId] = true;
    else delete this.config.repoOptIn[repoId];
    this.#persist();
  }

  /**
   * Run options for a post-consent run: carries the "Just once" consent
   * (`{ oneShot: true }`) when the repo is not persisted-opted-in — only
   * possible right after the dialog's consent — else `undefined`.
   */
  runOptsFor(repoId: string): AiRunOptions | undefined {
    return this.isOptedIn(repoId) ? undefined : { oneShot: true };
  }

  // -- agentic runs (M13 chunk 2) ------------------------------------------------

  /**
   * Runs one agentic job: a job ticket + the opencode provider's
   * `runAgentic` path, with the approval controller bound to the job the
   * moment the run's session exists (the transcript and approval cards
   * read the controller's reactive state through `job.agentic`). Partial
   * text streams into the ticket so a cancel keeps what arrived.
   *
   * Gates: the same master switch + per-repo opt-in as `run`. No backend
   * fallback — agentic runs need the opencode backend (OpenRouter has no
   * tool loop); an unusable server surfaces as the usual AiError. Real
   * invocations (agent chat, diagnose) wire in chunks 3/4 and call THIS.
   */
  async runAgentic(input: AiAgenticRunInput): Promise<AiResult & { sessionID: string }> {
    if (!this.isEnabled()) {
      throw new AiError("opt-in", "AI features are turned off in AI settings", {});
    }
    // One-shot consent ("Just once" in the first-use dialog) covers THIS
    // run exactly like single-shot's run() — it is never persisted.
    if (!input.oneShot && !this.isOptedIn(input.repoId)) {
      throw new AiError(
        "opt-in",
        "AI features are not enabled for this repository — allow them when prompted or in AI settings",
        {},
      );
    }
    const provider = this.#supervisor.provider("opencode");
    if (typeof (provider as { runAgentic?: unknown }).runAgentic !== "function") {
      throw new AiError(
        "unavailable",
        "agentic runs need the opencode backend — switch backends in AI settings",
        { backend: "opencode" },
      );
    }

    // Adopted ticket (chat turns): the caller owns begin + settle and the
    // turn's partial text; the runner only routes and reports into it.
    const adopt = input.ticket !== undefined;
    const ticket = input.ticket ?? this.beginAgenticTicket(input);
    if (!adopt) ticket.phase("generating");
    try {
      const run = await (provider as OpenCodeProvider).runAgentic({
        system: input.system,
        prompt: input.prompt,
        model: input.model,
        sessionID: input.sessionID,
        keepSession: input.keepSession,
        sessionKey: input.repoId,
        signal: ticket.signal,
        onServing: (backend, model) => ticket.serving(backend, model),
        onDelta: (delta) => {
          ticket.delta(delta);
          input.onDelta?.(delta);
        },
        onController: (controller) => {
          this.jobs.bindAgentic(ticket.id, controller);
          input.onController?.(controller);
        },
      });
      if (!adopt) {
        ticket.done({
          backend: "opencode",
          model: run.model,
          elapsedMs: run.elapsedMs,
          usage: run.usage ?? null,
          text: run.text,
        });
      }
      return {
        text: run.text,
        model: run.model,
        backend: "opencode",
        elapsedMs: run.elapsedMs,
        sessionID: run.sessionID,
        ...(run.usage !== undefined ? { usage: run.usage } : {}),
      };
    } catch (err) {
      if (!adopt) {
        if (isAiError(err) && err.kind === "cancelled") ticket.cancelled();
        else if (this.jobs.cancelRequested(ticket.id)) ticket.cancelled();
        else ticket.failed(describeError(err));
      }
      throw err;
    }
  }

  /** Begins the runner-owned ticket for one agentic run. */
  private beginAgenticTicket(input: AiAgenticRunInput): AiJobTicket {
    const repoName = tabStore.tabs.find((tab) => tab.id === input.repoId)?.name ?? null;
    return this.jobs.begin({
      repoId: input.repoId,
      repoName,
      feature: input.feature,
    });
  }

  /**
   * Resolves the agentic connection, or `null` when an agentic run cannot
   * be served right now: an OpenRouter-configured backend (agentic runs
   * are opencode-only by design — this also keeps the consent copy
   * honest, since the run can never go agentic while the copy says
   * OpenRouter), no runAgentic capability, or no 2.x server. OpenCode
   * probes consult the supervisor's coalesced state first (a fresh-ok
   * 60s status skips the health check) so repeated clicks do not
   * triple-probe.
   */
  async #agenticConnection(): Promise<ResolvedConnection | null> {
    if (this.config.backend === "openrouter") return null;
    const provider = this.#supervisor.provider("opencode");
    if (typeof (provider as { runAgentic?: unknown }).runAgentic !== "function") {
      return null;
    }
    const status = this.#supervisor.status("opencode");
    const fresh =
      status.status !== "unknown" &&
      status.lastCheck !== null &&
      Date.now() - status.lastCheck < STALE_AFTER_MS;
    if (!fresh) await this.#supervisor.check("opencode").catch(() => {});
    try {
      const conn = (await provider.connection?.()) ?? null;
      // A 1.x server answers but cannot serve agentic runs.
      return conn !== null && conn.serverKind === "v2" ? conn : null;
    } catch {
      return null;
    }
  }

  /**
   * Whether an agentic run can be served RIGHT NOW (see
   * {@link #agenticConnection}). The diagnose views do not call this
   * separately — {@link runAgenticWithFallback} resolves the connection
   * itself, once per invocation.
   */
  async agenticAvailable(): Promise<boolean> {
    return (await this.#agenticConnection()) !== null;
  }

  /**
   * One agentic attempt with the ADR-0001 degrade contract for
   * per-invocation features (diagnose views): agentic when servable; a
   * PRE-START refusal silently falls back to the given single-shot
   * runner; a POST-START failure (onController fired — the run dispatched)
   * surfaces honestly instead of re-executing the request on another
   * backend. Owns the agentic attempt's ticket (a refused attempt settles
   * CANCELLED — nothing ran — so the fallback never leaves a failed row
   * next to the single-shot's), wraps the attempt in the same per-repo
   * busy slot single-shot uses, and threads one-shot consent. `mode`
   * records what actually ran; agentic results carry the serving
   * `sessionID` (the views' mode marker reads it).
   */
  async runAgenticWithFallback(input: {
    repoId: string;
    prompt: string;
    system?: string;
    model?: string;
    feature: AiFeatureTag;
    oneShot?: boolean;
    onDelta?: (delta: string) => void;
    /** A pre-resolved connection (skips the capability probe). */
    connection?: ResolvedConnection;
    fallback: () => Promise<AiResult>;
  }): Promise<{ result: AiResult; mode: "agentic" | "single-shot" }> {
    const connection = input.connection ?? (await this.#agenticConnection());
    if (connection === null || connection.serverKind !== "v2") {
      return { result: await input.fallback(), mode: "single-shot" };
    }
    // Same concurrency gate as single-shot run() (pre-chunk-4 semantics):
    // an in-flight agentic diagnose blocks other runs for this repo.
    const state = this.stateFor(input.repoId);
    if (state.busy) {
      throw new AiError("unknown", "an AI request is already running for this repository", {});
    }
    const ticket = this.beginAgenticTicket(input);
    ticket.phase("generating");
    let runStarted = false;
    state.busy = true;
    try {
      const run = await this.runAgentic({
        repoId: input.repoId,
        prompt: input.prompt,
        system: input.system,
        model: input.model,
        feature: input.feature,
        oneShot: input.oneShot,
        ticket,
        onDelta: input.onDelta,
        onController: () => {
          runStarted = true;
        },
      });
      // Clear BEFORE the ticket settles/returns — no path may leave the
      // slot set (the single-shot fallback re-sets it itself).
      state.busy = false;
      ticket.done({
        backend: "opencode",
        model: run.model,
        elapsedMs: run.elapsedMs,
        usage: run.usage ?? null,
        text: run.text,
      });
      return { result: { ...run }, mode: "agentic" };
    } catch (err) {
      // Clear before the fallback too: single-shot's ai.run refuses while
      // the slot reads busy.
      state.busy = false;
      const preStart = err instanceof AiError && err.kind === "unavailable" && !runStarted;
      if (isAiError(err) && err.kind === "cancelled") ticket.cancelled();
      else if (this.jobs.cancelRequested(ticket.id)) ticket.cancelled();
      else if (preStart) ticket.cancelled();
      else ticket.failed(describeError(err));
      if (preStart) {
        return { result: await input.fallback(), mode: "single-shot" };
      }
      throw err;
    }
  }

  /**
   * Best-effort removal of one persistent agentic session (a chat thread
   * was deleted). Resolves when the server answered or the backend cannot
   * serve sessions; the chat store swallows every failure — a dead server
   * must never make thread deletion fail.
   */
  async #removeAgenticSession(sessionID: string): Promise<void> {
    const provider = this.#supervisor.provider("opencode");
    if (typeof (provider as { deleteAgenticSession?: unknown }).deleteAgenticSession !== "function") {
      return;
    }
    await (provider as OpenCodeProvider).deleteAgenticSession(sessionID);
  }

  // -- feature runs -----------------------------------------------------------------

  /** Run state for `repoId` (creates a fresh entry on read). */
  stateFor(repoId: string): AiGenerateState {
    return (this.generateState[repoId] ??= freshState());
  }

  /**
   * Read-only run-state peek for templates — never mutates `$state` during
   * a reactive read (returns a fresh object when no run happened yet).
   */
  peekState(repoId: string): AiGenerateState {
    return this.generateState[repoId] ?? FRESH_STATE;
  }

  /**
   * Runs one AI feature with gating and state management: refuses when AI
   * or the feature is switched off (AiError kind `opt-in`), refuses when
   * the repo is not opted in, refuses concurrent runs for the same repo,
   * records busy/error/last, and drives an {@link AiJobTicket} so the run
   * is visible (phase, elapsed, cancel) in the status bar and Activity
   * tab. Rethrows failures so the caller can react (toast etc.). No
   * automatic retries — the user re-invokes.
   */
  async run(
    kind: FeatureKind,
    ctx: FeatureCtx,
    git?: FeatureGitClient,
    opts?: AiRunOptions,
  ): Promise<FeatureOutcome> {
    if (!this.isEnabled()) {
      throw new AiError("opt-in", "AI features are turned off in AI settings", {});
    }
    if (!this.isFeatureEnabled(kind)) {
      throw new AiError("opt-in", `"${aiFeatureLabel(kind)}" is turned off in AI settings`, {});
    }
    const oneShot = opts?.oneShot === true;
    if (!oneShot && !this.isOptedIn(ctx.repoId)) {
      throw new AiError(
        "opt-in",
        "AI features are not enabled for this repository — allow them when prompted or in AI settings",
        {},
      );
    }
    const state = this.stateFor(ctx.repoId);
    if (state.busy) {
      throw new AiError("unknown", "an AI request is already running for this repository", {});
    }

    // Per-run model override (item 55): the launcher's picker rides the
    // existing job-start call — opts.model wins over a ctx.model default.
    const override = opts?.model?.trim();
    const runCtx: FeatureCtx =
      override !== undefined && override.length > 0 ? { ...ctx, model: override } : ctx;

    const deps: FeatureDeps = {
      git: git ?? (await defaultGitClient()),
      router: this.#supervisor,
      // One-shot consent satisfies runFeature's gate for this run only.
      isOptedIn: (repoId) => oneShot || this.isOptedIn(repoId),
    };

    // The job makes this run visible: status-bar row, phase, elapsed,
    // cancel, and an activity-history record afterwards.
    const repoName = tabStore.tabs.find((tab) => tab.id === runCtx.repoId)?.name ?? null;
    const ticket = this.jobs.begin({ repoId: runCtx.repoId, repoName, feature: kind });
    const hooks: AiRunHooks = {
      onPhase: (phase, message) => ticket.phase(phase, message),
      onPrompt: (system, prompt) => ticket.promptPreview(`${system}\n\n${prompt}`),
      onServing: (backend, model) => ticket.serving(backend, model),
      // Only stream-preferring features hook deltas (registry metadata):
      // structured one-shot outputs (branch-name, conflict-assist) parse
      // the finished text, so streaming them would spend tokens the
      // caller never renders progressively.
      onDelta: featureWantsStreaming(kind) ? (text) => ticket.delta(text) : undefined,
      signal: ticket.signal,
    };

    state.busy = true;
    state.error = null;
    try {
      const outcome = await runFeature(kind, runCtx, deps, hooks);
      state.last = outcome;
      ticket.done({
        backend: outcome.result.backend,
        model: outcome.result.model,
        elapsedMs: outcome.result.elapsedMs,
        usage: outcome.result.usage ?? null,
        text: outcome.result.text,
      });
      return outcome;
    } catch (err) {
      state.error = describeError(err);
      if (isAiError(err) && err.kind === "cancelled") ticket.cancelled();
      else if (this.jobs.cancelRequested(ticket.id)) ticket.cancelled();
      else ticket.failed(describeError(err));
      throw err;
    } finally {
      state.busy = false;
    }
  }

  /** Convenience: commit message from the repo's staged diff. */
  async generateCommitMessage(
    repoId: string,
    git?: FeatureGitClient,
    opts?: AiRunOptions,
  ): Promise<FeatureOutcome> {
    return this.run("commit-message", { repoId }, git, opts);
  }

  /**
   * Convenience: one assistant turn. Fills the tab name for the job's
   * status-bar row (the same lookup `run` does) and delegates to the chat
   * store — the thread is keyed by repo root, so callers must pass it.
   */
  async sendChat(opts: {
    repoId: string;
    root: string;
    question: string;
    context: ChatContextSelection;
    /** Composer's agentic chip (per-run override). */
    agentic?: boolean;
  }): Promise<ChatMessage> {
    const repoName = tabStore.tabs.find((tab) => tab.id === opts.repoId)?.name ?? null;
    return this.chat.send({ ...opts, repoName });
  }
}

/** Lazily wires the real git client (dynamic import keeps tests hermetic). */
async function defaultGitClient(): Promise<FeatureGitClient> {
  const client = await import("$lib/ipc/client");
  return {
    repoDiff: (repoId, oldSide, newSide) => client.repoDiff(repoId, oldSide, newSide),
    streamLog: (repoId, filter, onPage) => client.streamLog(repoId, filter, onPage),
  };
}

/** The application-wide AI store. */
export const ai = new AiStore();
