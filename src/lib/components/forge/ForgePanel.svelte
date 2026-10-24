<script lang="ts">
  /**
   * Forge panel (M6) — gh-assisted GitHub flow.
   *
   * Banner first: gh missing → install link, gh unauthenticated → copyable
   * `gh auth login`, non-GitHub origin → explanation. When ready, the panel
   * shows the current branch's PR card (state chips, author/stats/labels,
   * checks with pass/fail/pending icons + refresh/watch, merge/close/reopen,
   * draft↔ready, checkout, comments and reviews) and the filtered repo PR
   * list. "Create PR" opens {@link CreatePrDialog}; a successful create
   * refreshes context + list here.
   *
   * MOUNTING CONTRACT (docs/contracts.md "M6 FE contracts"): this component
   * takes `{ repoId }` and needs a `"forge"` PanelId registry entry to be
   * placeable — layout files were read-only for this lane, the orchestrator
   * adds the id + `PANEL_META` + the RepoView snippet + `TREE_PANELS`.
   */
  import { untrack } from "svelte";
  import type {
    PrInfo,
    PrMergeMethod,
    PrReviewAction,
    PrStateFilter,
    RunStatusFilter,
    WorkflowRun,
    WorkflowRunFilters,
  } from "$lib/ipc/client";
  import { forgeStore, openExternal } from "$lib/stores/forge.svelte";
  import { refreshStatus } from "$lib/stores/tabs.svelte";
  import { emitUiEvent } from "$lib/palette/events";
  import { toast } from "$lib/toast";
  import DialogShell from "$lib/components/safety/DialogShell.svelte";
  import CreatePrDialog from "./CreatePrDialog.svelte";
  import {
    GH_AUTH_COMMAND,
    GH_INSTALL_DOCS_URL,
    activeWorkflows,
    checkIcon,
    checkStateClass,
    checkSummaryLabel,
    isPrOpen,
    parseWorkflowInputs,
    prAgeLabel,
    prForBranch,
    prMergeBlockedReason,
    prStateClass,
    prStateLabel,
    reviewDecisionLabel,
    runCanRerun,
    runElapsedLabel,
    runHasFailures,
    runIsActive,
    runStateClass,
    runStateIcon,
    runStateKind,
    runStateLabel,
    statusBanner,
    summarizeChecks,
    workflowEventLabel,
    workflowStateLabel,
    worstChipClass,
  } from "./forgeModel";

  let { repoId }: { repoId: string } = $props();

  type ForgeTab = "prs" | "actions";
  let tab = $state<ForgeTab>("prs");

  let dialogOpen = $state(false);
  /** Ensure() runs once per repo (status flips would otherwise re-trigger). */
  let ensuredRepo = $state<string | null>(null);
  /** Currently expanded PR row (its checks/actions render inline). */
  let expandedNumber = $state<number | null>(null);
  /** PRs whose checks auto-refresh every 10 s (user-toggled). */
  let watching = $state<number[]>([]);

  // Actions tab state.
  let runsEnsuredFor = $state<string | null>(null);
  let branchFilter = $state("");
  let statusFilter = $state<RunStatusFilter>("");
  let workflowFilter = $state("");
  /** Ticks elapsed labels and powers the run-list auto refresh. */
  let now = $state(Date.now());
  // Dispatch dialog.
  let dispatchOpen = $state(false);
  let dispatchWorkflow = $state("");
  let dispatchRef = $state("");
  let dispatchInputs = $state("");
  let dispatchError = $state("");

  // Merge dialog.
  let mergeOpen = $state(false);
  let mergeTarget = $state<PrInfo | null>(null);
  let mergeMethod = $state<PrMergeMethod>("squash");
  let deleteBranch = $state(true);
  let autoMerge = $state(false);
  let mergeError = $state("");

  // Comment/review/close/reopen dialog.
  type TextAction = "comment" | "approve" | "changes" | "close" | "reopen";
  let textOpen = $state(false);
  let textAction = $state<TextAction>("comment");
  let textTarget = $state<PrInfo | null>(null);
  let textBody = $state("");
  let textError = $state("");

  $effect(() => {
    if (ensuredRepo === repoId) return;
    ensuredRepo = repoId;
    expandedNumber = null;
    watching = [];
    runsEnsuredFor = null;
    branchFilter = "";
    statusFilter = "";
    workflowFilter = "";
    dispatchOpen = false;
    void forgeStore.ensure(repoId);
    if (untrack(() => tab) === "actions") {
      runsEnsuredFor = repoId;
      void forgeStore.ensureActions(repoId);
    }
  });

  // Check watcher: one interval for every toggled PR; cleanup on unmount.
  $effect(() => {
    const numbers = watching;
    if (numbers.length === 0) return;
    const timer = setInterval(() => {
      for (const number of numbers) void forgeStore.refreshChecks(repoId, number);
    }, 10_000);
    return () => clearInterval(timer);
  });

  const banner = $derived(statusBanner(forgeStore.status));
  const context = $derived(forgeStore.contextFor(repoId));
  const contextError = $derived(forgeStore.contextErrorFor(repoId));
  const prs = $derived(forgeStore.prsFor(repoId));
  const prsLoading = $derived(forgeStore.prsLoadingFor(repoId));
  const filter = $derived(forgeStore.filterFor(repoId));
  const branchPr = $derived(prForBranch(prs, context?.branch ?? ""));
  const listedPrs = $derived(
    prs
      .filter((pr) => pr.number !== branchPr?.number)
      .sort((a, b) => b.number - a.number),
  );
  const branchAge = $derived(branchPr ? prAgeLabel(branchPr.created_at) : null);
  const emptyLabel = $derived(
    prsLoading
      ? "Loading pull requests…"
      : filter === "open"
        ? "No other open pull requests."
        : "No pull requests match this filter.",
  );

  // Actions tab.
  const runs = $derived(forgeStore.runsFor(repoId));
  const runsLoading = $derived(forgeStore.runsLoadingFor(repoId));
  const runsError = $derived(forgeStore.runsErrorFor(repoId));
  const workflows = $derived(forgeStore.workflowsFor(repoId));
  const workflowsLoading = $derived(forgeStore.workflowsLoadingFor(repoId));
  const workflowsError = $derived(forgeStore.workflowsErrorFor(repoId));
  const dispatchable = $derived(activeWorkflows(workflows));
  const dispatchBusy = $derived(forgeStore.isDispatchBusy(repoId));
  const activeRunCount = $derived(runs.filter((run) => runIsActive(run)).length);

  // Keep elapsed labels ticking and the list fresh while runs are active.
  // The refresh is silent so the toolbar does not flash "Loading…" every
  // few seconds.
  $effect(() => {
    if (tab !== "actions" || activeRunCount === 0) return;
    const timer = setInterval(() => (now = Date.now()), 1_000);
    return () => clearInterval(timer);
  });

  $effect(() => {
    if (tab !== "actions" || activeRunCount === 0) return;
    const targetRepo = repoId;
    const filters = currentRunFilters();
    const timer = setInterval(() => {
      if (
        typeof document !== "undefined" &&
        document.visibilityState === "hidden"
      ) {
        return;
      }
      void forgeStore.refreshRuns(targetRepo, filters, true);
    }, 10_000);
    return () => clearInterval(timer);
  });

  $effect(() => {
    if (!dispatchOpen || dispatchWorkflow) return;
    const first = dispatchable[0];
    if (first) dispatchWorkflow = first.name;
  });

  // Checks load lazily for the current branch's PR. `untrack` is required:
  // refreshChecks spreads `checksLoading` while updating it, and tracking that
  // read here would make the effect depend on state it writes (infinite loop).
  $effect(() => {
    if (!branchPr || !isPrOpen(branchPr.state)) return;
    const number = branchPr.number;
    untrack(() => void forgeStore.refreshChecks(repoId, number));
  });

  const canCreate = $derived(banner.kind === "ready" && !!context?.branch);
  const mergeBusy = $derived(
    mergeTarget ? forgeStore.isBusy(repoId, mergeTarget.number) : false,
  );
  const textBusy = $derived(
    textTarget ? forgeStore.isBusy(repoId, textTarget.number) : false,
  );

  /** Opens a URL in the system browser (toast on failure). */
  async function openUrl(url: string): Promise<void> {
    const ok = await openExternal(url);
    if (!ok) toast(`Could not open ${url}`, { kind: "error" });
  }

  async function copyAuthCommand(): Promise<void> {
    try {
      await navigator.clipboard.writeText(GH_AUTH_COMMAND);
      toast(`Copied "${GH_AUTH_COMMAND}" to the clipboard`, { kind: "success" });
    } catch {
      toast("Clipboard is not available", { kind: "error" });
    }
  }

  function errorText(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  function refresh(): void {
    void forgeStore.refreshPrs(repoId, filter);
    void forgeStore.refreshContext(repoId);
  }

  function setFilter(value: string): void {
    expandedNumber = null;
    watching = [];
    void forgeStore.refreshPrs(repoId, value as PrStateFilter);
  }

  /** Expands/collapses one PR row (expanding lazily loads its checks). */
  function toggleExpanded(number: number): void {
    expandedNumber = expandedNumber === number ? null : number;
    if (expandedNumber !== null) void forgeStore.refreshChecks(repoId, number);
  }

  /** Called by the dialog after a successful create (dialog stays open). */
  function onCreated(): void {
    void forgeStore.afterCreate(repoId);
  }

  function toggleWatch(number: number): void {
    watching = watching.includes(number)
      ? watching.filter((n) => n !== number)
      : [...watching, number];
  }

  // -- actions tab ----------------------------------------------------------

  function selectTab(next: ForgeTab): void {
    tab = next;
    if (next === "actions" && runsEnsuredFor !== repoId) {
      runsEnsuredFor = repoId;
      void forgeStore.ensureActions(repoId);
    }
  }

  function currentRunFilters(): WorkflowRunFilters {
    return {
      branch: branchFilter.trim(),
      status: statusFilter,
      workflow: workflowFilter,
    };
  }

  function applyRunFilters(): void {
    void forgeStore.refreshRuns(repoId, currentRunFilters());
  }

  function refreshActions(): void {
    void forgeStore.refreshRuns(repoId, currentRunFilters());
    void forgeStore.refreshWorkflows(repoId);
  }

  /** Opens a run in the main editor (jobs + logs live there). */
  function openRun(run: WorkflowRun): void {
    emitUiEvent("workflow-run-open", {
      repoId,
      runId: run.database_id,
    });
  }

  async function cancelRun(run: WorkflowRun): Promise<void> {
    try {
      await forgeStore.cancelRun(repoId, run.database_id);
      toast(`Cancelled run #${run.number}`, { kind: "success" });
    } catch (err) {
      toast(errorText(err), { kind: "error" });
    }
  }

  async function rerunRun(run: WorkflowRun, failedOnly: boolean): Promise<void> {
    try {
      await forgeStore.rerunRun(repoId, run.database_id, failedOnly);
      toast(
        failedOnly
          ? `Rerunning failed jobs of #${run.number}`
          : `Rerunning #${run.number}`,
        { kind: "success" },
      );
    } catch (err) {
      toast(errorText(err), { kind: "error" });
    }
  }

  function openDispatchDialog(): void {
    dispatchWorkflow = dispatchable[0]?.name ?? "";
    dispatchRef = context?.branch ?? "";
    dispatchInputs = "";
    dispatchError = "";
    dispatchOpen = true;
    if (workflows.length === 0) void forgeStore.refreshWorkflows(repoId);
  }

  async function submitDispatch(): Promise<void> {
    if (!dispatchWorkflow) {
      dispatchError = "Choose a workflow to run.";
      return;
    }
    const parsed = parseWorkflowInputs(dispatchInputs);
    if (parsed.error) {
      dispatchError = parsed.error;
      return;
    }
    dispatchError = "";
    try {
      await forgeStore.dispatchWorkflow(
        repoId,
        dispatchWorkflow,
        dispatchRef,
        parsed.inputs,
      );
      dispatchOpen = false;
      toast(`Dispatched ${dispatchWorkflow}`, { kind: "success" });
    } catch (err) {
      dispatchError = errorText(err);
    }
  }

  // -- action dialogs -------------------------------------------------------

  function openMerge(pr: PrInfo): void {
    mergeTarget = pr;
    mergeError = "";
    mergeMethod = "squash";
    deleteBranch = true;
    autoMerge = false;
    mergeOpen = true;
  }

  async function confirmMerge(): Promise<void> {
    const pr = mergeTarget;
    if (!pr) return;
    mergeError = "";
    try {
      await forgeStore.mergePr(
        repoId,
        pr.number,
        mergeMethod,
        deleteBranch,
        autoMerge,
      );
      mergeOpen = false;
      toast(
        autoMerge
          ? `Auto-merge enabled for #${pr.number}`
          : `Merged #${pr.number}`,
        { kind: "success" },
      );
      mergeTarget = null;
    } catch (err) {
      mergeError = errorText(err);
    }
  }

  function openText(action: TextAction, pr: PrInfo): void {
    textAction = action;
    textTarget = pr;
    textBody = "";
    textError = "";
    textOpen = true;
  }

  const textTitle = $derived.by(() => {
    const n = textTarget?.number ?? 0;
    switch (textAction) {
      case "comment":
        return `Comment on #${n}`;
      case "approve":
        return `Approve #${n}`;
      case "changes":
        return `Request changes on #${n}`;
      case "close":
        return `Close #${n}`;
      case "reopen":
        return `Reopen #${n}`;
    }
  });
  const textConfirm = $derived.by(() => {
    switch (textAction) {
      case "comment":
        return "Comment";
      case "approve":
        return "Approve";
      case "changes":
        return "Request changes";
      case "close":
        return "Close";
      case "reopen":
        return "Reopen";
    }
  });
  const textSuccess = $derived.by(() => {
    switch (textAction) {
      case "comment":
        return "Commented on";
      case "approve":
        return "Approved";
      case "changes":
        return "Requested changes on";
      case "close":
        return "Closed";
      case "reopen":
        return "Reopened";
    }
  });
  const textBodyRequired = $derived(
    textAction === "comment" || textAction === "changes",
  );

  async function confirmText(): Promise<void> {
    const pr = textTarget;
    if (!pr) return;
    const body = textBody.trim();
    if (textBodyRequired && !body) {
      textError = "A message is required for this action.";
      return;
    }
    textError = "";
    try {
      switch (textAction) {
        case "comment":
          await forgeStore.commentPr(repoId, pr.number, body);
          break;
        case "approve":
          await forgeStore.reviewPr(repoId, pr.number, "approve", body);
          break;
        case "changes":
          await forgeStore.reviewPr(repoId, pr.number, "request-changes", body);
          break;
        case "close":
          await forgeStore.closePr(repoId, pr.number, body || null);
          break;
        case "reopen":
          await forgeStore.reopenPr(repoId, pr.number, body || null);
          break;
      }
      textOpen = false;
      toast(`${textSuccess} #${pr.number}`, { kind: "success" });
      textTarget = null;
    } catch (err) {
      textError = errorText(err);
    }
  }

  async function toggleDraft(pr: PrInfo): Promise<void> {
    try {
      await forgeStore.setDraft(repoId, pr.number, !pr.is_draft);
      toast(
        pr.is_draft ? `#${pr.number} is ready for review` : `#${pr.number} is now a draft`,
        { kind: "success" },
      );
      if (pr.is_draft) {
        // Newly ready: checks can now start reporting.
        void forgeStore.refreshChecks(repoId, pr.number);
      }
    } catch (err) {
      toast(errorText(err), { kind: "error" });
    }
  }

  async function checkout(pr: PrInfo): Promise<void> {
    try {
      await forgeStore.checkoutPr(repoId, pr.number);
      await refreshStatus(repoId);
      toast(`Checked out #${pr.number} (${pr.head_ref_name})`, {
        kind: "success",
      });
    } catch (err) {
      toast(errorText(err), { kind: "error" });
    }
  }
</script>

<aside class="forge-panel" aria-label="Pull requests">
  {#snippet prMeta(pr: PrInfo)}
    {@const review = reviewDecisionLabel(pr.review_decision)}
    <div class="pr-meta">
      {#if pr.author}
        <span class="author" title="Author">by {pr.author}</span>
      {/if}
      {#if pr.additions || pr.deletions || pr.changed_files}
        <span
          class="stats"
          title={`${pr.additions} additions, ${pr.deletions} deletions, ${pr.changed_files} changed files`}
        >
          <span class="add">+{pr.additions}</span>
          <span class="del">−{pr.deletions}</span>
          <span class="files">{pr.changed_files} file{pr.changed_files === 1 ? "" : "s"}</span>
        </span>
      {/if}
      {#if review}
        <span class="review">{review}</span>
      {/if}
      {#if pr.mergeable === "CONFLICTING"}
        <span class="warn">Merge conflicts</span>
      {/if}
      {#if pr.labels.length > 0}
        <span class="labels">
          {#each pr.labels as label (label)}
            <span class="label">{label}</span>
          {/each}
        </span>
      {/if}
    </div>
  {/snippet}

  {#snippet prBody(pr: PrInfo)}
    {#if pr.body.trim()}
      <details class="pr-body">
        <summary>Description</summary>
        <div class="pr-body-text">{pr.body}</div>
      </details>
    {/if}
  {/snippet}

  {#snippet actionsBlock(pr: PrInfo)}
    {@const busy = forgeStore.isBusy(repoId, pr.number)}
    {@const blocked = prMergeBlockedReason(pr)}
    <div class="pr-actions" aria-label={`Actions for pull request ${pr.number}`}>
      {#if isPrOpen(pr.state)}
        <button
          class="tb primary"
          type="button"
          title={blocked ?? "Merge pull request"}
          disabled={busy || blocked !== null}
          onclick={() => openMerge(pr)}
        >
          Merge
        </button>
        <button
          class="tb"
          type="button"
          title={pr.is_draft ? "Mark ready for review" : "Convert back to draft"}
          disabled={busy}
          onclick={() => void toggleDraft(pr)}
        >
          {pr.is_draft ? "Ready" : "Draft"}
        </button>
        <button
          class="tb"
          type="button"
          title="Check out this branch locally"
          disabled={busy}
          onclick={() => void checkout(pr)}
        >
          Checkout
        </button>
        <button
          class="tb"
          type="button"
          title="Approve this pull request"
          disabled={busy}
          onclick={() => openText("approve", pr)}
        >
          Approve
        </button>
        <button
          class="tb"
          type="button"
          title="Request changes"
          disabled={busy}
          onclick={() => openText("changes", pr)}
        >
          Request changes
        </button>
        <button
          class="tb"
          type="button"
          title="Add a comment"
          disabled={busy}
          onclick={() => openText("comment", pr)}
        >
          Comment
        </button>
        <button
          class="tb danger"
          type="button"
          title="Close without merging"
          disabled={busy}
          onclick={() => openText("close", pr)}
        >
          Close
        </button>
      {:else}
        {#if pr.state === "CLOSED"}
          <button
            class="tb"
            type="button"
            disabled={busy}
            onclick={() => openText("reopen", pr)}
          >
            Reopen
          </button>
        {/if}
        <button
          class="tb"
          type="button"
          title="Check out this branch locally"
          disabled={busy}
          onclick={() => void checkout(pr)}
        >
          Checkout
        </button>
      {/if}
      <button class="tb" type="button" onclick={() => void openUrl(pr.url)}>
        Open
      </button>
      {#if busy}
        <span class="busy" aria-live="polite">Working…</span>
      {/if}
    </div>
  {/snippet}

  {#snippet checksBlock(pr: PrInfo)}
    {@const checks = forgeStore.checksFor(repoId, pr.number)}
    {@const loading = forgeStore.checksLoadingFor(repoId, pr.number)}
    {@const summary = summarizeChecks(checks ?? [])}
    {@const isWatching = watching.includes(pr.number)}
    <div class="checks">
      <div class="checks-head">
        <span class="chip {worstChipClass(summary.worst)}">
          {checkSummaryLabel(summary)}
        </span>
        <span class="spacer"></span>
        <button
          class="tb"
          type="button"
          class:watching={isWatching}
          title={isWatching
            ? "Stop auto-refreshing checks"
            : "Auto-refresh checks every 10 seconds"}
          onclick={() => toggleWatch(pr.number)}
        >
          {isWatching ? "Watching" : "Watch"}
        </button>
        <button
          class="tb"
          type="button"
          disabled={loading}
          onclick={() => void forgeStore.refreshChecks(repoId, pr.number)}
        >
          {loading ? "Loading…" : "Refresh checks"}
        </button>
      </div>
      {#if checks === null}
        <p class="muted" aria-live="polite">Loading checks…</p>
      {:else if checks.length === 0}
        <p class="muted">No checks reported for this PR.</p>
      {:else}
        <ul class="check-list" role="list" aria-label="CI checks">
          {#each checks as check (check.name)}
            <li class="check">
              <span class="check-icon {checkStateClass(check.state)}" aria-hidden="true">
                {checkIcon(check.state)}
              </span>
              <span class="check-name" title={check.name}>{check.name}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/snippet}

  <div class="toolbar">
    <div class="tabs" role="tablist" aria-label="GitHub panels">
      <button
        class="tab"
        class:active={tab === "prs"}
        type="button"
        role="tab"
        aria-selected={tab === "prs"}
        onclick={() => selectTab("prs")}
      >
        Pull requests
      </button>
      <button
        class="tab"
        class:active={tab === "actions"}
        type="button"
        role="tab"
        aria-selected={tab === "actions"}
        onclick={() => selectTab("actions")}
      >
        Actions
      </button>
    </div>
    {#if forgeStore.status?.available}
      <span class="gh-meta" title="GitHub CLI version">gh {forgeStore.status.version}</span>
    {/if}
    <span class="spacer"></span>
    {#if tab === "prs"}
      <select
        class="filter"
        aria-label="Pull request state filter"
        value={filter}
        onchange={(event) => setFilter(event.currentTarget.value)}
      >
        <option value="open">Open</option>
        <option value="closed">Closed</option>
        <option value="merged">Merged</option>
        <option value="all">All</option>
      </select>
      <button
        class="tb"
        type="button"
        onclick={() => void forgeStore.refreshStatus()}
        disabled={forgeStore.statusLoading}
      >
        {forgeStore.statusLoading ? "Checking…" : "Re-check gh"}
      </button>
      <button class="tb" type="button" onclick={refresh} disabled={prsLoading}>
        {prsLoading ? "Loading…" : "Refresh"}
      </button>
      <button class="act" type="button" disabled={!canCreate} onclick={() => (dialogOpen = true)}>
        Create PR
      </button>
    {:else}
      <input
        class="filter branch-filter"
        aria-label="Run branch filter"
        placeholder="Branch (any)"
        bind:value={branchFilter}
        onchange={applyRunFilters}
      />
      <select
        class="filter"
        aria-label="Run status filter"
        bind:value={statusFilter}
        onchange={applyRunFilters}
      >
        <option value="">Any status</option>
        <option value="queued">Queued</option>
        <option value="in_progress">In progress</option>
        <option value="completed">Completed</option>
        <option value="success">Success</option>
        <option value="failure">Failure</option>
        <option value="cancelled">Cancelled</option>
        <option value="skipped">Skipped</option>
        <option value="timed_out">Timed out</option>
        <option value="action_required">Action required</option>
      </select>
      <select
        class="filter workflow-filter"
        aria-label="Workflow filter"
        bind:value={workflowFilter}
        onchange={applyRunFilters}
      >
        <option value="">All workflows</option>
        {#each workflows as wf (wf.id)}
          <option value={wf.name}>
            {wf.name}{wf.state === "active" ? "" : ` (${workflowStateLabel(wf.state)})`}
          </option>
        {/each}
      </select>
      <button
        class="tb"
        type="button"
        onclick={() => void forgeStore.refreshStatus()}
        disabled={forgeStore.statusLoading}
      >
        {forgeStore.statusLoading ? "Checking…" : "Re-check gh"}
      </button>
      <button
        class="tb"
        type="button"
        onclick={refreshActions}
        disabled={runsLoading || workflowsLoading}
      >
        {runsLoading ? "Loading…" : "Refresh"}
      </button>
      <button
        class="act"
        type="button"
        disabled={!context || dispatchBusy}
        onclick={openDispatchDialog}
      >
        Run workflow
      </button>
    {/if}
  </div>

  {#if banner.kind !== "ready"}
    <div class="banner {banner.kind}" role="status">
      <p class="banner-text">{banner.text}</p>
      {#if banner.kind === "missing"}
        <button class="tb" type="button" onclick={() => void openUrl(GH_INSTALL_DOCS_URL)}>
          Install guide
        </button>
      {:else if banner.kind === "unauthed"}
        <code class="cmd">{GH_AUTH_COMMAND}</code>
        <button class="tb" type="button" onclick={() => void copyAuthCommand()}>Copy</button>
      {/if}
    </div>
  {:else if contextError}
    <div class="banner missing" role="status">
      <p class="banner-text">
        The gh flow is unavailable for this repository: {contextError}
      </p>
    </div>
  {:else if tab === "actions"}
    {#if context}
      <div class="section actions-section">
        <div class="actions-summary">
          <h3 class="section-title">Recent runs</h3>
          <span class="muted" aria-live="polite">
            {runs.length} run{runs.length === 1 ? "" : "s"}{#if activeRunCount > 0}
              · {activeRunCount} active{/if}{runsLoading ? " · loading…" : ""}
          </span>
          {#if workflowsLoading}
            <span class="muted">Refreshing workflows…</span>
          {/if}
        </div>
        {#if runsError}
          <p class="error-inline" role="alert">{runsError}</p>
        {/if}
        {#if workflowsError}
          <p class="error-inline" role="alert">Workflows: {workflowsError}</p>
        {/if}

        {#if runs.length === 0}
          <p class="muted">
            {runsLoading
              ? "Loading workflow runs…"
              : runsError
                ? "Could not load workflow runs."
                : "No workflow runs match these filters."}
          </p>
        {:else}
          <ul class="wf-runs" role="list" aria-label="Workflow runs">
            {#each runs as run (run.database_id)}
              {@const kind = runStateKind(run)}
              {@const active = runIsActive(run)}
              {@const busy = forgeStore.isRunBusy(repoId, run.database_id)}
              {@const age = prAgeLabel(run.created_at, now)}
              {@const duration = runElapsedLabel(run, now)}
              <li class="wf-run">
                <div class="wf-run-line">
                  <button
                    class="wf-run-main"
                    type="button"
                    title="Open run details, jobs and logs"
                    onclick={() => openRun(run)}
                  >
                    <span class="run-chip {runStateClass(kind)}">
                      <span aria-hidden="true">{runStateIcon(kind)}</span>
                      {runStateLabel(run)}
                    </span>
                    <span class="wf-run-name" title={run.name}>{run.name}</span>
                    <span class="wf-run-title" title={run.display_title}>
                      {run.display_title}
                    </span>
                    <span class="wf-run-meta">
                      <span class="pr-no">#{run.number}</span>
                      {#if run.head_branch}
                        <code class="wf-branch" title="Head branch">{run.head_branch}</code>
                      {/if}
                      <span class="wf-event" title="Trigger event">
                        {workflowEventLabel(run.event)}
                      </span>
                      {#if run.attempt > 1}
                        <span class="wf-attempt">attempt {run.attempt}</span>
                      {/if}
                      {#if age}
                        <span class="age">{age} ago</span>
                      {/if}
                      {#if duration}
                        <span class="wf-duration">{duration}</span>
                      {/if}
                    </span>
                  </button>
                  <span class="wf-run-actions">
                    {#if active}
                      <button
                        class="tb danger"
                        type="button"
                        title="Cancel this run"
                        disabled={busy}
                        onclick={() => void cancelRun(run)}
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
                        onclick={() => void rerunRun(run, false)}
                      >
                        {busy ? "Working…" : "Rerun"}
                      </button>
                      {#if runHasFailures(run)}
                        <button
                          class="tb"
                          type="button"
                          title="Rerun only failed jobs"
                          disabled={busy}
                          onclick={() => void rerunRun(run, true)}
                        >
                          Rerun failed
                        </button>
                      {/if}
                    {/if}
                    <button
                      class="tb primary"
                      type="button"
                      title="Open run details, jobs and logs"
                      onclick={() => openRun(run)}
                    >
                      Details
                    </button>
                    <button
                      class="tb"
                      type="button"
                      title="Open run on GitHub"
                      onclick={() => void openUrl(run.url)}
                    >
                      Open
                    </button>
                  </span>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  {:else if context}
    {#if branchPr}
      <div class="card current">
        <div class="card-head">
          <span class="chip {prStateClass(branchPr.state)}">{prStateLabel(branchPr.state)}</span>
          {#if branchPr.is_draft}
            <span class="chip chip-draft">Draft</span>
          {/if}
          <span class="pr-title" title={branchPr.title}>{branchPr.title}</span>
          <span class="pr-no">#{branchPr.number}</span>
        </div>
        <div class="card-sub">
          <span class="refs">{branchPr.head_ref_name} → {branchPr.base_ref_name}</span>
          {#if branchAge}
            <span class="age" title="Opened"
              >opened {branchAge === "just now" ? "just now" : `${branchAge} ago`}</span
            >
          {/if}
        </div>
        {@render prMeta(branchPr)}
        {@render prBody(branchPr)}
        {@render actionsBlock(branchPr)}
        {#if isPrOpen(branchPr.state)}
          {@render checksBlock(branchPr)}
        {/if}
      </div>
    {:else}
      <div class="card empty">
        <p>
          No pull request for branch <code>{context.branch}</code> yet.
        </p>
        <button class="act" type="button" onclick={() => (dialogOpen = true)}>Create PR</button>
      </div>
    {/if}

    <div class="section">
      <h3 class="section-title">
        {filter === "open"
          ? "Open pull requests"
          : filter === "all"
            ? "All pull requests"
            : `${filter[0]?.toUpperCase() ?? ""}${filter.slice(1)} pull requests`}
      </h3>
      {#if listedPrs.length === 0}
        <p class="muted">{emptyLabel}</p>
      {:else}
        <ul class="prs" role="list" aria-label="Pull requests">
          {#each listedPrs as pr (pr.number)}
            <li>
              <div class="pr-line">
                <button
                  class="pr-row"
                  type="button"
                  aria-expanded={expandedNumber === pr.number}
                  title="Show checks and actions"
                  onclick={() => toggleExpanded(pr.number)}
                >
                  <span class="expand-icon" aria-hidden="true">
                    {expandedNumber === pr.number ? "▾" : "▸"}
                  </span>
                  <span class="chip {prStateClass(pr.state)}">{prStateLabel(pr.state)}</span>
                  {#if pr.is_draft}
                    <span class="chip chip-draft">Draft</span>
                  {/if}
                  <span class="pr-title" title={pr.title}>{pr.title}</span>
                  <span class="pr-no">#{pr.number}</span>
                  <span class="refs">{pr.head_ref_name} → {pr.base_ref_name}</span>
                </button>
                <button
                  class="tb"
                  type="button"
                  title="Open on GitHub"
                  onclick={() => void openUrl(pr.url)}
                >
                  Open
                </button>
              </div>
              <div class="pr-sub">
                {@render prMeta(pr)}
              </div>
              {#if expandedNumber === pr.number}
                <div class="expand">
                  {@render prBody(pr)}
                  {@render actionsBlock(pr)}
                  {#if isPrOpen(pr.state)}
                    {@render checksBlock(pr)}
                  {/if}
                </div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  <CreatePrDialog {repoId} open={dialogOpen} onClose={() => (dialogOpen = false)} onCreated={onCreated} />

  <DialogShell
    bind:open={dispatchOpen}
    labelledBy="forge-dispatch-title"
    width="34rem"
    onCancel={() => (dispatchError = "")}
    onsubmit={() => void submitDispatch()}
  >
    <h2 class="sdlg-title" id="forge-dispatch-title">Run workflow</h2>
    <p class="sdlg-message">
      Trigger a <code>workflow_dispatch</code> run on a branch or tag.
    </p>
    <label class="dialog-field">
      <span>Workflow</span>
      <select bind:value={dispatchWorkflow}>
        {#if dispatchable.length === 0}
          <option value="">No active workflows</option>
        {/if}
        {#each dispatchable as wf (wf.id)}
          <option value={wf.name}>{wf.name} — {wf.path}</option>
        {/each}
      </select>
    </label>
    <label class="dialog-field">
      <span>Ref</span>
      <input
        class="dialog-text"
        aria-label="Branch or tag to run on"
        placeholder={context?.branch ?? "main"}
        bind:value={dispatchRef}
      />
    </label>
    <label class="dialog-field dialog-field-top">
      <span>Inputs</span>
      <textarea
        class="dialog-text"
        aria-label="Workflow inputs"
        rows="4"
        placeholder={"One key=value per line\nversion=1.2.3\ndry_run=true"}
        bind:value={dispatchInputs}
      ></textarea>
    </label>
    {#if dispatchError}
      <p class="dialog-error" role="alert">{dispatchError}</p>
    {/if}
    {#snippet actions()}
      <button
        class="sdlg-secondary"
        type="button"
        disabled={dispatchBusy}
        onclick={() => (dispatchOpen = false)}
      >
        Cancel
      </button>
      <button
        class="sdlg-primary"
        type="submit"
        disabled={dispatchBusy || !dispatchWorkflow}
      >
        {dispatchBusy ? "Dispatching…" : "Run workflow"}
      </button>
    {/snippet}
  </DialogShell>

  <DialogShell
    bind:open={mergeOpen}
    labelledBy="forge-merge-title"
    onCancel={() => (mergeTarget = null)}
    onsubmit={() => void confirmMerge()}
  >
    <h2 class="sdlg-title" id="forge-merge-title">Merge pull request</h2>
    <p class="sdlg-message">
      #{mergeTarget?.number} {mergeTarget?.title ?? ""}
    </p>
    <label class="dialog-field">
      <span>Method</span>
      <select bind:value={mergeMethod}>
        <option value="squash">Squash and merge</option>
        <option value="merge">Create a merge commit</option>
        <option value="rebase">Rebase and merge</option>
      </select>
    </label>
    <label class="dialog-check">
      <input type="checkbox" bind:checked={deleteBranch} />
      Delete the branch after merging
    </label>
    <label class="dialog-check">
      <input type="checkbox" bind:checked={autoMerge} />
      Enable auto-merge (merge when required checks pass)
    </label>
    {#if mergeError}
      <p class="dialog-error" role="alert">{mergeError}</p>
    {/if}
    {#snippet actions()}
      <button
        class="sdlg-secondary"
        type="button"
        disabled={mergeBusy}
        onclick={() => {
          mergeOpen = false;
          mergeTarget = null;
        }}
      >
        Cancel
      </button>
      <button class="sdlg-primary" type="submit" disabled={mergeBusy}>
        {autoMerge ? "Enable auto-merge" : "Merge"}
      </button>
    {/snippet}
  </DialogShell>

  <DialogShell
    bind:open={textOpen}
    labelledBy="forge-text-title"
    width="32rem"
    onCancel={() => (textTarget = null)}
    onsubmit={() => void confirmText()}
  >
    <h2 class="sdlg-title" id="forge-text-title">{textTitle}</h2>
    <p class="sdlg-message">
      {textTarget?.title ?? ""}
      {textBodyRequired ? " — a message is required." : " — optional message."}
    </p>
    <textarea
      class="dialog-text"
      rows="5"
      bind:value={textBody}
      placeholder={textAction === "close"
        ? "Why is this being closed? (optional)"
        : textAction === "approve"
          ? "Approval note (optional)"
          : "Write your message…"}
      data-autofocus
    ></textarea>
    {#if textError}
      <p class="dialog-error" role="alert">{textError}</p>
    {/if}
    {#snippet actions()}
      <button
        class="sdlg-secondary"
        type="button"
        disabled={textBusy}
        onclick={() => {
          textOpen = false;
          textTarget = null;
        }}
      >
        Cancel
      </button>
      <button
        class={textAction === "close" ? "sdlg-danger" : "sdlg-primary"}
        type="submit"
        disabled={textBusy}
      >
        {textConfirm}
      </button>
    {/snippet}
  </DialogShell>
</aside>

<style>
  .forge-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
    font-size: 0.8125rem;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-wrap: wrap;
    padding: 0.375rem 0.5rem;
    flex: none;
    position: sticky;
    top: 0;
    background: var(--m3-surface, inherit);
    z-index: 1;
  }

  .tabs {
    flex: none;
    display: inline-flex;
    gap: 0.125rem;
    padding: 0.1rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
  }

  .tab {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.12rem 0.6rem;
    cursor: pointer;
  }

  .tab.active {
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-on-surface));
    font-weight: 600;
  }

  .filter {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.1rem 0.25rem;
  }

  .branch-filter {
    width: 8.5rem;
  }

  .workflow-filter {
    max-width: 11rem;
  }

  .gh-meta {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }

  .spacer {
    margin-left: auto;
  }

  .banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin: 0.25rem 0.5rem;
    padding: 0.5rem 0.625rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
  }

  .banner-text {
    margin: 0;
    flex: 1;
    min-width: 12rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .cmd {
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    padding: 0.1rem 0.4rem;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface);
  }

  .card {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    padding: 0.5rem 0.625rem;
    margin: 0.25rem 0.5rem;
  }

  .card-head {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-wrap: wrap;
    min-width: 0;
  }

  .pr-title {
    font-weight: 600;
    color: var(--m3-on-surface);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .pr-no {
    flex: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.72rem;
  }

  .card-sub {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.375rem;
    flex-wrap: wrap;
  }

  .refs {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.72rem;
    font-family: ui-monospace, Consolas, monospace;
  }

  .age {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.6875rem;
  }

  .pr-meta {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-wrap: wrap;
    margin-top: 0.3rem;
    font-size: 0.68rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .pr-body {
    margin-top: 0.35rem;
    font-size: 0.72rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .pr-body summary {
    cursor: pointer;
    color: var(--m3-primary);
    font-size: 0.7rem;
    width: fit-content;
  }

  .pr-body-text {
    margin-top: 0.3rem;
    max-height: 16rem;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    line-height: 1.45;
    padding: 0.4rem 0.5rem;
    border-left: 2px solid var(--m3-outline-variant, var(--m3-primary));
    background: var(--m3-surface-container-low, transparent);
    border-radius: 0 var(--m3-shape-extra-small, 4px)
      var(--m3-shape-extra-small, 4px) 0;
  }

  .author {
    flex: none;
  }

  .stats {
    flex: none;
    display: inline-flex;
    align-items: baseline;
    gap: 0.2rem;
    font-family: ui-monospace, Consolas, monospace;
    font-variant-numeric: tabular-nums;
  }

  .stats .add {
    color: var(--app-added, var(--m3-primary));
  }

  .stats .del {
    color: var(--app-deleted, var(--m3-error));
  }

  .stats .files {
    margin-left: 0.15rem;
  }

  .review {
    flex: none;
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.02rem 0.4rem;
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface);
  }

  .warn {
    flex: none;
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.02rem 0.4rem;
    background: var(--m3-error-container, var(--m3-error));
    color: var(--m3-on-error-container, var(--m3-on-error));
    font-weight: 600;
  }

  .labels {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 0.2rem;
    min-width: 0;
  }

  .label {
    max-width: 9rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.02rem 0.4rem;
    font-size: 0.6563rem;
  }

  .pr-sub {
    padding: 0.15rem 0.5rem 0.2rem 1.5rem;
  }

  .pr-actions {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    flex-wrap: wrap;
    margin-top: 0.45rem;
  }

  .busy {
    font-size: 0.68rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .tb.primary {
    border-color: var(--m3-primary);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font-weight: 600;
  }

  .tb.primary:hover:not(:disabled) {
    background: var(--m3-primary);
    filter: brightness(1.06);
  }

  .tb.danger {
    border-color: var(--m3-error, var(--m3-primary));
    color: var(--m3-error, var(--m3-primary));
  }

  .tb.watching {
    border-color: var(--m3-primary);
    color: var(--m3-primary);
  }

  .dialog-field {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.5rem;
    font-size: 0.8125rem;
  }

  .dialog-field select {
    flex: 1;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    padding: 0.25rem 0.35rem;
  }

  .dialog-field-top {
    align-items: flex-start;
  }

  .dialog-field-top > span {
    padding-top: 0.35rem;
  }

  .dialog-field .dialog-text {
    flex: 1;
    width: auto;
    min-width: 0;
    margin-top: 0;
  }

  .dialog-check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.5rem;
    font-size: 0.8125rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .dialog-text {
    width: 100%;
    box-sizing: border-box;
    margin-top: 0.25rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.8125rem;
    padding: 0.5rem;
    resize: vertical;
  }

  .dialog-error {
    margin: 0.6rem 0 0;
    color: var(--m3-error, var(--m3-primary));
    font-size: 0.75rem;
  }

  .checks {
    margin-top: 0.5rem;
    border-top: 1px solid var(--m3-outline-variant, var(--m3-primary));
    padding-top: 0.375rem;
  }

  .checks-head {
    display: flex;
    align-items: center;
    gap: 0.375rem;
  }

  .check-list {
    margin: 0.25rem 0 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    min-width: 0;
  }

  .check-icon {
    flex: none;
    width: 1em;
    text-align: center;
  }

  .check-pass {
    color: var(--m3-primary, inherit);
  }

  .check-fail {
    color: var(--m3-error, inherit);
    font-weight: 700;
  }

  .check-pending {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .check-skipping {
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .check-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface);
  }

  .chip {
    flex: none;
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.05rem 0.5rem;
    font-size: 0.6563rem;
    font-weight: 600;
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface);
  }

  .chip-pass {
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-on-surface));
  }

  .chip-fail {
    background: var(--m3-error-container, var(--m3-error));
    color: var(--m3-on-error-container, var(--m3-on-error));
  }

  .chip-pending,
  .chip-skipping {
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .chip-none {
    background: var(--m3-surface-container, var(--m3-surface-container-low));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .chip-pr-open {
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-on-surface));
  }

  .chip-pr-merged {
    background: var(--m3-tertiary-container, var(--m3-surface-container-high));
    color: var(--m3-on-tertiary-container, var(--m3-on-surface));
  }

  .chip-pr-closed {
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .chip-draft {
    background: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .section {
    margin: 0.5rem 0.5rem 0.75rem;
  }

  .actions-section {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .actions-summary {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .actions-summary .section-title,
  .actions-summary .muted {
    margin: 0;
  }

  .error-inline {
    margin: 0.15rem 0;
    color: var(--m3-error, var(--m3-primary));
    font-size: 0.75rem;
    overflow-wrap: anywhere;
  }

  .wf-runs {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .wf-run {
    min-width: 0;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    overflow: hidden;
  }

  .wf-run-line {
    display: flex;
    align-items: flex-start;
    flex-wrap: wrap;
    gap: 0.375rem;
    padding: 0.375rem 0.5rem;
    min-width: 0;
  }

  .run-chip {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 0.15rem;
    margin-top: 0.05rem;
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.05rem 0.45rem;
    font-size: 0.6563rem;
    font-weight: 600;
    white-space: nowrap;
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface);
  }

  .run-chip.run-success {
    background: var(--m3-primary-container, var(--m3-surface-container-high));
    color: var(--m3-on-primary-container, var(--m3-on-surface));
  }

  .run-chip.run-failure {
    background: var(--m3-error-container, var(--m3-error));
    color: var(--m3-on-error-container, var(--m3-on-error));
  }

  .run-chip.run-running {
    background: var(--m3-tertiary-container, var(--m3-primary-container));
    color: var(--m3-on-tertiary-container, var(--m3-on-primary-container));
  }

  .run-chip.run-queued,
  .run-chip.run-cancelled,
  .run-chip.run-skipped,
  .run-chip.run-unknown {
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
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
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .wf-run-main {
    flex: 1 1 12rem;
    min-width: 0;
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 0.15rem 0.375rem;
    margin: 0;
    padding: 0;
    border: none;
    background: none;
    color: inherit;
    font: inherit;
    font-size: 0.7813rem;
    text-align: left;
    cursor: pointer;
  }

  .wf-run-main:hover .wf-run-name {
    text-decoration: underline;
  }

  .wf-run-main:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
    border-radius: var(--m3-shape-extra-small, 4px);
  }

  .wf-run-name {
    flex: 0 1 auto;
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }

  .wf-run-title {
    flex: 1 1 8rem;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .wf-run-meta {
    flex: 1 1 100%;
    min-width: 0;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.375rem;
    font-size: 0.68rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .wf-branch {
    max-width: 14rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.68rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .wf-event {
    padding: 0 0.35rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    font-size: 0.6375rem;
    white-space: nowrap;
  }

  .wf-attempt,
  .wf-duration {
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .wf-run-actions {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: 0.2rem;
    margin-left: auto;
  }

  .section-title {
    margin: 0 0 0.25rem;
    font-size: 0.72rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .prs {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .pr-row {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    width: 100%;
    text-align: left;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.7813rem;
    padding: 0.375rem 0.5rem;
    cursor: pointer;
    min-width: 0;
  }

  .pr-line {
    display: flex;
    align-items: flex-start;
    gap: 0.25rem;
    min-width: 0;
  }

  .pr-line .pr-row {
    flex: 1;
  }

  .pr-line .tb {
    margin-top: 0.375rem;
  }

  .expand {
    margin: 0.25rem 0 0.25rem 1.5rem;
  }

  .expand-icon {
    flex: none;
    width: 1em;
    text-align: center;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .pr-row:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .pr-row .pr-title {
    flex: 0 1 auto;
  }

  .pr-row .refs {
    margin-left: auto;
    flex: none;
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card.empty {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
  }

  .card.empty p {
    margin: 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .muted {
    margin: 0.25rem 0;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-size: 0.75rem;
  }

  .tb {
    flex: none;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.5rem;
    cursor: pointer;
  }

  .tb:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .act {
    flex: none;
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.2rem 0.75rem;
    cursor: pointer;
  }

  .tb:focus-visible,
  .act:focus-visible,
  .pr-row:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .tb:disabled,
  .act:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }
</style>
