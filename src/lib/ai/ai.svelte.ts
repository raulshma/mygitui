/**
 * AI store (Svelte 5 runes) — config, opt-in gating, per-repo run state.
 *
 * Non-secret config persists to localStorage under `mygitui.ai`
 * (`{backend, opencodeUrl, openrouterModel, allowFallback, repoOptIn}`).
 * SECRETS (OpenRouter API key, opencode basic-auth password) are NEVER
 * written to localStorage — they live in the OS keyring via the
 * `secrets_set`/`secrets_get` IPC commands; this store only forwards
 * save/clear calls.
 *
 * Per-repo generate state (`busy`/`error`/`last`) drives the feature
 * buttons' spinners and inline errors. `run` refuses to fire unless the
 * repo is opted in (the first-use dialog lives in the feature buttons).
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Tests construct isolated instances with an in-memory storage and mock
 * providers; the {@link ai} singleton wires the real backends.
 */

import { secretsDelete, secretsGet, secretsSet, SECRET_KEYS } from "$lib/ipc/client";
import { ConnectionSupervisor } from "./connection";
import type { SupervisorOptions } from "./connection";
import { OpenCodeProvider } from "./opencode";
import { OpenRouterProvider } from "./openrouter";
import { runFeature } from "./features";
import type { FeatureCtx, FeatureDeps, FeatureGitClient, FeatureOutcome } from "./features";
import { defaultAiConfig, AiError } from "./types";
import type { AiBackend, AiConfig, FeatureKind } from "./types";
import type { ProviderSet } from "./provider";

/** localStorage key for the non-secret AI config. */
export const AI_STORAGE_KEY = "mygitui.ai";

/** Minimal storage surface this store needs (subset of DOM `Storage`). */
export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem?(key: string): void;
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

/** Shared frozen default for {@link AiStore.peekState} reads (never mutated). */
const FRESH_STATE: AiGenerateState = freshState();

/** Injectable dependencies (tests). */
export interface AiStoreDeps {
  /** Storage for the non-secret config; `null` disables persistence. */
  storage?: StorageLike | null;
  /** Provider overrides (tests mock the backends). */
  providers?: Partial<ProviderSet>;
  /** Supervisor option overrides (tests). */
  supervisor?: Partial<Pick<SupervisorOptions, "now" | "scheduler" | "jitter">>;
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
  if (typeof obj.openrouterModel === "string" && obj.openrouterModel.length > 0) {
    config.openrouterModel = obj.openrouterModel;
  }
  if (typeof obj.allowFallback === "boolean") {
    config.allowFallback = obj.allowFallback;
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

  readonly #storage: StorageLike | null;
  readonly #supervisor: ConnectionSupervisor;

  constructor(deps: AiStoreDeps = {}) {
    this.#storage = deps.storage === undefined ? defaultStorage() : deps.storage;
    this.config = parseAiConfig(this.#storage?.getItem(AI_STORAGE_KEY) ?? null);

    const providers: ProviderSet = {
      opencode:
        deps.providers?.opencode ??
        new OpenCodeProvider({
          // Live getter: settings changes apply without rebuilding.
          url: () => this.config.opencodeUrl ?? undefined,
          password: () => secretsGet(SECRET_KEYS.opencodeServerPassword),
        }),
      openrouter: deps.providers?.openrouter ?? new OpenRouterProvider(),
    };
    this.#supervisor = new ConnectionSupervisor({
      providers,
      config: () => this.config,
      ...deps.supervisor,
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
   * Runs one AI feature with gating and state management: refuses when the
   * repo is not opted in (AiError kind `opt-in`), refuses concurrent runs
   * for the same repo, records busy/error/last, and rethrows failures so
   * the caller can react (toast etc.). No automatic retries — the user
   * re-invokes.
   */
  async run(kind: FeatureKind, ctx: FeatureCtx, git?: FeatureGitClient): Promise<FeatureOutcome> {
    if (!this.isOptedIn(ctx.repoId)) {
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

    const deps: FeatureDeps = {
      git: git ?? (await defaultGitClient()),
      router: this.#supervisor,
      isOptedIn: (repoId) => this.isOptedIn(repoId),
    };

    state.busy = true;
    state.error = null;
    try {
      const outcome = await runFeature(kind, ctx, deps);
      state.last = outcome;
      return outcome;
    } catch (err) {
      state.error = err instanceof Error ? err.message : String(err);
      throw err;
    } finally {
      state.busy = false;
    }
  }

  /** Convenience: commit message from the repo's staged diff. */
  async generateCommitMessage(repoId: string, git?: FeatureGitClient): Promise<FeatureOutcome> {
    return this.run("commit-message", { repoId }, git);
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
