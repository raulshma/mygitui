/**
 * Forge store (Svelte 5 runes) — M6 gh-assisted GitHub flow.
 *
 * Caches, per open repo: the `gh` status (process-global, fetched once per
 * refresh), the forge context (owner/repo/branch), the PR list and — lazily,
 * per opened PR — its CI checks (keyed `"<repoId>:<number>"`). Refresh
 * actions are the store's whole write surface; each target group is one
 * {@link AsyncMap} (idle/loading/ready/error per key, stale responses
 * dropped), so `loading && error` is unrepresentable and a failed PR/check
 * fetch surfaces instead of silently blanking the list.
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
import { AsyncMap } from "$lib/state/async.svelte";

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

/** Cache key for the process-global `gh` status probe. */
const STATUS_KEY = "gh-status";

/** What a failed status probe degrades to (the panel shows "unavailable"). */
const GH_UNAVAILABLE: ForgeStatus = {
  available: false,
  version: "",
  authed: false,
};

export class ForgeStore {
  /** `gh` status probe (global, not per repo): one entry under {@link STATUS_KEY}. */
  #status = new AsyncMap<ForgeStatus>();

  /** Forge context per repo id. */
  #contexts = new AsyncMap<ForgeContext>();

  /** PR list per repo id. */
  #prs = new AsyncMap<PrInfo[]>();
  /** Selected PR-list state filter per repo id (default "open"). */
  prFilters = $state<Record<string, PrStateFilter>>({});

  /** Mutating PR action in flight per `"<repoId>:<number>"`. */
  #busy = $state<Record<string, boolean>>({});

  /** CI checks per `"<repoId>:<number>"` (lazy, per opened PR). */
  #checks = new AsyncMap<CheckInfo[]>();

  // -- GitHub Actions -------------------------------------------------------

  /** Recent runs per repo id (newest first). */
  #runs = new AsyncMap<WorkflowRun[]>();
  /** Applied run-list filters per repo id. */
  runFilters = $state<Record<string, WorkflowRunFilters>>({});

  /** Workflow files per repo id (dispatch picker). */
  #workflows = new AsyncMap<WorkflowInfo[]>();

  /** Run detail (jobs/steps) per `"<repoId>:<runId>"` (lazy). */
  #details = new AsyncMap<WorkflowRunDetail>();

  /** Log text per {@link runLogsKey}. */
  #logs = new AsyncMap<string>();

  /** Cancel/rerun in flight per `"<repoId>:<runId>"`. */
  #runBusy = $state<Record<string, boolean>>({});
  /** Workflow dispatch in flight per repo id. */
  #dispatchBusy = $state<Record<string, boolean>>({});

  // -- shared AsyncMap read helpers -----------------------------------------

  /** Ready data for `key`, or null (absent / loading / error without data). */
  #readyData<T>(map: AsyncMap<T>, key: string): T | null {
    const entry = map.get(key);
    return entry.state === "ready" ? entry.data : null;
  }

  /** Ready data, or the data a failed refresh left visible. */
  #staleData<T>(map: AsyncMap<T>, key: string): T | null {
    const entry = map.get(key);
    if (entry.state === "ready") return entry.data;
    return entry.state === "error" ? entry.previous : null;
  }

  /** The failure message for `key`, or null when its latest fetch succeeded. */
  #mapError<T>(map: AsyncMap<T>, key: string): string | null {
    const entry = map.get(key);
    return entry.state === "error" ? entry.message : null;
  }

  /** True while `key`'s latest fetch is in flight. */
  #mapLoading<T>(map: AsyncMap<T>, key: string): boolean {
    return map.get(key).state === "loading";
  }

  /** Probes `gh` status (once per call; panels call this on mount). */
  async refreshStatus(): Promise<ForgeStatus | null> {
    return this.#status.run(STATUS_KEY, async () => {
      try {
        return await forgeStatus();
      } catch (err) {
        console.warn("[forge] status probe failed:", err);
        return GH_UNAVAILABLE;
      }
    });
  }

  /** Refreshes (or clears) one repo's forge context. */
  async refreshContext(repoId: RepoId): Promise<ForgeContext | null> {
    return this.#contexts.run(repoId, () => forgeContext(repoId));
  }

  /** Refreshes one repo's PR list (resolves [] on error; error is visible). */
  async refreshPrs(
    repoId: RepoId,
    state: PrStateFilter = this.filterFor(repoId),
  ): Promise<PrInfo[]> {
    this.prFilters = { ...this.prFilters, [repoId]: state };
    const list = await this.#prs.run(repoId, () => prList(repoId, state), {
      onError: (err) => console.warn("[forge] pr list failed:", err),
    });
    return list ?? [];
  }

  /** Lazily refreshes one PR's checks (keyed per repo+number). */
  async refreshChecks(repoId: RepoId, number: number): Promise<CheckInfo[]> {
    const key = checksKey(repoId, number);
    const list = await this.#checks.run(key, () => prChecks(repoId, number), {
      onError: (err) => console.warn("[forge] pr checks failed:", err),
    });
    return list ?? [];
  }

  // -- mutating PR actions --------------------------------------------------

  /** Runs `fn` while marking one PR busy; always clears the flag. */
  async #runAction<T>(
    repoId: RepoId,
    number: number,
    fn: () => Promise<T>,
  ): Promise<T> {
    const key = checksKey(repoId, number);
    this.#busy = { ...this.#busy, [key]: true };
    try {
      return await fn();
    } finally {
      const busy = { ...this.#busy };
      delete busy[key];
      this.#busy = busy;
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

  /** Refreshes one repo's run list (resolves [] on error; error is visible). */
  async refreshRuns(
    repoId: RepoId,
    filters: WorkflowRunFilters = this.runFiltersFor(repoId),
    silent = false,
  ): Promise<WorkflowRun[]> {
    this.runFilters = { ...this.runFilters, [repoId]: filters };
    const list = await this.#runs.run(
      repoId,
      () => workflowRuns(repoId, filters),
      {
        silent,
        onError: (err) => console.warn("[forge] workflow runs failed:", err),
      },
    );
    return list ?? [];
  }

  /** Refreshes the repo's workflow files (dispatch picker). */
  async refreshWorkflows(repoId: RepoId): Promise<WorkflowInfo[]> {
    const list = await this.#workflows.run(repoId, () => workflowList(repoId), {
      onError: (err) => console.warn("[forge] workflow list failed:", err),
    });
    return list ?? [];
  }

  /** Keeps the run list row in sync with a fresher detail payload. */
  #mergeRun(repoId: RepoId, run: WorkflowRun): void {
    const entry = this.#runs.get(repoId);
    if (entry.state !== "ready") return;
    const index = entry.data.findIndex(
      (candidate) => candidate.database_id === run.database_id,
    );
    if (index === -1) return;
    const next = entry.data.slice();
    next[index] = run;
    this.#runs.set(repoId, next);
  }

  /** Lazily loads one run's jobs/steps (keyed per repo+run). */
  async loadRunDetail(
    repoId: RepoId,
    runId: number,
  ): Promise<WorkflowRunDetail | null> {
    const key = runKey(repoId, runId);
    const detail = await this.#details.run(
      key,
      () => workflowRunDetail(repoId, runId),
      {
        onError: (err) =>
          console.warn("[forge] workflow run detail failed:", err),
      },
    );
    if (detail !== null) this.#mergeRun(repoId, detail.run);
    return detail;
  }

  /** Loads a run/job log (cached per view; `jobId` null = whole run). */
  async loadRunLogs(
    repoId: RepoId,
    runId: number,
    jobId: number | null = null,
    failedOnly = false,
  ): Promise<string | null> {
    const key = runLogsKey(repoId, runId, jobId, failedOnly);
    return this.#logs.run(key, () => workflowRunLogs(repoId, runId, jobId, failedOnly), {
      onError: (err) => console.warn("[forge] workflow run logs failed:", err),
    });
  }

  /** Drops cached logs of one run (after cancel/rerun, logs go stale). */
  #clearRunLogs(repoId: RepoId, runId: number): void {
    this.#logs.resetPrefix(`${repoId}:${runId}:`);
  }

  /** Cancel a queued/running run, then refresh list + detail. */
  async cancelRun(repoId: RepoId, runId: number): Promise<string> {
    const key = runKey(repoId, runId);
    this.#runBusy = { ...this.#runBusy, [key]: true };
    try {
      const out = await workflowRunCancel(repoId, runId);
      this.#clearRunLogs(repoId, runId);
      await Promise.all([
        this.refreshRuns(repoId),
        this.loadRunDetail(repoId, runId),
      ]);
      return out;
    } finally {
      const busy = { ...this.#runBusy };
      delete busy[key];
      this.#runBusy = busy;
    }
  }

  /** Rerun all jobs (or only failed ones), then refresh list + detail. */
  async rerunRun(
    repoId: RepoId,
    runId: number,
    failedOnly = false,
  ): Promise<string> {
    const key = runKey(repoId, runId);
    this.#runBusy = { ...this.#runBusy, [key]: true };
    try {
      const out = await workflowRunRerun(repoId, runId, failedOnly);
      this.#clearRunLogs(repoId, runId);
      await Promise.all([
        this.refreshRuns(repoId),
        this.loadRunDetail(repoId, runId),
      ]);
      return out;
    } finally {
      const busy = { ...this.#runBusy };
      delete busy[key];
      this.#runBusy = busy;
    }
  }

  /** Trigger a workflow_dispatch run on `gitRef` with raw string inputs. */
  async dispatchWorkflow(
    repoId: RepoId,
    workflow: string,
    gitRef: string,
    inputs: WorkflowInput[] = [],
  ): Promise<string> {
    this.#dispatchBusy = { ...this.#dispatchBusy, [repoId]: true };
    try {
      const out = await workflowDispatch(repoId, workflow, gitRef, inputs);
      await this.refreshRuns(repoId);
      return out;
    } finally {
      const busy = { ...this.#dispatchBusy };
      delete busy[repoId];
      this.#dispatchBusy = busy;
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
    if (this.status === null) jobs.push(this.refreshStatus());
    await Promise.all(jobs);
  }

  /** Everything a PR creation should refresh: context + list. */
  async afterCreate(repoId: RepoId): Promise<void> {
    await Promise.all([this.refreshContext(repoId), this.refreshPrs(repoId)]);
  }

  // -- read helpers ---------------------------------------------------------

  /** `gh` availability/auth (global), or null before the first probe lands. */
  get status(): ForgeStatus | null {
    return this.#readyData(this.#status, STATUS_KEY);
  }

  /** True while the status probe is in flight. */
  get statusLoading(): boolean {
    return this.#mapLoading(this.#status, STATUS_KEY);
  }

  contextFor(repoId: RepoId): ForgeContext | null {
    return this.#readyData(this.#contexts, repoId);
  }

  contextErrorFor(repoId: RepoId): string | null {
    return this.#mapError(this.#contexts, repoId);
  }

  /** PR list, or the list a failed refresh left visible. */
  prsFor(repoId: RepoId): PrInfo[] {
    return this.#staleData(this.#prs, repoId) ?? [];
  }

  prsLoadingFor(repoId: RepoId): boolean {
    return this.#mapLoading(this.#prs, repoId);
  }

  /** The PR-list fetch failure, if the latest fetch failed. */
  prsErrorFor(repoId: RepoId): string | null {
    return this.#mapError(this.#prs, repoId);
  }

  filterFor(repoId: RepoId): PrStateFilter {
    return this.prFilters[repoId] ?? "open";
  }

  isBusy(repoId: RepoId, number: number): boolean {
    return this.#busy[checksKey(repoId, number)] ?? false;
  }

  /** CI checks, or the checks a failed refresh left visible. */
  checksFor(repoId: RepoId, number: number): CheckInfo[] | null {
    return this.#staleData(this.#checks, checksKey(repoId, number));
  }

  checksLoadingFor(repoId: RepoId, number: number): boolean {
    return this.#mapLoading(this.#checks, checksKey(repoId, number));
  }

  /** The checks fetch failure, if the latest fetch failed. */
  checksErrorFor(repoId: RepoId, number: number): string | null {
    return this.#mapError(this.#checks, checksKey(repoId, number));
  }

  // -- GitHub Actions read helpers -----------------------------------------

  /** Run list, or the list a failed refresh left visible. */
  runsFor(repoId: RepoId): WorkflowRun[] {
    return this.#staleData(this.#runs, repoId) ?? [];
  }

  runsLoadingFor(repoId: RepoId): boolean {
    return this.#mapLoading(this.#runs, repoId);
  }

  runsErrorFor(repoId: RepoId): string | null {
    return this.#mapError(this.#runs, repoId);
  }

  runFiltersFor(repoId: RepoId): WorkflowRunFilters {
    return this.runFilters[repoId] ?? {};
  }

  /** Workflow files, or the list a failed refresh left visible. */
  workflowsFor(repoId: RepoId): WorkflowInfo[] {
    return this.#staleData(this.#workflows, repoId) ?? [];
  }

  workflowsLoadingFor(repoId: RepoId): boolean {
    return this.#mapLoading(this.#workflows, repoId);
  }

  workflowsErrorFor(repoId: RepoId): string | null {
    return this.#mapError(this.#workflows, repoId);
  }

  detailFor(repoId: RepoId, runId: number): WorkflowRunDetail | null {
    return this.#readyData(this.#details, runKey(repoId, runId));
  }

  detailLoadingFor(repoId: RepoId, runId: number): boolean {
    return this.#mapLoading(this.#details, runKey(repoId, runId));
  }

  detailErrorFor(repoId: RepoId, runId: number): string | null {
    return this.#mapError(this.#details, runKey(repoId, runId));
  }

  logsFor(
    repoId: RepoId,
    runId: number,
    jobId: number | null,
    failedOnly: boolean,
  ): string | null {
    return this.#readyData(this.#logs, runLogsKey(repoId, runId, jobId, failedOnly));
  }

  logsLoadingFor(
    repoId: RepoId,
    runId: number,
    jobId: number | null,
    failedOnly: boolean,
  ): boolean {
    return this.#mapLoading(
      this.#logs,
      runLogsKey(repoId, runId, jobId, failedOnly),
    );
  }

  logsErrorFor(
    repoId: RepoId,
    runId: number,
    jobId: number | null,
    failedOnly: boolean,
  ): string | null {
    return this.#mapError(
      this.#logs,
      runLogsKey(repoId, runId, jobId, failedOnly),
    );
  }

  isRunBusy(repoId: RepoId, runId: number): boolean {
    return this.#runBusy[runKey(repoId, runId)] ?? false;
  }

  isDispatchBusy(repoId: RepoId): boolean {
    return this.#dispatchBusy[repoId] ?? false;
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
