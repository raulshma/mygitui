/**
 * AI job store (M13, lane T1) — the transparency engine.
 *
 * Every AI run (feature or chat) becomes a first-class {@link AiJob} with
 * a visible lifecycle `queued → gathering → prompting → generating →
 * parsing → done | cancelled | error`. The store is the single place the
 * rest of the app observes:
 *
 *   - the status bar renders one row per entry in `active` (phase,
 *     elapsed, cancel — the same treatment git ops get from `op-progress`);
 *   - feature buttons read phase/elapsed for tooltips;
 *   - the assistant panel's Activity tab lists `history` with usage.
 *
 * Active jobs are reactive `$state` (deep) so incremental mutations —
 * phase updates, streamed text — re-render consumers. Terminal states are
 * moved to a capped `history` ring (newest first) persisted under
 * `mygitui.ai.activity` in the versioned envelope
 * (`$lib/state/persist`). Stored text is capped (responses 20k, prompt
 * previews 2k) and never contains secrets — prompts are git data and
 * instructions only, per `features.ts` guardrails.
 *
 * Elapsed time is NOT ticked here: consumers derive it from `startedAt`
 * with their own 1s `now` ticker (the WorkflowRunView pattern), so an
 * idle store costs nothing.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Tests construct isolated instances with an in-memory storage.
 */

import { loadVersioned, writeEnvelope } from "$lib/state/persist";
import type { StorageLike } from "$lib/state/persist";
import type { AiBackend, AiJobPhase, AiUsage } from "./types";
import type { AiFeatureTag } from "./registry";

// Re-exported for consumers that think of jobs as the source of these.
export type { AiJobPhase, AiUsage } from "./types";

/** Terminal status of a history record. */
export type AiJobStatus = "done" | "cancelled" | "error";

/** One in-flight AI run (reactive; mutate only via {@link AiJobTicket}). */
export interface AiJob {
  id: string;
  repoId: string;
  /** Tab name at start time (`null` when the tab is already gone). */
  repoName: string | null;
  /** Which feature (or `"chat"`) this run serves. */
  feature: AiFeatureTag;
  phase: AiJobPhase;
  /** Human phase detail ("reading staged diff"), `null` when none. */
  message: string | null;
  /** Epoch ms when the run began. */
  startedAt: number;
  /** Progressive output while streaming (`null` until the first delta). */
  streamedText: string | null;
  backend: AiBackend | null;
  model: string | null;
  usage: AiUsage | null;
  error: string | null;
}

/** One finished AI run (frozen summary; the Activity tab's rows). */
export interface AiJobRecord {
  id: string;
  repoId: string;
  repoName: string | null;
  feature: AiFeatureTag;
  status: AiJobStatus;
  startedAt: number;
  /** Wall-clock duration of the whole run. */
  elapsedMs: number;
  backend: AiBackend | null;
  model: string | null;
  usage: AiUsage | null;
  /** Safe failure message when `status === "error"`. */
  error: string | null;
  /** Final generated text, capped at {@link JOB_TEXT_CAP}. */
  text: string | null;
  /** Start of the prompt (what was sent), capped at {@link JOB_PROMPT_PREVIEW_CAP}. */
  promptPreview: string | null;
}

/** localStorage key for the capped history envelope. */
export const JOB_HISTORY_KEY = "mygitui.ai.activity";
/** History ring size (newest kept). */
export const JOB_HISTORY_CAP = 200;
/** Cap for a stored response body (characters). */
export const JOB_TEXT_CAP = 20_000;
/** Cap for a stored prompt preview (characters). */
export const JOB_PROMPT_PREVIEW_CAP = 2_000;
/** Cap for live streamed text on a job (characters; UI safety valve). */
export const JOB_STREAM_CAP = 500_000;

/** Injectable dependencies (tests). */
export interface AiJobStoreDeps {
  /** Storage for the history envelope; `null` disables persistence. */
  storage?: StorageLike | null;
  /** Job id minting (tests; default counter + random salt). */
  id?: () => string;
  /** Epoch-ms clock (tests). */
  now?: () => number;
}

/** Parse-and-drop validation for persisted history. */
export function parseJobHistory(data: unknown): AiJobRecord[] | null {
  if (!Array.isArray(data)) return null;
  const STATUSES: readonly string[] = ["done", "cancelled", "error"];
  const records: AiJobRecord[] = [];
  for (const raw of data) {
    if (typeof raw !== "object" || raw === null) continue;
    const r = raw as Record<string, unknown>;
    if (
      typeof r.id !== "string" ||
      typeof r.repoId !== "string" ||
      typeof r.feature !== "string" ||
      !STATUSES.includes(r.status as string) ||
      typeof r.startedAt !== "number" ||
      typeof r.elapsedMs !== "number"
    ) {
      continue;
    }
    records.push({
      id: r.id,
      repoId: r.repoId,
      repoName: typeof r.repoName === "string" ? r.repoName : null,
      feature: r.feature as AiJobRecord["feature"],
      status: r.status as AiJobStatus,
      startedAt: r.startedAt,
      elapsedMs: r.elapsedMs,
      backend: r.backend === "opencode" || r.backend === "openrouter" ? r.backend : null,
      model: typeof r.model === "string" ? r.model : null,
      usage: typeof r.usage === "object" && r.usage !== null ? (r.usage as AiUsage) : null,
      error: typeof r.error === "string" ? r.error : null,
      // Capped at parse too: a payload written before the write-side cap
      // (or by a buggy build) must not wedge storage on load.
      text: typeof r.text === "string" ? cap(r.text, JOB_TEXT_CAP) : null,
      promptPreview:
        typeof r.promptPreview === "string" ? cap(r.promptPreview, JOB_PROMPT_PREVIEW_CAP) : null,
    });
  }
  return records.slice(0, JOB_HISTORY_CAP);
}

function cap(text: string, max: number): string {
  return text.length <= max ? text : text.slice(0, max);
}

/** Handle a runner uses to advance one job through its lifecycle. */
export interface AiJobTicket {
  readonly id: string;
  /** Aborts when the user cancels (or `cancelAll` fires) — wire to fetch. */
  readonly signal: AbortSignal;
  /** Advances the phase (and optional human detail). */
  phase(phase: AiJobPhase, message?: string): void;
  /** Records which backend/model ended up serving (routing resolved). */
  serving(backend: AiBackend, model: string | null): void;
  /** Appends one streamed chunk to `streamedText`. */
  delta(text: string): void;
  /** Replaces the usage snapshot (may arrive once, at the end). */
  usage(usage: AiUsage): void;
  /** Sets the prompt preview (what the Activity detail will show). */
  promptPreview(text: string): void;
  /** Terminal: success. Moves the job to history. */
  done(outcome: {
    backend: AiBackend | null;
    model: string | null;
    elapsedMs: number;
    usage: AiUsage | null;
    text: string | null;
  }): void;
  /** Terminal: cancelled (user abort or provider-side interrupt). */
  cancelled(): void;
  /** Terminal: failed with a safe message. */
  failed(message: string): void;
}

/** Aggregated usage over a time window (Activity tab summary). */
export interface AiUsageSummary {
  runs: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  costUsd: number;
  /** Per-feature runs + token totals. */
  byFeature: Record<string, { runs: number; totalTokens: number }>;
}

/** The AI job store — one per app (owned by `AiStore`); construct for tests. */
export class AiJobStore {
  /** In-flight jobs by id (deep-reactive). */
  readonly active = $state<Record<string, AiJob>>({});
  /** Finished runs, newest first, capped at {@link JOB_HISTORY_CAP}. */
  history = $state<AiJobRecord[]>([]);

  readonly #storage: StorageLike | null;
  readonly #mintId: () => string;
  readonly #now: () => number;
  readonly #controllers = new Map<string, AbortController>();
  readonly #cancelRequested = new Set<string>();

  constructor(deps: AiJobStoreDeps = {}) {
    this.#storage = deps.storage === undefined ? defaultStorage() : deps.storage;
    this.#mintId =
      deps.id ??
      (() => {
        // Process-unique: counter for sortability, salt for safety.
        return `ai-${jobIdCounter.next().value!.toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
      });
    this.#now = deps.now ?? Date.now;
    const loaded = loadVersioned<AiJobRecord[]>({
      storage: this.#storage,
      key: JOB_HISTORY_KEY,
      parse: parseJobHistory,
    });
    if (loaded !== null) this.history = loaded;
  }

  // -- lifecycle ----------------------------------------------------------------

  /**
   * Starts tracking one run: adds an `queued` job to `active` and returns
   * the ticket the runner advances. The caller owns terminal transition —
   * exactly one of `done` / `cancelled` / `failed` per ticket.
   */
  begin(init: {
    repoId: string;
    repoName?: string | null;
    feature: AiFeatureTag;
  }): AiJobTicket {
    const id = this.#mintId();
    const job: AiJob = {
      id,
      repoId: init.repoId,
      repoName: init.repoName ?? null,
      feature: init.feature,
      phase: "queued",
      message: null,
      startedAt: this.#now(),
      streamedText: null,
      backend: null,
      model: null,
      usage: null,
      error: null,
    };
    this.active[id] = job;
    const controller = new AbortController();
    this.#controllers.set(id, controller);
    const store = this;
    const ticket: AiJobTicket = {
      id,
      signal: controller.signal,
      phase(phase, message) {
        const job = store.active[id];
        if (job === undefined) return;
        job.phase = phase;
        job.message = message ?? null;
      },
      serving(backend, model) {
        const job = store.active[id];
        if (job === undefined) return;
        job.backend = backend;
        job.model = model;
      },
      delta(text) {
        const job = store.active[id];
        if (job === undefined) return;
        // Cap the live buffer: a runaway model must not wedge the UI.
        const next = (job.streamedText ?? "") + text;
        job.streamedText = next.length > JOB_STREAM_CAP ? job.streamedText : next;
      },
      usage(usage) {
        const job = store.active[id];
        if (job === undefined) return;
        job.usage = usage;
      },
      promptPreview(text) {
        store.#prompts.set(id, cap(text, JOB_PROMPT_PREVIEW_CAP));
      },
      done(outcome) {
        store.#finish(id, "done", {
          backend: outcome.backend,
          model: outcome.model,
          elapsedMs: outcome.elapsedMs,
          usage: outcome.usage,
          error: null,
          text: outcome.text,
        });
      },
      cancelled() {
        const job = store.active[id];
        store.#finish(id, "cancelled", {
          backend: job?.backend ?? null,
          model: job?.model ?? null,
          elapsedMs: store.#now() - (job?.startedAt ?? store.#now()),
          usage: job?.usage ?? null,
          error: null,
          text: job?.streamedText ?? null,
        });
      },
      failed(message) {
        const job = store.active[id];
        store.#finish(id, "error", {
          backend: job?.backend ?? null,
          model: job?.model ?? null,
          elapsedMs: store.#now() - (job?.startedAt ?? store.#now()),
          usage: job?.usage ?? null,
          error: message,
          text: job?.streamedText ?? null,
        });
      },
    };
    return ticket;
  }

  /**
   * User-initiated cancel: aborts the ticket's signal and flags the job.
   * The runner observes the abort through its provider and reports the
   * terminal transition (`cancelled()`).
   */
  cancel(jobId: string): void {
    this.#cancelRequested.add(jobId);
    this.#controllers.get(jobId)?.abort();
  }

  /** Cancels every active job (master switch off, repo closed, …). */
  cancelAll(): void {
    for (const id of Object.keys(this.active)) this.cancel(id);
  }

  /** True when `cancel(jobId)` was called (runner decides the terminal). */
  cancelRequested(jobId: string): boolean {
    return this.#cancelRequested.has(jobId);
  }

  // -- queries --------------------------------------------------------------------

  /** Active jobs for one repo, oldest first. */
  activeForRepo(repoId: string): AiJob[] {
    return Object.values(this.active).filter((job) => job.repoId === repoId);
  }

  /**
   * The active job for `repoId` (optionally narrowed to one feature).
   * Feature runs are one-per-repo (the `AiStore.run` gate); chat carries
   * its own slot, so callers pass `feature` when they mean a specific one.
   */
  activeJob(repoId: string, feature?: AiFeatureTag): AiJob | null {
    const jobs = this.activeForRepo(repoId);
    if (feature === undefined) return jobs[0] ?? null;
    return jobs.find((job) => job.feature === feature) ?? null;
  }

  /** Clears persisted + in-memory history. */
  clearHistory(): void {
    this.history = [];
    writeEnvelope(this.#storage, JOB_HISTORY_KEY, this.history);
  }

  /** Aggregates usage over history entries that started after `cutoffMs`. */
  usageSince(cutoffMs: number): AiUsageSummary {
    const summary: AiUsageSummary = {
      runs: 0,
      inputTokens: 0,
      outputTokens: 0,
      totalTokens: 0,
      costUsd: 0,
      byFeature: {},
    };
    for (const record of this.history) {
      if (record.startedAt < cutoffMs) continue;
      summary.runs += 1;
      summary.inputTokens += record.usage?.inputTokens ?? 0;
      summary.outputTokens += record.usage?.outputTokens ?? 0;
      summary.totalTokens += record.usage?.totalTokens ?? 0;
      summary.costUsd += record.usage?.costUsd ?? 0;
      const bucket = (summary.byFeature[record.feature] ??= { runs: 0, totalTokens: 0 });
      bucket.runs += 1;
      bucket.totalTokens += record.usage?.totalTokens ?? 0;
    }
    return summary;
  }

  // -- internals --------------------------------------------------------------------

  /** Prompt previews keyed by id (moved into the record at finish). */
  readonly #prompts = new Map<string, string>();

  #finish(
    id: string,
    status: AiJobStatus,
    fields: {
      backend: AiBackend | null;
      model: string | null;
      elapsedMs: number;
      usage: AiUsage | null;
      error: string | null;
      text: string | null;
    },
  ): void {
    const job = this.active[id];
    if (job === undefined) return; // already finished — ignore late calls
    delete this.active[id];
    this.#controllers.delete(id);
    this.#cancelRequested.delete(id);
    const record: AiJobRecord = {
      id,
      repoId: job.repoId,
      repoName: job.repoName,
      feature: job.feature,
      status,
      startedAt: job.startedAt,
      elapsedMs: Math.max(0, fields.elapsedMs),
      backend: fields.backend,
      model: fields.model,
      usage: fields.usage,
      error: fields.error,
      // Capped on EVERY terminal path: cancelled/failed records carry the
      // live `streamedText` (up to JOB_STREAM_CAP), and 200 such records
      // would blow past the localStorage quota uncapped.
      text: fields.text === null ? null : cap(fields.text, JOB_TEXT_CAP),
      promptPreview: this.#prompts.get(id) ?? null,
    };
    this.#prompts.delete(id);
    this.history = [record, ...this.history].slice(0, JOB_HISTORY_CAP);
    writeEnvelope(this.#storage, JOB_HISTORY_KEY, this.history);
  }
}

/** Module-level id counter (stable across store instances in tests). */
const jobIdCounter: Generator<number, number, unknown> = (function* () {
  let n = 0;
  while (true) yield n++;
})();

/** Returns `localStorage` when available, else `null` (never throws). */
function defaultStorage(): StorageLike | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}
