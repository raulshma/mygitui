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
  forgePrFiles,
  forgePrReviewThreads,
  forgeSnapshot,
  forgeStatus,
  isForgeFailure,
  issueClose,
  issueCreate,
  issueList,
  issueReopen,
  issueView,
  notificationMarkRead,
  notificationsList,
  notificationsMarkAllRead,
  prChecks,
  prCheckout,
  prClose,
  prComment,
  prEdit,
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
  ForgeFailureKind,
  ForgeIssue,
  ForgeIssueDetail,
  ForgeNotification,
  ForgePrFile,
  ForgeReviewThread,
  ForgeSnapshot,
  ForgeStatus,
  IssueStateFilter,
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
import { tabStore } from "$lib/stores/tabs.svelte";

/** Storage key for one PR's checks. */
export function checksKey(repoId: RepoId, number: number): string {
  return `${repoId}:${number}`;
}

/** Storage key for one issue's detail (body + comments). */
export function issueDetailKey(repoId: RepoId, number: number): string {
  return `${repoId}:${number}`;
}

/** Storage key for one workflow run (detail/busy lookups). */
export function runKey(repoId: RepoId, runId: number): string {
  return `${repoId}:${runId}`;
}

/** True when every snapshot content hash matches (unchanged content). */
export function snapshotHashesEqual(
  a: ForgeSnapshot["hashes"],
  b: ForgeSnapshot["hashes"],
): boolean {
  return (
    a.pr === b.pr &&
    a.head_checks === b.head_checks &&
    a.pr_head_checks === b.pr_head_checks &&
    a.run === b.run
  );
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

/** Cache key for the local machine's `gh` status probe. */
const STATUS_KEY = "gh-status";

/** Cache key of one device's `gh` status probe (remote repos run gh there). */
function deviceStatusKey(deviceId: string): string {
  return `device:${deviceId}`;
}

/** What a failed status probe degrades to (the panel shows "unavailable"). */
const GH_UNAVAILABLE: ForgeStatus = {
  available: false,
  version: "",
  authed: false,
};

export class ForgeStore {
  /**
   * `gh` status probe per machine: the local machine under
   * {@link STATUS_KEY}, each device under `device:<id>` (a device repo's
   * GitHub features run the device's own gh).
   */
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

  // -- Issues (Phase 6 batch A; refreshed on demand, never auto-polled) -----

  /** Issue list per repo id. */
  #issues = new AsyncMap<ForgeIssue[]>();
  /** Selected issue-list state filter per repo id (default "open"). */
  issueFilters = $state<Record<string, IssueStateFilter>>({});
  /** Issue detail (body + comments) per `"<repoId>:<number>"` (lazy). */
  #issueDetails = new AsyncMap<ForgeIssueDetail>();
  /** Mutating issue action in flight per `"<repoId>:<number>"`. */
  #issueBusy = $state<Record<string, boolean>>({});

  // -- PR-detail depth (E2, plan §9.4; read-only, loaded on open) -----------

  /** Changed files (with patches) of one PR per `"<repoId>:<number>"` (lazy). */
  #prFiles = new AsyncMap<ForgePrFile[]>();
  /** Review threads (top-level + replies) of one PR per `"<repoId>:<number>"` (lazy). */
  #threads = new AsyncMap<ForgeReviewThread[]>();

  // -- Notifications (Phase 6 batch A; refreshed on open / manual) ----------

  /** Unread notification threads per repo id. */
  #notifications = new AsyncMap<ForgeNotification[]>();

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

  // -- Snapshot (E1, plan §9.3) ----------------------------------------------
  //
  // The footer chips' batched GitHub state per open repo, refreshed by the
  // poll scheduler's `forge:snapshot:<repoId>` lanes. Same AsyncMap machine
  // as every other group (seq guards + stale-on-error); content hashes skip
  // re-renders of unchanged snapshots (the previous object is restored when
  // every hash matches, so chip identities stay stable).

  /** Batched GitHub snapshot per repo id. */
  #snapshots = new AsyncMap<ForgeSnapshot>();
  /** Wall-clock of the last successful snapshot fetch per repo id ("as of Xm ago"). */
  #snapshotFetchedAt = new Map<RepoId, number>();
  /** Last snapshot failure's ForgeError kind per repo id ("" = none). */
  #snapshotKinds = $state<Record<string, string>>({});

  // -- snapshot (E1) ---------------------------------------------------------

  /**
   * Refreshes one repo's snapshot (the poll scheduler's lane work and the
   * popovers' manual refresh). `remote` selects the per-repo override remote
   * (`null`/absent = `origin`). Silent when a previous entry exists — the
   * chips must not flash "loading" on every 60 s tick. Resolves the snapshot,
   * or `null` when the fetch failed (failure kind via
   * {@link snapshotFailureKindFor}; the failure also REJECTS out of
   * `refreshSnapshotStrict`, which the scheduler lanes use to trigger
   * backoff).
   */
  async refreshSnapshot(
    repoId: RepoId,
    opts: { remote?: string | null; silent?: boolean } = {},
  ): Promise<ForgeSnapshot | null> {
    const remote = opts.remote ?? null;
    const silent = opts.silent ?? this.#snapshots.get(repoId).state !== "idle";
    const previous = this.#readyData(this.#snapshots, repoId);
    const next = await this.#snapshots.run(
      repoId,
      () => forgeSnapshot(repoId, remote),
      {
        silent,
        onError: (err) => {
          // The rejection is the structured ForgeError (raw wire object or
          // the wrapped ForgeIpcError) — the kind drives the chips' named
          // degraded states (§9.1).
          if (isForgeFailure(err)) {
            this.#snapshotKinds = { ...this.#snapshotKinds, [repoId]: err.kind };
          }
          console.warn("[forge] snapshot failed:", err);
        },
      },
    );
    if (next !== null) {
      delete this.#snapshotKinds[repoId];
      this.#snapshotKinds = { ...this.#snapshotKinds };
      this.#snapshotFetchedAt.set(repoId, Date.now());
      if (
        previous !== null &&
        snapshotHashesEqual(previous.hashes, next.hashes)
      ) {
        // Unchanged content: restore the previous object (identical hashes →
        // identical data), so derived chips keep their object identity and
        // consumers skip re-rendering.
        this.#snapshots.set(repoId, previous);
      }
      return this.#readyData(this.#snapshots, repoId);
    }
    return null;
  }

  /**
   * The scheduler-lane variant: resolves like {@link refreshSnapshot} but
   * REJECTS when the fetch failed so the poll scheduler's exponential
   * backoff engages (403 ⇒ backoff + visible rate-limited state, §9.3).
   */
  async refreshSnapshotStrict(
    repoId: RepoId,
    opts: { remote?: string | null } = {},
  ): Promise<void> {
    const next = await this.refreshSnapshot(repoId, {
      ...opts,
      silent: true,
    });
    if (next === null && this.#snapshots.get(repoId).state === "error") {
      throw new Error(this.snapshotErrorFor(repoId) ?? "snapshot failed");
    }
  }

  /** The repo's ready snapshot (`null` while loading/failed without data). */
  snapshotFor(repoId: RepoId): ForgeSnapshot | null {
    return this.#readyData(this.#snapshots, repoId);
  }

  /**
   * The snapshot, or the one a failed refresh left visible (stale-on-error —
   * the chips keep showing the last-known state; the failure kind rides
   * separately via {@link snapshotFailureKindFor}).
   */
  snapshotStaleFor(repoId: RepoId): ForgeSnapshot | null {
    const entry = this.#snapshots.get(repoId);
    if (entry.state === "ready") return entry.data;
    return entry.state === "error" ? entry.previous : null;
  }

  /** The snapshot cache entry state (`idle | loading | ready | error`). */
  snapshotStateFor(repoId: RepoId): "idle" | "loading" | "ready" | "error" {
    return this.#snapshots.get(repoId).state;
  }

  /** The snapshot fetch failure message (when the latest fetch failed). */
  snapshotErrorFor(repoId: RepoId): string | null {
    return this.#mapError(this.#snapshots, repoId);
  }

  /** Last snapshot failure's `ForgeError` kind (`null` = last fetch ok). */
  snapshotFailureKindFor(repoId: RepoId): ForgeFailureKind | null {
    const kind = this.#snapshotKinds[repoId];
    return kind ? (kind as ForgeFailureKind) : null;
  }

  /** Wall-clock (ms) of the last successful snapshot fetch (`null` = never). */
  snapshotFetchedAtFor(repoId: RepoId): number | null {
    return this.#snapshotFetchedAt.get(repoId) ?? null;
  }

  /** Drops the snapshot cache of one repo (tab close). */
  dropSnapshot(repoId: RepoId): void {
    this.#snapshots.reset(repoId);
    this.#snapshotFetchedAt.delete(repoId);
    delete this.#snapshotKinds[repoId];
    this.#snapshotKinds = { ...this.#snapshotKinds };
  }

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

  /**
   * Probes `gh` status (once per call; panels call this on mount). Without
   * `repoId` — and for local repos — this machine is probed; a repo that
   * lives on a device probes that device, so the panel and chips speak for
   * the machine the repo actually runs on.
   */
  async refreshStatus(repoId?: RepoId): Promise<ForgeStatus | null> {
    const tab =
      repoId !== undefined
        ? (tabStore.tabs.find((tab) => tab.id === repoId) ?? null)
        : null;
    const key = tab?.deviceId ? deviceStatusKey(tab.deviceId) : STATUS_KEY;
    return this.#status.run(key, async () => {
      try {
        return await forgeStatus(tab?.deviceId ? repoId : undefined);
      } catch (err) {
        console.warn("[forge] status probe failed:", err);
        return GH_UNAVAILABLE;
      }
    });
  }

  /** The status entry key for one repo (its device, or the local machine). */
  #statusKeyFor(repoId?: RepoId): string {
    if (repoId === undefined) return STATUS_KEY;
    const tab = tabStore.tabs.find((tab) => tab.id === repoId) ?? null;
    return tab?.deviceId ? deviceStatusKey(tab.deviceId) : STATUS_KEY;
  }

  /**
   * `gh` availability/auth for one repo's machine (local status when the
   * repo is local), or null before the first probe lands.
   */
  statusFor(repoId?: RepoId): ForgeStatus | null {
    return this.#readyData(this.#status, this.#statusKeyFor(repoId));
  }

  /** True while the repo's machine's status probe is in flight. */
  statusLoadingFor(repoId?: RepoId): boolean {
    return this.#mapLoading(this.#status, this.#statusKeyFor(repoId));
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

  /** Updates a PR's title and/or body, then refreshes the list. */
  async editPr(
    repoId: RepoId,
    number: number,
    title: string,
    body: string,
  ): Promise<string> {
    const out = await this.#runAction(repoId, number, () =>
      prEdit(repoId, number, title || null, body || null),
    );
    await this.refreshPrs(repoId);
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

  // -- Issues ---------------------------------------------------------------

  /** Refreshes one repo's issue list (resolves [] on error; error visible). */
  async refreshIssues(
    repoId: RepoId,
    state: IssueStateFilter = this.issueFilterFor(repoId),
  ): Promise<ForgeIssue[]> {
    this.issueFilters = { ...this.issueFilters, [repoId]: state };
    const list = await this.#issues.run(repoId, () => issueList(repoId, state), {
      onError: (err) => console.warn("[forge] issue list failed:", err),
    });
    return list ?? [];
  }

  /** TTL-guarded issue-list read (first Issues-tab activation). */
  async ensureIssues(repoId: RepoId): Promise<ForgeIssue[]> {
    const list = await this.#issues.ensure(
      repoId,
      () => issueList(repoId, this.issueFilterFor(repoId)),
      ForgeStore.PANEL_ENSURE_TTL_MS,
      { onError: (err) => console.warn("[forge] issue list failed:", err) },
    );
    return list ?? [];
  }

  /** Runs `fn` while marking one issue busy; always clears the flag. */
  async #runIssueAction<T>(
    repoId: RepoId,
    number: number,
    fn: () => Promise<T>,
  ): Promise<T> {
    const key = issueDetailKey(repoId, number);
    this.#issueBusy = { ...this.#issueBusy, [key]: true };
    try {
      return await fn();
    } finally {
      const busy = { ...this.#issueBusy };
      delete busy[key];
      this.#issueBusy = busy;
    }
  }

  /** Lazily loads one issue's body + comments (keyed per repo+number). */
  async loadIssueDetail(
    repoId: RepoId,
    number: number,
  ): Promise<ForgeIssueDetail | null> {
    return this.#issueDetails.run(
      issueDetailKey(repoId, number),
      () => issueView(repoId, number),
      { onError: (err) => console.warn("[forge] issue view failed:", err) },
    );
  }

  /** Lazily loads one PR's changed files (keyed per repo+number). */
  async loadPrFiles(repoId: RepoId, number: number): Promise<ForgePrFile[]> {
    const files = await this.#prFiles.run(
      checksKey(repoId, number),
      () => forgePrFiles(repoId, number),
      { onError: (err) => console.warn("[forge] pr files failed:", err) },
    );
    return files ?? [];
  }

  /** Lazily loads one PR's review threads (keyed per repo+number). */
  async loadReviewThreads(
    repoId: RepoId,
    number: number,
  ): Promise<ForgeReviewThread[]> {
    const threads = await this.#threads.run(
      checksKey(repoId, number),
      () => forgePrReviewThreads(repoId, number),
      { onError: (err) => console.warn("[forge] review threads failed:", err) },
    );
    return threads ?? [];
  }

  /** Creates an issue, then refreshes the list. */
  async createIssue(repoId: RepoId, title: string, body: string): Promise<void> {
    await issueCreate(repoId, title, body);
    await this.refreshIssues(repoId);
  }

  /** Closes an issue (optional comment), then refreshes list + detail. */
  async closeIssue(
    repoId: RepoId,
    number: number,
    comment: string | null = null,
  ): Promise<string> {
    const out = await this.#runIssueAction(repoId, number, () =>
      issueClose(repoId, number, comment),
    );
    this.#issueDetails.reset(issueDetailKey(repoId, number));
    await this.refreshIssues(repoId);
    return out;
  }

  /** Reopens a closed issue (optional comment), then refreshes list + detail. */
  async reopenIssue(
    repoId: RepoId,
    number: number,
    comment: string | null = null,
  ): Promise<string> {
    const out = await this.#runIssueAction(repoId, number, () =>
      issueReopen(repoId, number, comment),
    );
    this.#issueDetails.reset(issueDetailKey(repoId, number));
    await this.refreshIssues(repoId);
    return out;
  }

  // -- Notifications --------------------------------------------------------

  /**
   * Refreshes one repo's notification threads (resolves [] on error;
   * error visible). Called when the tab opens and from its refresh button;
   * never auto-polled.
   */
  async refreshNotifications(repoId: RepoId): Promise<ForgeNotification[]> {
    const list = await this.#notifications.run(repoId, () => notificationsList(repoId), {
      onError: (err) => console.warn("[forge] notifications failed:", err),
    });
    return list ?? [];
  }

  /** TTL-guarded notifications read (first tab activation). */
  async ensureNotifications(repoId: RepoId): Promise<ForgeNotification[]> {
    const list = await this.#notifications.ensure(
      repoId,
      () => notificationsList(repoId),
      ForgeStore.PANEL_ENSURE_TTL_MS,
      { onError: (err) => console.warn("[forge] notifications failed:", err) },
    );
    return list ?? [];
  }

  /** Marks one thread read and drops it from the cached list. */
  async markNotificationRead(repoId: RepoId, id: number): Promise<void> {
    await notificationMarkRead(repoId, id);
    const entry = this.#notifications.get(repoId);
    if (entry.state !== "ready") return;
    this.#notifications.set(
      repoId,
      entry.data.filter((item) => item.id !== id),
    );
  }

  /** Marks every thread read and clears the cached list. */
  async markAllNotificationsRead(repoId: RepoId): Promise<void> {
    await notificationsMarkAllRead(repoId);
    this.#notifications.set(repoId, []);
  }

  /** First activation of the Actions tab: runs + workflow files. Ensure
   * semantics with a short TTL: re-activations within the window resolve
   * from cache instead of respawning two `gh` subprocesses. */
  static readonly ACTIONS_ENSURE_TTL_MS = 5_000;
  async ensureActions(repoId: RepoId): Promise<void> {
    await Promise.all([
      this.ensureRuns(repoId),
      this.#workflows.ensure(
        repoId,
        () => workflowList(repoId),
        ForgeStore.ACTIONS_ENSURE_TTL_MS,
        { onError: (err) => console.warn("[forge] workflow list failed:", err) },
      ),
    ]);
  }

  /** TTL-guarded runs read (see {@link ensureActions}). */
  async ensureRuns(repoId: RepoId): Promise<WorkflowRun[]> {
    const list = await this.#runs.ensure(
      repoId,
      () => workflowRuns(repoId, this.runFiltersFor(repoId)),
      ForgeStore.ACTIONS_ENSURE_TTL_MS,
      { onError: (err) => console.warn("[forge] workflow runs failed:", err) },
    );
    return list ?? [];
  }

  /**
   * Panel mount bundle: status (the repo's machine — device repos probe
   * their device; skipped when already known), context and PR list for
   * `repoId` — in parallel. Ensure semantics with a short TTL so every tab
   * switch with the panel mounted doesn't respawn the `forge_context` +
   * `pr_list` subprocesses for data that just landed; mutations and
   * explicit refreshes still bypass it via the refresh* calls.
   */
  static readonly PANEL_ENSURE_TTL_MS = 3_000;
  async ensure(repoId: RepoId): Promise<void> {
    const jobs: Promise<unknown>[] = [this.ensureContext(repoId), this.ensurePrs(repoId)];
    if (this.statusFor(repoId) === null) jobs.push(this.refreshStatus(repoId));
    await Promise.all(jobs);
  }

  /** TTL-guarded context read (see {@link ensure}). */
  async ensureContext(repoId: RepoId): Promise<ForgeContext | null> {
    return this.#contexts.ensure(repoId, () => forgeContext(repoId), ForgeStore.PANEL_ENSURE_TTL_MS);
  }

  /** TTL-guarded PR list read (see {@link ensure}). */
  async ensurePrs(repoId: RepoId): Promise<PrInfo[]> {
    const list = await this.#prs.ensure(
      repoId,
      () => prList(repoId, this.filterFor(repoId)),
      ForgeStore.PANEL_ENSURE_TTL_MS,
      { onError: (err) => console.warn("[forge] pr list failed:", err) },
    );
    return list ?? [];
  }

  /** Everything a PR creation should refresh: context + list. */
  async afterCreate(repoId: RepoId): Promise<void> {
    await Promise.all([this.refreshContext(repoId), this.refreshPrs(repoId)]);
  }

  // -- read helpers ---------------------------------------------------------

  /**
   * `gh` availability/auth on THIS machine, or null before the first probe
   * lands. Repo-scoped consumers use {@link statusFor} (device repos have
   * their own probe).
   */
  get status(): ForgeStatus | null {
    return this.#readyData(this.#status, STATUS_KEY);
  }

  /** True while the local status probe is in flight. */
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

  // -- Issues read helpers --------------------------------------------------

  /** Issue list, or the list a failed refresh left visible. */
  issuesFor(repoId: RepoId): ForgeIssue[] {
    return this.#staleData(this.#issues, repoId) ?? [];
  }

  issuesLoadingFor(repoId: RepoId): boolean {
    return this.#mapLoading(this.#issues, repoId);
  }

  issuesErrorFor(repoId: RepoId): string | null {
    return this.#mapError(this.#issues, repoId);
  }

  issueFilterFor(repoId: RepoId): IssueStateFilter {
    return this.issueFilters[repoId] ?? "open";
  }

  isIssueBusy(repoId: RepoId, number: number): boolean {
    return this.#issueBusy[issueDetailKey(repoId, number)] ?? false;
  }

  /** Issue detail (body + comments) for `number`, or null while loading. */
  issueDetailFor(repoId: RepoId, number: number): ForgeIssueDetail | null {
    return this.#readyData(this.#issueDetails, issueDetailKey(repoId, number));
  }

  issueDetailLoadingFor(repoId: RepoId, number: number): boolean {
    return this.#mapLoading(this.#issueDetails, issueDetailKey(repoId, number));
  }

  /** The issue-detail fetch failure, if the latest fetch failed. */
  issueDetailErrorFor(repoId: RepoId, number: number): string | null {
    return this.#mapError(this.#issueDetails, issueDetailKey(repoId, number));
  }

  // -- PR-detail depth read helpers (E2) -------------------------------------

  /** One PR's changed files, or the files a failed refresh left visible. */
  prFilesFor(repoId: RepoId, number: number): ForgePrFile[] | null {
    return this.#staleData(this.#prFiles, checksKey(repoId, number));
  }

  prFilesLoadingFor(repoId: RepoId, number: number): boolean {
    return this.#mapLoading(this.#prFiles, checksKey(repoId, number));
  }

  /** The PR-files fetch failure, if the latest fetch failed. */
  prFilesErrorFor(repoId: RepoId, number: number): string | null {
    return this.#mapError(this.#prFiles, checksKey(repoId, number));
  }

  /** One PR's review threads, or the threads a failed refresh left visible. */
  reviewThreadsFor(repoId: RepoId, number: number): ForgeReviewThread[] | null {
    return this.#staleData(this.#threads, checksKey(repoId, number));
  }

  reviewThreadsLoadingFor(repoId: RepoId, number: number): boolean {
    return this.#mapLoading(this.#threads, checksKey(repoId, number));
  }

  /** The review-threads fetch failure, if the latest fetch failed. */
  reviewThreadsErrorFor(repoId: RepoId, number: number): string | null {
    return this.#mapError(this.#threads, checksKey(repoId, number));
  }

  // -- Notifications read helpers ------------------------------------------

  /** Notification threads, or the list a failed refresh left visible. */
  notificationsFor(repoId: RepoId): ForgeNotification[] {
    return this.#staleData(this.#notifications, repoId) ?? [];
  }

  notificationsLoadingFor(repoId: RepoId): boolean {
    return this.#mapLoading(this.#notifications, repoId);
  }

  notificationsErrorFor(repoId: RepoId): string | null {
    return this.#mapError(this.#notifications, repoId);
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
