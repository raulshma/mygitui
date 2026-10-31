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
  import { Button } from "$lib/components/ui/button/index.js";
  import { Toggle } from "$lib/components/ui/toggle/index.js";
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

  // Tailwind replacements for the removed scoped styles: chip/icon tones per
  // run-state class plus the shared toolbar-button utilities.
  const chipTone: Record<string, string> = {
    "run-success": "bg-[var(--app-added-bg)] text-[var(--app-added)]",
    "run-failure": "bg-[var(--app-deleted-bg)] text-[var(--app-deleted)]",
    "run-running": "bg-primary/10 text-primary",
    "run-queued": "bg-accent text-muted-foreground",
    "run-cancelled": "bg-accent text-muted-foreground",
    "run-skipped": "bg-accent text-muted-foreground",
    "run-unknown": "bg-accent text-muted-foreground",
  };
  const runTone: Record<string, string> = {
    "run-success": "text-[var(--app-added)]",
    "run-failure": "text-[var(--app-deleted)]",
    "run-running": "text-primary",
    "run-queued": "text-muted-foreground",
    "run-cancelled": "text-muted-foreground",
    "run-skipped": "text-muted-foreground opacity-60",
    "run-unknown": "text-muted-foreground",
  };
  // Pill geometry/type only — border, background, hover and disabled chrome
  // come from the ui Button/Toggle primitives this is passed to as `class`.
  const tb =
    "flex-none gap-1 whitespace-nowrap rounded-full px-[0.6rem] py-[0.1rem] text-[0.68rem] leading-[1.4] font-normal text-muted-foreground @max-[38rem]:px-[0.45rem] @max-[38rem]:text-[0.64rem]";

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

<div class="run-view @container flex h-full min-h-0 min-w-0 flex-col bg-card">
  <header class="run-head flex-none flex flex-col gap-[0.35rem] border-b border-border bg-popover px-[0.7rem] pt-[0.4rem] pb-[0.55rem] @max-[38rem]:px-2 @max-[38rem]:pt-[0.35rem] @max-[38rem]:pb-[0.45rem]">
    <div class="head-top flex min-w-0 items-center gap-[0.4rem]">
      <span class="crumbs flex min-w-0 items-baseline gap-[0.35rem] text-[0.72rem] text-muted-foreground">
        <span class="crumb-muted flex-none">Actions</span>
        <span class="crumb-sep flex-none opacity-60" aria-hidden="true">/</span>
        <span class="crumb-current min-w-0 truncate font-medium text-foreground" title={run?.name ?? "Workflow run"}>
          {run?.name ?? "Workflow run"}
        </span>
        {#if run}<span class="run-number flex-none font-mono text-[0.68rem] text-muted-foreground">#{run.number}</span>{/if}
      </span>
      <span class="spacer ml-auto"></span>
      {#if onPopout}
        <Button
          class={tb}
          variant="outline"
          type="button"
          title="Open this run in its own window"
          onclick={onPopout}
        >
          Pop out
        </Button>
      {/if}
      {#if onClose}
        <Button
          class="close {tb} px-[0.4rem] py-[0.05rem] text-[0.9rem] leading-none"
          variant="outline"
          type="button"
          aria-label="Close run details"
          title="Close run details"
          onclick={onClose}
        >
          ×
        </Button>
      {/if}
    </div>

    <div class="title-row flex min-w-0 flex-wrap items-center gap-[0.45rem]">
      <span class="status-chip flex-none inline-flex items-center gap-1 whitespace-nowrap rounded-full px-2 py-[0.05rem] text-[0.66rem] font-semibold {chipTone[runStateClass(kind)] ?? "bg-accent text-muted-foreground"}">
        <span class="status-icon text-[0.62rem]" aria-hidden="true">{runStateIcon(kind)}</span>
        {statusText}
      </span>
      {#if active}
        <span
          class="live-badge flex-none inline-flex items-center gap-[0.3rem] text-[0.66rem] font-semibold {autoRefresh
            ? "text-primary"
            : "text-muted-foreground"}"
          class:paused={!autoRefresh}
        >
          <span class="live-dot size-[0.42rem] rounded-full bg-current {autoRefresh ? "animate-pulse" : ""}" aria-hidden="true"></span>
          {autoRefresh ? "Live" : "Paused"}
        </span>
      {/if}
      <h2 class="run-title m-0 min-w-0 text-[0.95rem] font-semibold leading-[1.25] text-foreground [overflow-wrap:anywhere]">{run?.display_title || run?.name || `Run ${runId}`}</h2>
    </div>

    <div class="meta flex flex-wrap items-center gap-x-3 gap-y-[0.35rem] text-[0.68rem] text-muted-foreground" aria-label="Run metadata">
      {#if run}
        <span class="meta-item inline-flex items-baseline gap-1 whitespace-nowrap">
          <span class="meta-label opacity-75">Trigger</span>
          {workflowEventLabel(run.event)}
        </span>
        {#if run.head_branch}
          <span class="meta-item inline-flex items-baseline gap-1 whitespace-nowrap">
            <span class="meta-label opacity-75">Branch</span>
            <code class="branch max-w-64 truncate font-mono text-[0.68rem]" title={run.head_branch}>{run.head_branch}</code>
          </span>
        {/if}
        {#if run.head_sha}
          <Button
            class="h-auto rounded-sm px-[0.3rem] font-mono text-[0.66rem] font-normal text-muted-foreground"
            variant="outline"
            type="button"
            title="Copy the full commit SHA"
            onclick={() => void copySha()}
          >
            {run.head_sha.slice(0, 7)}
          </Button>
        {/if}
        {#if run.attempt > 1}
          <span class="meta-item inline-flex items-baseline gap-1 whitespace-nowrap">attempt {run.attempt}</span>
        {/if}
        {#if duration}
          <span class="meta-item inline-flex items-baseline gap-1 whitespace-nowrap" title="Elapsed time">
            <span class="meta-label opacity-75">{active ? "Running" : "Took"}</span>
            {duration}
          </span>
        {/if}
        {#if updatedAgo}
          <span class="meta-item inline-flex items-baseline gap-1 whitespace-nowrap">
            <span class="meta-label opacity-75">Updated</span>
            {updatedAgo} ago
          </span>
        {/if}
      {:else if detailLoading}
        <span class="meta-item inline-flex items-baseline gap-1 whitespace-nowrap">Loading run metadata…</span>
      {/if}
    </div>

    <div class="actions flex flex-wrap items-center gap-[0.3rem] @max-[38rem]:gap-[0.2rem]" aria-label="Run actions">
      {#if active}
        <Button
          class={tb}
          variant="outline"
          type="button"
          title={autoRefresh ? "Pause live updates" : "Resume live updates"}
          onclick={toggleAutoRefresh}
        >
          {autoRefresh ? "Pause live" : "Resume live"}
        </Button>
      {/if}
      <Button
        class={tb}
        variant="outline"
        type="button"
        disabled={refreshing}
        onclick={() => void refreshNow()}
      >
        {refreshing ? "Refreshing…" : "Refresh"}
      </Button>
      {#if run}
        {#if active}
          <Button
            class="danger {tb} border-current/45 text-[var(--app-deleted)]"
            variant="outline"
            type="button"
            title="Cancel this run"
            disabled={busy}
            onclick={() => void cancelRun()}
          >
            {busy ? "Cancelling…" : "Cancel"}
          </Button>
        {/if}
        {#if runCanRerun(run)}
          <Button
            class={tb}
            variant="outline"
            type="button"
            title="Rerun all jobs"
            disabled={busy}
            onclick={() => void rerunRun(false)}
          >
            {busy ? "Working…" : "Rerun"}
          </Button>
          {#if runHasFailures(run)}
            <Button
              class={tb}
              variant="outline"
              type="button"
              title="Rerun only the failed jobs"
              disabled={busy}
              onclick={() => void rerunRun(true)}
            >
              Rerun failed
            </Button>
          {/if}
        {/if}
        <Button class={tb} variant="outline" type="button" onclick={() => void openOnGitHub()}>
          Open on GitHub
        </Button>
      {/if}
    </div>

    {#if detailError && detail}
      <div class="inline-error [overflow-wrap:anywhere] rounded-sm bg-destructive/15 px-2 py-[0.3rem] text-[0.68rem] text-destructive" role="alert">
        Could not refresh run details: {detailError}
      </div>
    {/if}
  </header>

  <div class="run-body grid min-h-0 flex-1 grid-cols-[minmax(14rem,19rem)_minmax(0,1fr)] gap-2 p-2 @max-[55rem]:grid-cols-[minmax(0,1fr)] @max-[55rem]:grid-rows-[minmax(7rem,32%)_minmax(0,1fr)] @max-[38rem]:gap-[0.35rem] @max-[38rem]:p-[0.35rem] @max-[38rem]:grid-rows-[minmax(6rem,30%)_minmax(0,1fr)]">
    <aside class="jobs-pane flex min-h-0 min-w-0 flex-col overflow-hidden rounded-sm border border-border bg-card @max-[55rem]:min-h-0" aria-label="Workflow jobs">
      <div class="jobs-head flex-none flex flex-wrap items-center gap-[0.4rem] border-b border-border bg-popover px-2 py-[0.35rem]">
        <h3 class="pane-title m-0 inline-flex min-w-0 items-baseline gap-[0.35rem] truncate text-xs font-semibold text-foreground">
          Jobs
          {#if jobs.length > 0}<span class="pane-count rounded-full bg-accent px-[0.35rem] text-[0.62rem] font-medium text-muted-foreground">{jobs.length}</span>{/if}
        </h3>
        {#if active && jobs.length > 0}
          <span
            class="job-progress inline-flex items-center gap-[0.3rem] whitespace-nowrap text-[0.62rem] text-muted-foreground"
            title={`${jobsDone} of ${jobs.length} jobs finished`}
          >
            <span class="job-progress-track block h-[0.3rem] w-[3.2rem] overflow-hidden rounded-full bg-accent" aria-hidden="true">
              <span class="job-progress-fill block h-full bg-primary transition-[width] duration-[0.35s] ease-out [border-radius:inherit]" style="width: {jobProgressPct}%"></span>
            </span>
            <span class="job-progress-label [font-variant-numeric:tabular-nums]">{jobsDone}/{jobs.length}</span>
          </span>
        {/if}
        {#if failedJobs.length > 0 || failedOnly}
          <Toggle
            class="{tb} {failedOnly ? "border-transparent bg-accent text-accent-foreground" : ""} focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary focus-visible:ring-0"
            pressed={failedOnly}
            onPressedChange={() => toggleFailedOnly()}
            title="Show failed jobs and only failed steps in the log"
          >
            Failed only
            <span class="filter-count rounded-full bg-current/15 px-[0.3rem] text-[0.6rem]">{failedJobs.length}</span>
          </Toggle>
        {/if}
      </div>

      {#if detailLoading && !detail}
        <div class="pane-state flex min-h-0 flex-1 flex-col items-center justify-center gap-[0.4rem] p-4 text-center text-xs text-muted-foreground" aria-live="polite">
          <span class="state-spinner size-4 animate-spin rounded-full border-2 border-border border-t-primary" aria-hidden="true"></span>
          <p class="m-0">Loading jobs…</p>
        </div>
      {:else if detailError && !detail}
        <div class="pane-state flex min-h-0 flex-1 flex-col items-center justify-center gap-[0.4rem] p-4 text-center text-xs text-muted-foreground" role="alert">
          <p class="m-0">Could not load this run.</p>
          <p class="state-hint m-0 max-w-[18rem] text-[0.66rem] leading-[1.4] opacity-85 [overflow-wrap:anywhere]">{detailError}</p>
          <Button class={tb} variant="outline" type="button" onclick={() => void refreshNow()}>
            Try again
          </Button>
        </div>
      {:else if !detail && !listRun}
        <div class="pane-state flex min-h-0 flex-1 flex-col items-center justify-center gap-[0.4rem] p-4 text-center text-xs text-muted-foreground" aria-live="polite">
          <p class="m-0">This run is no longer available.</p>
          <p class="state-hint m-0 max-w-[18rem] text-[0.66rem] leading-[1.4] opacity-85 [overflow-wrap:anywhere]">
            It may have been removed, or the run list has not loaded yet.
          </p>
        </div>
      {:else}
        <div class="jobs-list flex min-h-0 flex-1 flex-col gap-[0.1rem] overflow-y-auto p-1" role="group" aria-label="Jobs in this run">
          <button
            class="job-row all mb-[0.15rem] grid w-full cursor-pointer grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[0.45rem] rounded-none border border-transparent border-b-border bg-transparent px-[0.4rem] pb-[0.4rem] pt-[0.3rem] text-left text-inherit hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {selectedJobId === null
              ? "border-primary bg-primary/10"
              : ""}"
            class:selected={selectedJobId === null}
            type="button"
            aria-pressed={selectedJobId === null}
            title="Show the combined log of every job"
            onclick={() => selectJob(null)}
          >
            <span class="job-bullet all-bullet inline-flex size-[1.1rem] flex-none items-center justify-center rounded-full bg-accent text-[0.62rem] text-muted-foreground" aria-hidden="true">▤</span>
            <span class="job-main flex min-w-0 flex-col">
              <span class="job-name truncate text-[0.74rem] font-medium text-foreground">All jobs</span>
              <span class="job-sub truncate text-[0.64rem] text-muted-foreground">
                {failedOnly ? "Failed steps across the run" : "Combined log"}
              </span>
            </span>
          </button>

          {#if visibleJobs.length === 0}
            <p class="pane-note m-[0.5rem_0.4rem] text-[0.68rem] text-muted-foreground">
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
              class="job-row grid w-full cursor-pointer grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[0.45rem] rounded-sm border border-transparent bg-transparent px-[0.4rem] py-[0.3rem] text-left text-inherit hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {selectedJobId === job.database_id
                ? "border-primary bg-primary/10"
                : ""}"
              class:selected={selectedJobId === job.database_id}
              type="button"
              aria-pressed={selectedJobId === job.database_id}
              title={`Show the log of ${job.name}`}
              onclick={() => selectJob(job.database_id)}
            >
              <span
                class="job-bullet inline-flex size-[1.1rem] flex-none items-center justify-center rounded-full text-[0.7rem] {stepStateClass(job.conclusion, job.status)} {runTone[stepStateClass(job.conclusion, job.status)] ?? "text-muted-foreground"}"
                aria-hidden="true"
              >
                {runStateIcon(jobKind)}
              </span>
              <span class="job-main flex min-w-0 flex-col">
                <span class="job-name truncate text-[0.74rem] font-medium text-foreground">{job.name}</span>
                <span class="job-sub truncate text-[0.64rem] text-muted-foreground">
                  {stepStateLabel(job.conclusion, job.status)}
                  {#if jobDuration} · {jobDuration}{/if}
                </span>
              </span>
              <span class="job-status whitespace-nowrap text-[0.62rem] {stepStateClass(job.conclusion, job.status)} {runTone[stepStateClass(job.conclusion, job.status)] ?? "text-muted-foreground"}">
                {stepStateLabel(job.conclusion, job.status)}
              </span>
            </button>

            {#if selectedJobId === job.database_id && job.steps.length > 0}
              <ol class="steps m-0 list-none flex flex-col gap-[0.1rem] border-l-2 border-border pb-[0.1rem] pl-[0.5rem] pt-[0.1rem] ml-[0.6rem] mb-[0.25rem] mt-[0.1rem]" aria-label={`Steps in ${job.name}`}>
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
                  <li class="step-item m-0 p-0">
                    <button
                      class="step-row-btn group flex w-full cursor-pointer items-center gap-[0.4rem] rounded-sm border px-[0.45rem] py-[0.2rem] text-left text-[0.68rem] text-inherit transition-[background-color,border-color] duration-150 hover:border-border hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary {activeStepKey === stepKeyStr
                        ? "border-primary bg-primary/[0.14] font-medium"
                        : "border-transparent bg-transparent"}"
                      class:active={activeStepKey === stepKeyStr}
                      type="button"
                      title={`Jump to ${step.name} logs`}
                      onclick={() => jumpToStep(job.name, step.name)}
                    >
                      <span
                        class="step-icon w-[0.8rem] flex-none text-center text-[0.6rem] {stepStateClass(step.conclusion, step.status)} {runTone[stepStateClass(step.conclusion, step.status)] ?? "text-muted-foreground"}"
                        aria-hidden="true"
                      >
                        {runStateIcon(stepKind)}
                      </span>
                      <span class="step-name min-w-0 flex-1 truncate text-foreground" title={step.name}>{step.name}</span>
                      <span class="step-meta flex-none text-[0.62rem] text-muted-foreground opacity-90">
                        {stepStateLabel(step.conclusion, step.status)}
                        {#if stepDuration} · {stepDuration}{/if}
                      </span>
                      <span class="step-jump-icon flex-none text-[0.64rem] text-muted-foreground opacity-0 transition-[opacity,transform] duration-150 group-hover:translate-x-[2px] group-hover:opacity-80 {activeStepKey === stepKeyStr ? "translate-x-[2px] opacity-80" : ""}" aria-hidden="true">→</span>
                    </button>
                  </li>
                {/each}
              </ol>
            {/if}
          {/each}
        </div>
      {/if}
    </aside>

    <section class="log-pane flex min-h-0 min-w-0 flex-col overflow-hidden rounded-sm border border-border bg-card" aria-label="Workflow log">
      <div class="log-head flex-none flex flex-wrap items-center gap-[0.4rem] border-b border-border bg-popover px-2 py-[0.35rem]">
        <h3 class="pane-title m-0 inline-flex min-w-0 items-baseline gap-[0.35rem] truncate text-xs font-semibold text-foreground">
          {selectedJob ? selectedJob.name : "All jobs"}
          {#if failedOnly}<span class="scope-chip rounded-full bg-muted px-[0.4rem] text-[0.6rem] font-medium text-muted-foreground">failed steps</span>{/if}
        </h3>
        {#if live}
          <span class="live-badge small flex-none inline-flex items-center gap-[0.3rem] text-[0.62rem] font-medium text-primary">
            <span class="live-dot size-[0.42rem] animate-pulse rounded-full bg-current" aria-hidden="true"></span>
            Live
          </span>
        {:else if active}
          <span class="paused-chip flex-none rounded-full bg-accent px-[0.45rem] text-[0.62rem] text-muted-foreground">Paused</span>
        {/if}
        <span class="spacer ml-auto"></span>
        {#if selectedJob}
          <Button
            class={tb}
            variant="outline"
            type="button"
            title="Back to the combined log"
            onclick={() => selectJob(null)}
          >
            All jobs
          </Button>
        {/if}
      </div>

      <div class="log-body flex min-h-0 flex-1 p-[0.4rem]">
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
