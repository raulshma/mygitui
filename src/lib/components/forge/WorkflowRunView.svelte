<script lang="ts">
  /**
   * GitHub Actions run detail — rendered in the main editor (or a popout).
   *
   * Header: status/elapsed/trigger metadata + actions (cancel, rerun, open on
   * GitHub, copy sha, popout, close). Body: a jobs pane (status, steps,
   * durations, failed-only filter) and the selected scope's log rendered by
   * {@link WorkflowLogViewer}.
   *
   * While the run is queued/running the view polls the detail and the active
   * log every five seconds; pausing stops both. Polling never hides stale
   * content: the log viewer keeps the previous payload and shows a refresh
   * hint. The store merges refreshed `detail.run` rows back into the run
   * list, so the Actions sidebar stays in sync.
   */
  import { untrack } from "svelte";
  import type { WorkflowJob, WorkflowStep } from "$lib/ipc/types";
  import { forgeStore, openExternal } from "$lib/stores/forge.svelte";
  import { toast } from "$lib/toast";
  import WorkflowLogViewer from "./WorkflowLogViewer.svelte";
  import {
    activityDurationLabel,
    prAgeLabel,
    runCanRerun,
    runElapsedLabel,
    runHasFailures,
    runIsActive,
    runStateClass,
    runStateIcon,
    runStateKind,
    runStateLabel,
    stepStateClass,
    stepStateLabel,
    workflowEventLabel,
  } from "./forgeModel";

  let {
    repoId,
    runId,
    onClose = undefined,
    onPopout = undefined,
  }: {
    repoId: string;
    runId: number;
    /** Closes the run view (main editor only). */
    onClose?: () => void;
    /** Opens this run in its own window. */
    onPopout?: () => void;
  } = $props();

  /** `null` = combined log of every job. */
  let selectedJobId = $state<number | null>(null);
  /** Selected or targeted step (`job\0step`) to highlight/scroll in the log viewer. */
  let activeStepKey = $state<string | null>(null);
  /** Keep only failed jobs and failed steps in the log request. */
  let failedOnly = $state(false);
  /** Live polling (active runs only). */
  let autoRefresh = $state(true);
  /** Ticks elapsed labels once per second while the run is active. */
  let now = $state(Date.now());
  /** Manual refresh only: background polling must not flicker the toolbar. */
  let manualRefresh = $state(false);

  function jumpToStep(jobName: string, stepName: string): void {
    activeStepKey = `${jobName}\u0000${stepName}`;
  }

  const detail = $derived(forgeStore.detailFor(repoId, runId));
  const listRun = $derived(
    forgeStore.runsFor(repoId).find((entry) => entry.database_id === runId) ?? null,
  );
  const run = $derived(detail?.run ?? listRun);
  const jobs = $derived(detail?.jobs ?? []);
  const selectedJob = $derived(
    selectedJobId === null
      ? null
      : (jobs.find((job) => job.database_id === selectedJobId) ?? null),
  );
  const logText = $derived(
    forgeStore.logsFor(repoId, runId, selectedJobId, failedOnly),
  );
  const logLoading = $derived(
    forgeStore.logsLoadingFor(repoId, runId, selectedJobId, failedOnly),
  );
  const logError = $derived(
    forgeStore.logsErrorFor(repoId, runId, selectedJobId, failedOnly),
  );
  const detailLoading = $derived(forgeStore.detailLoadingFor(repoId, runId));
  const detailError = $derived(forgeStore.detailErrorFor(repoId, runId));
  const busy = $derived(forgeStore.isRunBusy(repoId, runId));

  const active = $derived(run ? runIsActive(run) : true);
  const kind = $derived(run ? runStateKind(run) : "unknown");
  const statusText = $derived(run ? runStateLabel(run) : "Loading");
  const duration = $derived(run ? runElapsedLabel(run, now) : null);
  const updatedAgo = $derived(run ? prAgeLabel(run.updated_at, now) : null);
  const refreshing = $derived(manualRefresh);
  const live = $derived(active && autoRefresh);

  function jobFailed(job: WorkflowJob): boolean {
    return (
      job.conclusion === "failure" ||
      job.conclusion === "timed_out" ||
      job.conclusion === "cancelled" ||
      job.conclusion === "action_required" ||
      job.conclusion === "startup_failure"
    );
  }

  function stepFailed(step: WorkflowStep): boolean {
    return (
      step.conclusion === "failure" ||
      step.conclusion === "timed_out" ||
      step.conclusion === "cancelled" ||
      step.conclusion === "startup_failure"
    );
  }

  /** Metadata handed to the log viewer must match the requested scope. */
  function scopeJob(job: WorkflowJob): WorkflowJob {
    return failedOnly ? { ...job, steps: job.steps.filter(stepFailed) } : job;
  }

  const failedJobs = $derived(jobs.filter(jobFailed));
  const visibleJobs = $derived(
    failedOnly ? jobs.filter(jobFailed) : jobs,
  );
  /** The log viewer only receives metadata for the scope it is showing. */
  const logJobs = $derived(
    selectedJob
      ? [scopeJob(selectedJob)]
      : failedOnly
        ? visibleJobs.map(scopeJob)
        : jobs,
  );
  const jobsDone = $derived(
    jobs.filter((job) => job.status === "completed").length,
  );
  const jobProgressPct = $derived(
    jobs.length === 0 ? 0 : Math.round((jobsDone / jobs.length) * 100),
  );
  const emptyLogText = $derived(
    active
      ? "Logs stream here once GitHub publishes them for the running jobs."
      : failedOnly
        ? "No failed steps were recorded for this selection."
        : "No log output for this selection.",
  );

  // A different run is a different view: drop the previous scope and resume
  // live updates even if the user had paused the last run.
  let viewKey = "";
  $effect(() => {
    const key = `${repoId}\u0000${runId}`;
    if (key === viewKey) return;
    viewKey = key;
    selectedJobId = null;
    activeStepKey = null;
    failedOnly = false;
    autoRefresh = true;
    manualRefresh = false;
  });

  // Initial load + selection changes. `untrack` keeps the fetch from making
  // the effect depend on the loading state it writes.
  $effect(() => {
    const targetRepo = repoId;
    const id = runId;
    untrack(() => void forgeStore.loadRunDetail(targetRepo, id));
  });

  $effect(() => {
    const targetRepo = repoId;
    const id = runId;
    const jobId = selectedJobId;
    const onlyFailed = failedOnly;
    untrack(() => void forgeStore.loadRunLogs(targetRepo, id, jobId, onlyFailed));
  });

  // Mild polling while the run is active. Hidden windows skip a beat; the
  // next visible tick catches up. Reading the scope state keeps the interval's
  // closure fresh when the user switches jobs mid-run.
  $effect(() => {
    if (!autoRefresh) return;
    const targetRepo = repoId;
    const id = runId;
    const jobId = selectedJobId;
    const onlyFailed = failedOnly;
    if (!active) return;
    const timer = setInterval(() => {
      if (
        typeof document !== "undefined" &&
        document.visibilityState === "hidden"
      ) {
        return;
      }
      void forgeStore.loadRunDetail(targetRepo, id);
      void forgeStore.loadRunLogs(targetRepo, id, jobId, onlyFailed);
    }, 3_500);
    return () => clearInterval(timer);
  });

  $effect(() => {
    if (!active) return;
    const timer = setInterval(() => (now = Date.now()), 1_000);
    return () => clearInterval(timer);
  });

  // Final fetch when the run finishes (the poll may stop before the last
  // archive chunk becomes available).
  let wasActive = false;
  $effect(() => {
    const isActive = active;
    if (wasActive && !isActive) {
      void forgeStore.loadRunDetail(repoId, runId);
      void forgeStore.loadRunLogs(repoId, runId, selectedJobId, failedOnly);
    }
    wasActive = isActive;
  });

  // Keep the selection valid when the failed-only filter hides the job.
  $effect(() => {
    if (selectedJobId === null) return;
    const match = jobs.find((job) => job.database_id === selectedJobId);
    if (!match || (failedOnly && !jobFailed(match))) {
      selectedJobId = null;
      activeStepKey = null;
    }
  });

  async function refreshNow(): Promise<void> {
    if (manualRefresh) return;
    manualRefresh = true;
    try {
      await Promise.all([
        forgeStore.loadRunDetail(repoId, runId),
        forgeStore.loadRunLogs(repoId, runId, selectedJobId, failedOnly),
      ]);
    } finally {
      manualRefresh = false;
    }
  }

  function selectJob(id: number | null): void {
    selectedJobId = id;
    activeStepKey = null;
    if (id === null) return;
    const job = jobs.find((entry) => entry.database_id === id);
    if (job) activeStepKey = stepToTarget(job);
  }

  /** The most useful step to reveal when a job is selected. */
  function stepToTarget(job: WorkflowJob): string | null {
    const steps = job.steps;
    const running = steps.find((step) => step.status === "in_progress");
    if (running) return `${job.name}\u0000${running.name}`;
    const queued = steps.find((step) => step.status === "queued");
    if (queued) return `${job.name}\u0000${queued.name}`;
    const failed = steps.find(stepFailed);
    if (failed) return `${job.name}\u0000${failed.name}`;
    return null;
  }

  function toggleFailedOnly(): void {
    failedOnly = !failedOnly;
    activeStepKey = null;
  }

  function toggleAutoRefresh(): void {
    autoRefresh = !autoRefresh;
  }

  async function openOnGitHub(): Promise<void> {
    if (!run) return;
    const ok = await openExternal(run.url);
    if (!ok) toast(`Could not open ${run.url}`, { kind: "error" });
  }

  async function copySha(): Promise<void> {
    if (!run?.head_sha) return;
    try {
      await navigator.clipboard.writeText(run.head_sha);
      toast("Commit SHA copied to the clipboard", { kind: "success" });
    } catch {
      toast("Clipboard is not available", { kind: "error" });
    }
  }

  function errorText(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  async function cancelRun(): Promise<void> {
    if (!run) return;
    try {
      await forgeStore.cancelRun(repoId, runId);
      toast(`Cancelled run #${run.number}`, { kind: "success" });
    } catch (err) {
      toast(errorText(err), { kind: "error" });
    }
  }

  async function rerunRun(failed: boolean): Promise<void> {
    if (!run) return;
    try {
      await forgeStore.rerunRun(repoId, runId, failed);
      toast(
        failed ? `Rerunning failed jobs of #${run.number}` : `Rerunning #${run.number}`,
        { kind: "success" },
      );
    } catch (err) {
      toast(errorText(err), { kind: "error" });
    }
  }
</script>

<div class="run-view">
  <header class="run-head">
    <div class="head-top">
      <span class="crumbs">
        <span class="crumb-muted">Actions</span>
        <span class="crumb-sep" aria-hidden="true">/</span>
        <span class="crumb-current" title={run?.name ?? "Workflow run"}>
          {run?.name ?? "Workflow run"}
        </span>
        {#if run}<span class="run-number">#{run.number}</span>{/if}
      </span>
      <span class="spacer"></span>
      {#if onPopout}
        <button
          class="tb"
          type="button"
          title="Open this run in its own window"
          onclick={onPopout}
        >
          Pop out
        </button>
      {/if}
      {#if onClose}
        <button
          class="tb close"
          type="button"
          aria-label="Close run details"
          title="Close run details"
          onclick={onClose}
        >
          ×
        </button>
      {/if}
    </div>

    <div class="title-row">
      <span class="status-chip {runStateClass(kind)}">
        <span class="status-icon" aria-hidden="true">{runStateIcon(kind)}</span>
        {statusText}
      </span>
      {#if active}
        <span class="live-badge" class:paused={!autoRefresh}>
          <span class="live-dot" aria-hidden="true"></span>
          {autoRefresh ? "Live" : "Paused"}
        </span>
      {/if}
      <h2 class="run-title">{run?.display_title || run?.name || `Run ${runId}`}</h2>
    </div>

    <div class="meta" aria-label="Run metadata">
      {#if run}
        <span class="meta-item">
          <span class="meta-label">Trigger</span>
          {workflowEventLabel(run.event)}
        </span>
        {#if run.head_branch}
          <span class="meta-item">
            <span class="meta-label">Branch</span>
            <code class="branch" title={run.head_branch}>{run.head_branch}</code>
          </span>
        {/if}
        {#if run.head_sha}
          <button
            class="sha"
            type="button"
            title="Copy the full commit SHA"
            onclick={() => void copySha()}
          >
            {run.head_sha.slice(0, 7)}
          </button>
        {/if}
        {#if run.attempt > 1}
          <span class="meta-item attempt">attempt {run.attempt}</span>
        {/if}
        {#if duration}
          <span class="meta-item" title="Elapsed time">
            <span class="meta-label">{active ? "Running" : "Took"}</span>
            {duration}
          </span>
        {/if}
        {#if updatedAgo}
          <span class="meta-item">
            <span class="meta-label">Updated</span>
            {updatedAgo} ago
          </span>
        {/if}
      {:else if detailLoading}
        <span class="meta-item">Loading run metadata…</span>
      {/if}
    </div>

    <div class="actions" aria-label="Run actions">
      {#if active}
        <button
          class="tb"
          type="button"
          title={autoRefresh ? "Pause live updates" : "Resume live updates"}
          onclick={toggleAutoRefresh}
        >
          {autoRefresh ? "Pause live" : "Resume live"}
        </button>
      {/if}
      <button
        class="tb"
        type="button"
        disabled={refreshing}
        onclick={() => void refreshNow()}
      >
        {refreshing ? "Refreshing…" : "Refresh"}
      </button>
      {#if run}
        {#if active}
          <button
            class="tb danger"
            type="button"
            title="Cancel this run"
            disabled={busy}
            onclick={() => void cancelRun()}
          >
            {busy ? "Cancelling…" : "Cancel"}
          </button>
        {/if}
        {#if runCanRerun(run)}
          <button
            class="tb"
            type="button"
            title="Rerun all jobs"
            disabled={busy}
            onclick={() => void rerunRun(false)}
          >
            {busy ? "Working…" : "Rerun"}
          </button>
          {#if runHasFailures(run)}
            <button
              class="tb"
              type="button"
              title="Rerun only the failed jobs"
              disabled={busy}
              onclick={() => void rerunRun(true)}
            >
              Rerun failed
            </button>
          {/if}
        {/if}
        <button class="tb" type="button" onclick={() => void openOnGitHub()}>
          Open on GitHub
        </button>
      {/if}
    </div>

    {#if detailError && detail}
      <div class="inline-error" role="alert">
        Could not refresh run details: {detailError}
      </div>
    {/if}
  </header>

  <div class="run-body">
    <aside class="jobs-pane" aria-label="Workflow jobs">
      <div class="jobs-head">
        <h3 class="pane-title">
          Jobs
          {#if jobs.length > 0}<span class="pane-count">{jobs.length}</span>{/if}
        </h3>
        {#if active && jobs.length > 0}
          <span
            class="job-progress"
            title={`${jobsDone} of ${jobs.length} jobs finished`}
          >
            <span class="job-progress-track" aria-hidden="true">
              <span class="job-progress-fill" style="width: {jobProgressPct}%"></span>
            </span>
            <span class="job-progress-label">{jobsDone}/{jobs.length}</span>
          </span>
        {/if}
        {#if failedJobs.length > 0 || failedOnly}
          <button
            class="tb"
            class:active={failedOnly}
            type="button"
            aria-pressed={failedOnly}
            title="Show failed jobs and only failed steps in the log"
            onclick={toggleFailedOnly}
          >
            Failed only
            <span class="filter-count">{failedJobs.length}</span>
          </button>
        {/if}
      </div>

      {#if detailLoading && !detail}
        <div class="pane-state" aria-live="polite">
          <span class="state-spinner" aria-hidden="true"></span>
          <p>Loading jobs…</p>
        </div>
      {:else if detailError && !detail}
        <div class="pane-state" role="alert">
          <p>Could not load this run.</p>
          <p class="state-hint">{detailError}</p>
          <button class="tb" type="button" onclick={() => void refreshNow()}>
            Try again
          </button>
        </div>
      {:else if !detail && !listRun}
        <div class="pane-state" aria-live="polite">
          <p>This run is no longer available.</p>
          <p class="state-hint">
            It may have been removed, or the run list has not loaded yet.
          </p>
        </div>
      {:else}
        <div class="jobs-list" role="group" aria-label="Jobs in this run">
          <button
            class="job-row all"
            class:selected={selectedJobId === null}
            type="button"
            aria-pressed={selectedJobId === null}
            title="Show the combined log of every job"
            onclick={() => selectJob(null)}
          >
            <span class="job-bullet all-bullet" aria-hidden="true">▤</span>
            <span class="job-main">
              <span class="job-name">All jobs</span>
              <span class="job-sub">
                {failedOnly ? "Failed steps across the run" : "Combined log"}
              </span>
            </span>
          </button>

          {#if visibleJobs.length === 0}
            <p class="pane-note">
              {failedOnly
                ? "No failed jobs in this run."
                : "No jobs reported for this run."}
            </p>
          {/if}

          {#each visibleJobs as job (job.database_id)}
            {@const jobKind = runStateKind({
              status: job.status,
              conclusion: job.conclusion,
            })}
            {@const jobDuration = activityDurationLabel(
              job.started_at,
              job.completed_at,
              job.status,
              now,
            )}
            <button
              class="job-row"
              class:selected={selectedJobId === job.database_id}
              type="button"
              aria-pressed={selectedJobId === job.database_id}
              title={`Show the log of ${job.name}`}
              onclick={() => selectJob(job.database_id)}
            >
              <span
                class="job-bullet {stepStateClass(job.conclusion, job.status)}"
                aria-hidden="true"
              >
                {runStateIcon(jobKind)}
              </span>
              <span class="job-main">
                <span class="job-name">{job.name}</span>
                <span class="job-sub">
                  {stepStateLabel(job.conclusion, job.status)}
                  {#if jobDuration} · {jobDuration}{/if}
                </span>
              </span>
              <span class="job-status {stepStateClass(job.conclusion, job.status)}">
                {stepStateLabel(job.conclusion, job.status)}
              </span>
            </button>

            {#if selectedJobId === job.database_id && job.steps.length > 0}
              <ol class="steps" aria-label={`Steps in ${job.name}`}>
                {#each job.steps as step (step.number)}
                  {@const stepKind = runStateKind({
                    status: step.status,
                    conclusion: step.conclusion,
                  })}
                  {@const stepDuration = activityDurationLabel(
                    step.started_at,
                    step.completed_at,
                    step.status,
                    now,
                  )}
                  {@const stepKeyStr = `${job.name}\u0000${step.name}`}
                  <li class="step-item">
                    <button
                      class="step-row-btn"
                      class:active={activeStepKey === stepKeyStr}
                      type="button"
                      title={`Jump to ${step.name} logs`}
                      onclick={() => jumpToStep(job.name, step.name)}
                    >
                      <span
                        class="step-icon {stepStateClass(step.conclusion, step.status)}"
                        aria-hidden="true"
                      >
                        {runStateIcon(stepKind)}
                      </span>
                      <span class="step-name" title={step.name}>{step.name}</span>
                      <span class="step-meta">
                        {stepStateLabel(step.conclusion, step.status)}
                        {#if stepDuration} · {stepDuration}{/if}
                      </span>
                      <span class="step-jump-icon" aria-hidden="true">→</span>
                    </button>
                  </li>
                {/each}
              </ol>
            {/if}
          {/each}
        </div>
      {/if}
    </aside>

    <section class="log-pane" aria-label="Workflow log">
      <div class="log-head">
        <h3 class="pane-title">
          {selectedJob ? selectedJob.name : "All jobs"}
          {#if failedOnly}<span class="scope-chip">failed steps</span>{/if}
        </h3>
        {#if live}
          <span class="live-badge small">
            <span class="live-dot" aria-hidden="true"></span>
            Live
          </span>
        {:else if active}
          <span class="paused-chip">Paused</span>
        {/if}
        <span class="spacer"></span>
        {#if selectedJob}
          <button
            class="tb"
            type="button"
            title="Back to the combined log"
            onclick={() => selectJob(null)}
          >
            All jobs
          </button>
        {/if}
      </div>

      <div class="log-body">
        <!-- Remount per run so search, collapse state, and tailing do not leak
             from the previously opened run into this one. -->
        {#key `${repoId}:${runId}`}
          <WorkflowLogViewer
            text={logText}
            jobs={logJobs}
            loading={logLoading}
            error={logError}
            live={live}
            liveConsole={active}
            onOpenLive={() => void openOnGitHub()}
            emptyText={emptyLogText}
            targetStepKey={activeStepKey}
          />
        {/key}
      </div>
    </section>
  </div>
</div>

<style>
  .run-view {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    min-width: 0;
    background: var(--m3-surface);
    container-type: inline-size;
  }

  .run-head {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding: 0.4rem 0.7rem 0.55rem;
    border-bottom: 1px solid var(--app-border, var(--m3-outline-variant));
    background: var(--m3-surface-container, var(--m3-surface));
  }

  .head-top {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
  }

  .crumbs {
    display: flex;
    align-items: baseline;
    gap: 0.35rem;
    min-width: 0;
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant);
  }

  .crumb-muted {
    flex: none;
  }

  .crumb-sep {
    flex: none;
    opacity: 0.6;
  }

  .crumb-current {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface);
    font-weight: 500;
  }

  .run-number {
    flex: none;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.68rem;
    color: var(--m3-on-surface-variant);
  }

  .spacer {
    margin-left: auto;
  }

  .title-row {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    flex-wrap: wrap;
    min-width: 0;
  }

  .run-title {
    margin: 0;
    min-width: 0;
    font-size: 0.95rem;
    font-weight: 600;
    line-height: 1.25;
    color: var(--m3-on-surface);
    overflow-wrap: anywhere;
  }

  .status-chip {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.05rem 0.5rem;
    font-size: 0.66rem;
    font-weight: 600;
    white-space: nowrap;
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface-variant);
  }

  .status-chip.run-success {
    background: var(--m3-primary-container);
    color: var(--m3-on-primary-container);
  }

  .status-chip.run-failure {
    background: var(--m3-error-container);
    color: var(--m3-on-error-container);
  }

  .status-chip.run-running {
    background: var(--m3-tertiary-container);
    color: var(--m3-on-tertiary-container);
  }

  .status-icon {
    font-size: 0.62rem;
  }

  .live-badge {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.66rem;
    font-weight: 600;
    color: var(--m3-primary);
  }

  .live-badge.paused {
    color: var(--m3-on-surface-variant);
  }

  .live-badge.small {
    font-size: 0.62rem;
    font-weight: 500;
  }

  .live-dot {
    width: 0.42rem;
    height: 0.42rem;
    border-radius: 50%;
    background: currentColor;
    animation: pulse 1.4s ease-in-out infinite;
  }

  .paused .live-dot {
    animation: none;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.25;
    }
  }

  .paused-chip {
    flex: none;
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0 0.45rem;
    font-size: 0.62rem;
    background: var(--m3-surface-container-highest);
    color: var(--m3-on-surface-variant);
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 0.35rem 0.75rem;
    flex-wrap: wrap;
    font-size: 0.68rem;
    color: var(--m3-on-surface-variant);
  }

  .meta-item {
    display: inline-flex;
    align-items: baseline;
    gap: 0.25rem;
    white-space: nowrap;
  }

  .meta-label {
    opacity: 0.75;
  }

  .branch {
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.68rem;
    max-width: 16rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sha {
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant);
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.66rem;
    padding: 0 0.3rem;
    cursor: pointer;
  }

  .sha:hover {
    background: var(--app-hover, var(--m3-surface-container-high));
    color: var(--m3-on-surface);
  }

  .sha:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    flex-wrap: wrap;
  }

  .tb {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant);
    font: inherit;
    font-size: 0.68rem;
    line-height: 1.4;
    padding: 0.1rem 0.6rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .tb:hover:not(:disabled) {
    background: var(--app-hover, var(--m3-surface-container-high));
    color: var(--m3-on-surface);
  }

  .tb:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .tb:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .tb.active {
    background: var(--m3-primary-container);
    border-color: transparent;
    color: var(--m3-on-primary-container);
  }

  .tb.danger {
    color: var(--m3-error, var(--app-deleted));
    border-color: color-mix(in srgb, currentColor 45%, transparent);
  }

  .tb.close {
    font-size: 0.9rem;
    line-height: 1;
    padding: 0.05rem 0.4rem;
  }

  .inline-error {
    padding: 0.3rem 0.5rem;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-error-container);
    color: var(--m3-on-error-container);
    font-size: 0.68rem;
    overflow-wrap: anywhere;
  }

  .run-body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(14rem, 19rem) minmax(0, 1fr);
    gap: 0.5rem;
    padding: 0.5rem;
  }

  .jobs-pane,
  .log-pane {
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    overflow: hidden;
  }

  .jobs-head,
  .log-head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
    padding: 0.35rem 0.5rem;
    border-bottom: 1px solid var(--app-border, var(--m3-outline-variant));
    background: var(--m3-surface-container, var(--m3-surface));
  }

  .pane-title {
    margin: 0;
    display: inline-flex;
    align-items: baseline;
    gap: 0.35rem;
    min-width: 0;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--m3-on-surface);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pane-count {
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-surface-container-highest);
    color: var(--m3-on-surface-variant);
    font-size: 0.62rem;
    font-weight: 500;
    padding: 0 0.35rem;
  }

  .scope-chip {
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-tertiary-container);
    color: var(--m3-on-tertiary-container);
    font-size: 0.6rem;
    font-weight: 500;
    padding: 0 0.4rem;
  }

  .filter-count {
    border-radius: var(--m3-shape-full, 9999px);
    background: color-mix(in srgb, currentColor 15%, transparent);
    font-size: 0.6rem;
    padding: 0 0.3rem;
  }

  .job-progress {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.62rem;
    color: var(--m3-on-surface-variant);
    white-space: nowrap;
  }

  .job-progress-track {
    width: 3.2rem;
    height: 0.3rem;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    overflow: hidden;
  }

  .job-progress-fill {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--m3-primary);
    transition: width 0.35s ease;
  }

  .job-progress-label {
    font-variant-numeric: tabular-nums;
  }

  .jobs-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }

  .job-row {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 0.45rem;
    width: 100%;
    border: 1px solid transparent;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    padding: 0.3rem 0.4rem;
    cursor: pointer;
  }

  .job-row:hover {
    background: var(--app-hover, var(--m3-surface-container-high));
  }

  .job-row:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .job-row.selected {
    border-color: var(--m3-primary);
    background: color-mix(in srgb, var(--m3-primary) 10%, transparent);
  }

  .job-row.all {
    border-bottom: 1px solid var(--m3-outline-variant);
    border-radius: 0;
    padding-bottom: 0.4rem;
    margin-bottom: 0.15rem;
  }

  .job-bullet {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.1rem;
    height: 1.1rem;
    font-size: 0.7rem;
    border-radius: 50%;
    flex: none;
  }

  .all-bullet {
    color: var(--m3-on-surface-variant);
    background: var(--m3-surface-container-highest);
    font-size: 0.62rem;
  }

  .job-main {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .job-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.74rem;
    font-weight: 500;
    color: var(--m3-on-surface);
  }

  .job-sub {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.64rem;
    color: var(--m3-on-surface-variant);
  }

  .job-status {
    font-size: 0.62rem;
    white-space: nowrap;
  }

  .run-success {
    color: var(--app-added, var(--m3-primary));
  }

  .run-failure {
    color: var(--m3-error, var(--app-deleted));
  }

  .run-running {
    color: var(--m3-primary);
  }

  .run-queued,
  .run-cancelled,
  .run-skipped,
  .run-unknown {
    color: var(--m3-on-surface-variant);
  }

  .steps {
    list-style: none;
    margin: 0.1rem 0 0.25rem 0.6rem;
    padding: 0.1rem 0 0.1rem 0.5rem;
    border-left: 2px solid var(--m3-outline-variant);
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }

  .step-item {
    margin: 0;
    padding: 0;
  }

  .step-row-btn {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    width: 100%;
    border: 1px solid transparent;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    padding: 0.2rem 0.45rem;
    font-size: 0.68rem;
    cursor: pointer;
    transition: background 0.15s ease, border-color 0.15s ease;
  }

  .step-row-btn:hover {
    background: var(--app-hover, var(--m3-surface-container-high));
    border-color: var(--app-border, var(--m3-outline-variant));
  }

  .step-row-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .step-row-btn.active {
    background: color-mix(in srgb, var(--m3-primary) 14%, transparent);
    border-color: var(--m3-primary);
    font-weight: 500;
  }

  .step-jump-icon {
    flex: none;
    font-size: 0.64rem;
    color: var(--m3-on-surface-variant);
    opacity: 0;
    transition: opacity 0.15s ease, transform 0.15s ease;
  }

  .step-row-btn:hover .step-jump-icon,
  .step-row-btn.active .step-jump-icon {
    opacity: 0.8;
    transform: translateX(2px);
  }

  .step-icon {
    flex: none;
    font-size: 0.6rem;
    width: 0.8rem;
    text-align: center;
  }

  .step-name {
    min-width: 0;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface);
  }

  .step-meta {
    flex: none;
    color: var(--m3-on-surface-variant);
    font-size: 0.62rem;
    opacity: 0.9;
  }

  .pane-state {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    padding: 1rem;
    text-align: center;
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant);
  }

  .pane-state p {
    margin: 0;
  }

  .state-hint {
    max-width: 18rem;
    font-size: 0.66rem;
    line-height: 1.4;
    opacity: 0.85;
    overflow-wrap: anywhere;
  }

  .state-spinner {
    width: 1rem;
    height: 1rem;
    border: 2px solid var(--m3-outline-variant);
    border-top-color: var(--m3-primary);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .pane-note {
    margin: 0.5rem 0.4rem;
    font-size: 0.68rem;
    color: var(--m3-on-surface-variant);
  }

  .log-body {
    flex: 1;
    min-height: 0;
    display: flex;
    padding: 0.4rem;
  }

  .log-body > :global(*) {
    flex: 1;
    min-height: 0;
    min-width: 0;
  }

  @container (max-width: 55rem) {
    .run-body {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: minmax(7rem, 32%) minmax(0, 1fr);
    }
    .jobs-pane {
      min-height: 0;
    }
  }

  @container (max-width: 38rem) {
    .run-head {
      padding: 0.35rem 0.5rem 0.45rem;
    }
    .actions {
      gap: 0.2rem;
    }
    .tb {
      padding: 0.08rem 0.45rem;
      font-size: 0.64rem;
    }
    .run-body {
      padding: 0.35rem;
      gap: 0.35rem;
      grid-template-rows: minmax(6rem, 30%) minmax(0, 1fr);
    }
  }
</style>
