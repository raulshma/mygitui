/**
 * Forge store (Svelte 5 runes) — M6 gh-assisted GitHub flow.
 *
 * Caches, per open repo: the `gh` status (process-global, fetched once per
 * refresh), the forge context (owner/repo/branch), the PR list and — lazily,
 * per opened PR — its CI checks (keyed `"<repoId>:<number>"`). Refresh
 * actions are the store's whole write surface; stale-response races are
 * dropped with per-target sequence counters.
 *
 * Lives in a `.svelte.ts` module because `$state` only compiles there.
 * Backend access goes exclusively through `$lib/ipc/client` (mocked with
 * `vi.mock` in tests); components build on the {@link forgeStore} singleton
 * or construct isolated instances.
 */

import {
  forgeContext,
  forgeStatus,
  prChecks,
  prCheckout,
  prClose,
  prComment,
  prList,
  prMerge,
  prReady,
  prReopen,
  prReview,
  workflowDispatch,
  workflowList,
  workflowRunCancel,
  workflowRunDetail,
  workflowRunLogs,
  workflowRunRerun,
  workflowRuns,
} from "$lib/ipc/client";
import type {
  CheckInfo,
  ForgeContext,
  ForgeStatus,
  PrInfo,
  PrMergeMethod,
  PrReviewAction,
  PrStateFilter,
  WorkflowInfo,
  WorkflowInput,
  WorkflowRun,
  WorkflowRunDetail,
  WorkflowRunFilters,
} from "$lib/ipc/client";
import type { RepoId } from "$lib/ipc/types";

/** Storage key for one PR's checks. */
export function checksKey(repoId: RepoId, number: number): string {
  return `${repoId}:${number}`;
}

/** Storage key for one workflow run (detail/busy lookups). */
export function runKey(repoId: RepoId, runId: number): string {
  return `${repoId}:${runId}`;
}

/** Storage key for one run log view (run-wide, per job, failed-only). */
export function runLogsKey(
  repoId: RepoId,
  runId: number,
  jobId: number | null,
  failedOnly: boolean,
): string {
  return `${repoId}:${runId}:${jobId ?? "all"}:${failedOnly ? "failed" : "full"}`;
}

export class ForgeStore {
  /** `gh` availability/auth (global, not per repo). */
  status = $state<ForgeStatus | null>(null);
  /** True while the status probe is in flight. */
  statusLoading = $state(false);

  /** Forge context per repo id. */
  contexts = $state<Record<string, ForgeContext>>({});
  /** Context errors per repo id (no origin / non-GitHub / detached HEAD). */
  contextErrors = $state<Record<string, string>>({});

  /** PR list per repo id. */
  prs = $state<Record<string, PrInfo[]>>({});
  /** PR-list refresh in flight per repo id. */
  prsLoading = $state<Record<string, boolean>>({});
  /** Selected PR-list state filter per repo id (default "open"). */
  prFilters = $state<Record<string, PrStateFilter>>({});

  /** Mutating PR action in flight per `"<repoId>:<number>"`. */
  busy = $state<Record<string, boolean>>({});

  /** CI checks per `"<repoId>:<number>"` (lazy, per opened PR). */
  checks = $state<Record<string, CheckInfo[]>>({});
  /** Checks refresh in flight per `"<repoId>:<number>"`. */
  checksLoading = $state<Record<string, boolean>>({});

  // -- GitHub Actions -------------------------------------------------------

  /** Recent runs per repo id (newest first). */
  runs = $state<Record<string, WorkflowRun[]>>({});
  /** Run-list refresh in flight per repo id. */
  runsLoading = $state<Record<string, boolean>>({});
  /** Run-list errors per repo id. */
  runsErrors = $state<Record<string, string>>({});
  /** Applied run-list filters per repo id. */
  runFilters = $state<Record<string, WorkflowRunFilters>>({});

  /** Workflow files per repo id (dispatch picker). */
  workflows = $state<Record<string, WorkflowInfo[]>>({});
  workflowsLoading = $state<Record<string, boolean>>({});
  workflowsErrors = $state<Record<string, string>>({});

  /** Run detail (jobs/steps) per `"<repoId>:<runId>"` (lazy). */
  details = $state<Record<string, WorkflowRunDetail>>({});
  detailsLoading = $state<Record<string, boolean>>({});
  detailsErrors = $state<Record<string, string>>({});

  /** Log text per {@link runLogsKey}. */
  logs = $state<Record<string, string>>({});
  logsLoading = $state<Record<string, boolean>>({});
  logsErrors = $state<Record<string, string>>({});

  /** Cancel/rerun in flight per `"<repoId>:<runId>"`. */
  runBusy = $state<Record<string, boolean>>({});
  /** Workflow dispatch in flight per repo id. */
  dispatchBusy = $state<Record<string, boolean>>({});

  #statusSeq = 0;
  #contextSeqs = new Map<string, number>();
  #prsSeqs = new Map<string, number>();
  #checksSeqs = new Map<string, number>();
  #runsSeqs = new Map<string, number>();
  #workflowsSeqs = new Map<string, number>();
  #detailsSeqs = new Map<string, number>();
  #logsSeqs = new Map<string, number>();

  /** Probes `gh` status (once per call; panels call this on mount). */
  async refreshStatus(): Promise<ForgeStatus | null> {
    this.statusLoading = true;
    const seq = ++this.#statusSeq;
    try {
      const status = await forgeStatus();
      if (seq === this.#statusSeq) this.status = status;
      return status;
    } catch (err) {
      if (seq === this.#statusSeq) {
        this.status = { available: false, version: "", authed: false };
        console.warn("[forge] status probe failed:", err);
      }
      return null;
    } finally {
      if (seq === this.#statusSeq) this.statusLoading = false;
    }
  }

  /** Refreshes (or clears) one repo's forge context. */
  async refreshContext(repoId: RepoId): Promise<ForgeContext | null> {
    const seq = (this.#contextSeqs.get(repoId) ?? 0) + 1;
    this.#contextSeqs.set(repoId, seq);
    try {
      const context = await forgeContext(repoId);
      if (seq === this.#contextSeqs.get(repoId)) {
        this.contexts = { ...this.contexts, [repoId]: context };
        const errors = { ...this.contextErrors };
        delete errors[repoId];
        this.contextErrors = errors;
      }
      return context;
    } catch (err) {
      if (seq === this.#contextSeqs.get(repoId)) {
        const message = err instanceof Error ? err.message : String(err);
        this.contextErrors = { ...this.contextErrors, [repoId]: message };
      }
      return null;
    }
  }

  /** Refreshes one repo's PR list (resolves [] on error — panel toasts). */
  async refreshPrs(
    repoId: RepoId,
    state: PrStateFilter = this.filterFor(repoId),
  ): Promise<PrInfo[]> {
    const seq = (this.#prsSeqs.get(repoId) ?? 0) + 1;
    this.#prsSeqs.set(repoId, seq);
    this.prFilters = { ...this.prFilters, [repoId]: state };
    this.prsLoading = { ...this.prsLoading, [repoId]: true };
    try {
      const list = await prList(repoId, state);
      if (seq === this.#prsSeqs.get(repoId)) {
        this.prs = { ...this.prs, [repoId]: list };
      }
      return list;
    } catch (err) {
      console.warn("[forge] pr list failed:", err);
      if (seq === this.#prsSeqs.get(repoId)) {
        this.prs = { ...this.prs, [repoId]: [] };
      }
      return [];
    } finally {
      if (seq === this.#prsSeqs.get(repoId)) {
        this.prsLoading = { ...this.prsLoading, [repoId]: false };
      }
    }
  }

  /** Lazily refreshes one PR's checks (keyed per repo+number). */
  async refreshChecks(repoId: RepoId, number: number): Promise<CheckInfo[]> {
    const key = checksKey(repoId, number);
    const seq = (this.#checksSeqs.get(key) ?? 0) + 1;
    this.#checksSeqs.set(key, seq);
    this.checksLoading = { ...this.checksLoading, [key]: true };
    try {
      const list = await prChecks(repoId, number);
      if (seq === this.#checksSeqs.get(key)) {
        this.checks = { ...this.checks, [key]: list };
      }
      return list;
    } catch (err) {
      console.warn("[forge] pr checks failed:", err);
      if (seq === this.#checksSeqs.get(key)) {
        this.checks = { ...this.checks, [key]: [] };
      }
      return [];
    } finally {
      if (seq === this.#checksSeqs.get(key)) {
        this.checksLoading = { ...this.checksLoading, [key]: false };
      }
    }
  }

  // -- mutating PR actions --------------------------------------------------

  /** Runs `fn` while marking one PR busy; always clears the flag. */
  async #runAction<T>(
    repoId: RepoId,
    number: number,
    fn: () => Promise<T>,
  ): Promise<T> {
    const key = checksKey(repoId, number);
    this.busy = { ...this.busy, [key]: true };
    try {
      return await fn();
    } finally {
      const busy = { ...this.busy };
      delete busy[key];
      this.busy = busy;
    }
  }

  /** Refreshes PR list + checks + branch context after a successful action. */
  async #afterAction(repoId: RepoId, number: number): Promise<void> {
    await Promise.all([
      this.refreshPrs(repoId),
      this.refreshChecks(repoId, number),
      this.refreshContext(repoId),
    ]);
  }

  async mergePr(
    repoId: RepoId,
    number: number,
    method: PrMergeMethod,
    deleteBranch: boolean,
    auto: boolean,
  ): Promise<string> {
    const out = await this.#runAction(repoId, number, () =>
      prMerge(repoId, number, method, deleteBranch, auto),
    );
    await this.#afterAction(repoId, number);
    return out;
  }

  async closePr(
    repoId: RepoId,
    number: number,
    comment: string | null = null,
  ): Promise<string> {
    const out = await this.#runAction(repoId, number, () =>
      prClose(repoId, number, comment),
    );
    await this.#afterAction(repoId, number);
    return out;
  }

  async reopenPr(
    repoId: RepoId,
    number: number,
    comment: string | null = null,
  ): Promise<string> {
    const out = await this.#runAction(repoId, number, () =>
      prReopen(repoId, number, comment),
    );
    await this.#afterAction(repoId, number);
    return out;
  }

  /** `ready = false` converts the PR back to draft. */
  async setDraft(repoId: RepoId, number: number, draft: boolean): Promise<string> {
    const out = await this.#runAction(repoId, number, () =>
      prReady(repoId, number, !draft),
    );
    await this.#afterAction(repoId, number);
    return out;
  }

  async checkoutPr(repoId: RepoId, number: number): Promise<string> {
    const out = await this.#runAction(repoId, number, () =>
      prCheckout(repoId, number),
    );
    await this.#afterAction(repoId, number);
    return out;
  }

  async commentPr(
    repoId: RepoId,
    number: number,
    body: string,
  ): Promise<string> {
    const out = await this.#runAction(repoId, number, () =>
      prComment(repoId, number, body),
    );
    await this.#afterAction(repoId, number);
    return out;
  }

  async reviewPr(
    repoId: RepoId,
    number: number,
    action: PrReviewAction,
    body: string,
  ): Promise<string> {
    const out = await this.#runAction(repoId, number, () =>
      prReview(repoId, number, action, body),
    );
    await this.#afterAction(repoId, number);
    return out;
  }

  // -- GitHub Actions -------------------------------------------------------

  /** Refreshes one repo's run list (resolves [] on error; panel shows it). */
  async refreshRuns(
    repoId: RepoId,
    filters: WorkflowRunFilters = this.runFiltersFor(repoId),
    silent = false,
  ): Promise<WorkflowRun[]> {
    const seq = (this.#runsSeqs.get(repoId) ?? 0) + 1;
    this.#runsSeqs.set(repoId, seq);
    this.runFilters = { ...this.runFilters, [repoId]: filters };
    if (!silent) this.runsLoading = { ...this.runsLoading, [repoId]: true };
    try {
      const list = await workflowRuns(repoId, filters);
      if (seq === this.#runsSeqs.get(repoId)) {
        this.runs = { ...this.runs, [repoId]: list };
        const errors = { ...this.runsErrors };
        delete errors[repoId];
        this.runsErrors = errors;
      }
      return list;
    } catch (err) {
      console.warn("[forge] workflow runs failed:", err);
      if (seq === this.#runsSeqs.get(repoId)) {
        this.runsErrors = {
          ...this.runsErrors,
          [repoId]: err instanceof Error ? err.message : String(err),
        };
      }
      return [];
    } finally {
      if (seq === this.#runsSeqs.get(repoId)) {
        this.runsLoading = { ...this.runsLoading, [repoId]: false };
      }
    }
  }

  /** Refreshes the repo's workflow files (dispatch picker). */
  async refreshWorkflows(repoId: RepoId): Promise<WorkflowInfo[]> {
    const seq = (this.#workflowsSeqs.get(repoId) ?? 0) + 1;
    this.#workflowsSeqs.set(repoId, seq);
    this.workflowsLoading = { ...this.workflowsLoading, [repoId]: true };
    try {
      const list = await workflowList(repoId);
      if (seq === this.#workflowsSeqs.get(repoId)) {
        this.workflows = { ...this.workflows, [repoId]: list };
        const errors = { ...this.workflowsErrors };
        delete errors[repoId];
        this.workflowsErrors = errors;
      }
      return list;
    } catch (err) {
      console.warn("[forge] workflow list failed:", err);
      if (seq === this.#workflowsSeqs.get(repoId)) {
        this.workflowsErrors = {
          ...this.workflowsErrors,
          [repoId]: err instanceof Error ? err.message : String(err),
        };
      }
      return [];
    } finally {
      if (seq === this.#workflowsSeqs.get(repoId)) {
        this.workflowsLoading = { ...this.workflowsLoading, [repoId]: false };
      }
    }
  }

  /** Keeps the run list row in sync with a fresher detail payload. */
  #mergeRun(repoId: RepoId, run: WorkflowRun): void {
    const list = this.runs[repoId];
    if (!list) return;
    const index = list.findIndex(
      (entry) => entry.database_id === run.database_id,
    );
    if (index === -1) return;
    const next = list.slice();
    next[index] = run;
    this.runs = { ...this.runs, [repoId]: next };
  }

  /** Lazily loads one run's jobs/steps (keyed per repo+run). */
  async loadRunDetail(
    repoId: RepoId,
    runId: number,
  ): Promise<WorkflowRunDetail | null> {
    const key = runKey(repoId, runId);
    const seq = (this.#detailsSeqs.get(key) ?? 0) + 1;
    this.#detailsSeqs.set(key, seq);
    this.detailsLoading = { ...this.detailsLoading, [key]: true };
    try {
      const detail = await workflowRunDetail(repoId, runId);
      if (seq === this.#detailsSeqs.get(key)) {
        this.details = { ...this.details, [key]: detail };
        this.#mergeRun(repoId, detail.run);
        const errors = { ...this.detailsErrors };
        delete errors[key];
        this.detailsErrors = errors;
      }
      return detail;
    } catch (err) {
      console.warn("[forge] workflow run detail failed:", err);
      if (seq === this.#detailsSeqs.get(key)) {
        this.detailsErrors = {
          ...this.detailsErrors,
          [key]: err instanceof Error ? err.message : String(err),
        };
      }
      return null;
    } finally {
      if (seq === this.#detailsSeqs.get(key)) {
        this.detailsLoading = { ...this.detailsLoading, [key]: false };
      }
    }
  }

  /** Loads a run/job log (cached per view; `jobId` null = whole run). */
  async loadRunLogs(
    repoId: RepoId,
    runId: number,
    jobId: number | null = null,
    failedOnly = false,
  ): Promise<string | null> {
    const key = runLogsKey(repoId, runId, jobId, failedOnly);
    const seq = (this.#logsSeqs.get(key) ?? 0) + 1;
    this.#logsSeqs.set(key, seq);
    this.logsLoading = { ...this.logsLoading, [key]: true };
    try {
      const text = await workflowRunLogs(repoId, runId, jobId, failedOnly);
      if (seq === this.#logsSeqs.get(key)) {
        this.logs = { ...this.logs, [key]: text };
        const errors = { ...this.logsErrors };
        delete errors[key];
        this.logsErrors = errors;
      }
      return text;
    } catch (err) {
      console.warn("[forge] workflow run logs failed:", err);
      if (seq === this.#logsSeqs.get(key)) {
        this.logsErrors = {
          ...this.logsErrors,
          [key]: err instanceof Error ? err.message : String(err),
        };
      }
      return null;
    } finally {
      if (seq === this.#logsSeqs.get(key)) {
        this.logsLoading = { ...this.logsLoading, [key]: false };
      }
    }
  }

  /** Drops cached logs of one run (after cancel/rerun, logs go stale). */
  #clearRunLogs(repoId: RepoId, runId: number): void {
    const prefix = `${repoId}:${runId}:`;
    const logs = { ...this.logs };
    const errors = { ...this.logsErrors };
    let changed = false;
    for (const key of Object.keys(logs)) {
      if (key.startsWith(prefix)) {
        delete logs[key];
        changed = true;
      }
    }
    for (const key of Object.keys(errors)) {
      if (key.startsWith(prefix)) {
        delete errors[key];
        changed = true;
      }
    }
    if (changed) {
      this.logs = logs;
      this.logsErrors = errors;
    }
  }

  /** Cancel a queued/running run, then refresh list + detail. */
  async cancelRun(repoId: RepoId, runId: number): Promise<string> {
    const key = runKey(repoId, runId);
    this.runBusy = { ...this.runBusy, [key]: true };
    try {
      const out = await workflowRunCancel(repoId, runId);
      this.#clearRunLogs(repoId, runId);
      await Promise.all([
        this.refreshRuns(repoId),
        this.loadRunDetail(repoId, runId),
      ]);
      return out;
    } finally {
      const busy = { ...this.runBusy };
      delete busy[key];
      this.runBusy = busy;
    }
  }

  /** Rerun all jobs (or only failed ones), then refresh list + detail. */
  async rerunRun(
    repoId: RepoId,
    runId: number,
    failedOnly = false,
  ): Promise<string> {
    const key = runKey(repoId, runId);
    this.runBusy = { ...this.runBusy, [key]: true };
    try {
      const out = await workflowRunRerun(repoId, runId, failedOnly);
      this.#clearRunLogs(repoId, runId);
      await Promise.all([
        this.refreshRuns(repoId),
        this.loadRunDetail(repoId, runId),
      ]);
      return out;
    } finally {
      const busy = { ...this.runBusy };
      delete busy[key];
      this.runBusy = busy;
    }
  }

  /** Trigger a workflow_dispatch run on `gitRef` with raw string inputs. */
  async dispatchWorkflow(
    repoId: RepoId,
    workflow: string,
    gitRef: string,
    inputs: WorkflowInput[] = [],
  ): Promise<string> {
    this.dispatchBusy = { ...this.dispatchBusy, [repoId]: true };
    try {
      const out = await workflowDispatch(repoId, workflow, gitRef, inputs);
      await this.refreshRuns(repoId);
      return out;
    } finally {
      const busy = { ...this.dispatchBusy };
      delete busy[repoId];
      this.dispatchBusy = busy;
    }
  }

  /** First activation of the Actions tab: runs + workflow files. */
  async ensureActions(repoId: RepoId): Promise<void> {
    await Promise.all([
      this.refreshRuns(repoId),
      this.refreshWorkflows(repoId),
    ]);
  }

  /**
   * Panel mount bundle: status (global, skipped when already known),
   * context and PR list for `repoId` — in parallel.
   */
  async ensure(repoId: RepoId): Promise<void> {
    const jobs: Promise<unknown>[] = [this.refreshContext(repoId), this.refreshPrs(repoId)];
    if (!this.status) jobs.push(this.refreshStatus());
    await Promise.all(jobs);
  }

  /** Everything a PR creation should refresh: context + list. */
  async afterCreate(repoId: RepoId): Promise<void> {
    await Promise.all([this.refreshContext(repoId), this.refreshPrs(repoId)]);
  }

  // -- read helpers ---------------------------------------------------------

  contextFor(repoId: RepoId): ForgeContext | null {
    return this.contexts[repoId] ?? null;
  }

  contextErrorFor(repoId: RepoId): string | null {
    return this.contextErrors[repoId] ?? null;
  }

  prsFor(repoId: RepoId): PrInfo[] {
    return this.prs[repoId] ?? [];
  }

  prsLoadingFor(repoId: RepoId): boolean {
    return this.prsLoading[repoId] ?? false;
  }

  filterFor(repoId: RepoId): PrStateFilter {
    return this.prFilters[repoId] ?? "open";
  }

  isBusy(repoId: RepoId, number: number): boolean {
    return this.busy[checksKey(repoId, number)] ?? false;
  }

  checksFor(repoId: RepoId, number: number): CheckInfo[] | null {
    return this.checks[checksKey(repoId, number)] ?? null;
  }

  checksLoadingFor(repoId: RepoId, number: number): boolean {
    return this.checksLoading[checksKey(repoId, number)] ?? false;
  }

  // -- GitHub Actions read helpers -----------------------------------------

  runsFor(repoId: RepoId): WorkflowRun[] {
    return this.runs[repoId] ?? [];
  }

  runsLoadingFor(repoId: RepoId): boolean {
    return this.runsLoading[repoId] ?? false;
  }

  runsErrorFor(repoId: RepoId): string | null {
    return this.runsErrors[repoId] ?? null;
  }

  runFiltersFor(repoId: RepoId): WorkflowRunFilters {
    return this.runFilters[repoId] ?? {};
  }

  workflowsFor(repoId: RepoId): WorkflowInfo[] {
    return this.workflows[repoId] ?? [];
  }

  workflowsLoadingFor(repoId: RepoId): boolean {
    return this.workflowsLoading[repoId] ?? false;
  }

  workflowsErrorFor(repoId: RepoId): string | null {
    return this.workflowsErrors[repoId] ?? null;
  }

  detailFor(repoId: RepoId, runId: number): WorkflowRunDetail | null {
    return this.details[runKey(repoId, runId)] ?? null;
  }

  detailLoadingFor(repoId: RepoId, runId: number): boolean {
    return this.detailsLoading[runKey(repoId, runId)] ?? false;
  }

  detailErrorFor(repoId: RepoId, runId: number): string | null {
    return this.detailsErrors[runKey(repoId, runId)] ?? null;
  }

  logsFor(
    repoId: RepoId,
    runId: number,
    jobId: number | null,
    failedOnly: boolean,
  ): string | null {
    return this.logs[runLogsKey(repoId, runId, jobId, failedOnly)] ?? null;
  }

  logsLoadingFor(
    repoId: RepoId,
    runId: number,
    jobId: number | null,
    failedOnly: boolean,
  ): boolean {
    return (
      this.logsLoading[runLogsKey(repoId, runId, jobId, failedOnly)] ?? false
    );
  }

  logsErrorFor(
    repoId: RepoId,
    runId: number,
    jobId: number | null,
    failedOnly: boolean,
  ): string | null {
    return (
      this.logsErrors[runLogsKey(repoId, runId, jobId, failedOnly)] ?? null
    );
  }

  isRunBusy(repoId: RepoId, runId: number): boolean {
    return this.runBusy[runKey(repoId, runId)] ?? false;
  }

  isDispatchBusy(repoId: RepoId): boolean {
    return this.dispatchBusy[repoId] ?? false;
  }
}

/** The application-wide forge store. */
export const forgeStore = new ForgeStore();

/**
 * Opens a URL in the system browser via the opener plugin. No-op (resolves
 * `false`) outside a Tauri webview or on failure — never throws.
 */
export async function openExternal(url: string): Promise<boolean> {
  if (!url) return false;
  try {
    const { isTauri } = await import("$lib/entry/dragdrop");
    if (!isTauri()) return false;
    const opener = await import("@tauri-apps/plugin-opener");
    await opener.openUrl(url);
    return true;
  } catch (err) {
    console.warn("[forge] open url failed:", err);
    return false;
  }
}
