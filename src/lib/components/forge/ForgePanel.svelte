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
  import { Button } from "$lib/components/ui/button/index.js";
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

  // Tailwind replacements for the removed scoped styles: chip/icon tones per
  // state class plus the shared toolbar-button utilities.
  // Geometry/type only — border, background, hover and disabled chrome come
  // from the ui Button primitive this is passed to as `class`.
  const tb =
    "flex-none gap-1 whitespace-nowrap rounded-sm px-2 py-[0.15rem] text-[0.72rem] leading-[1.4] font-normal text-muted-foreground";
  const chipTone: Record<string, string> = {
    "chip-pass": "bg-primary/15 text-primary",
    "chip-fail": "bg-destructive/15 text-destructive",
    "chip-pending": "bg-accent text-muted-foreground",
    "chip-skipping": "bg-accent text-muted-foreground",
    "chip-none": "bg-muted text-muted-foreground",
    "chip-pr-open": "bg-primary/15 text-primary",
    "chip-pr-merged": "bg-primary/10 text-primary",
    "chip-pr-closed": "bg-accent text-muted-foreground",
    "check-pass": "text-primary",
    "check-fail": "text-destructive font-bold",
    "check-pending": "text-muted-foreground",
    "check-skipping": "text-muted-foreground",
    "run-success": "bg-[var(--app-added-bg)] text-[var(--app-added)]",
    "run-failure": "bg-[var(--app-deleted-bg)] text-[var(--app-deleted)]",
    "run-running": "bg-primary/10 text-primary",
    "run-queued": "bg-accent text-muted-foreground",
    "run-cancelled": "bg-accent text-muted-foreground",
    "run-skipped": "bg-accent text-muted-foreground",
    "run-unknown": "bg-accent text-muted-foreground",
  };

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

<aside class="forge-panel flex min-h-0 flex-col overflow-y-auto text-[0.8125rem]" aria-label="Pull requests">
  {#snippet prMeta(pr: PrInfo)}
    {@const review = reviewDecisionLabel(pr.review_decision)}
    <div class="pr-meta mt-[0.3rem] flex flex-wrap items-center gap-[0.375rem] text-[0.68rem] text-muted-foreground">
      {#if pr.author}
        <span class="author flex-none" title="Author">by {pr.author}</span>
      {/if}
      {#if pr.additions || pr.deletions || pr.changed_files}
        <span
          class="stats inline-flex flex-none items-baseline gap-[0.2rem] font-mono tabular-nums"
          title={`${pr.additions} additions, ${pr.deletions} deletions, ${pr.changed_files} changed files`}
        >
          <span class="add text-[var(--app-added)]">+{pr.additions}</span>
          <span class="del text-[var(--app-deleted)]">−{pr.deletions}</span>
          <span class="files ml-[0.15rem]">{pr.changed_files} file{pr.changed_files === 1 ? "" : "s"}</span>
        </span>
      {/if}
      {#if review}
        <span class="review flex-none rounded-full bg-accent px-[0.4rem] py-[0.02rem] text-foreground">{review}</span>
      {/if}
      {#if pr.mergeable === "CONFLICTING"}
        <span class="warn flex-none rounded-full bg-destructive/15 px-[0.4rem] py-[0.02rem] font-semibold text-destructive">Merge conflicts</span>
      {/if}
      {#if pr.labels.length > 0}
        <span class="labels inline-flex min-w-0 flex-wrap gap-[0.2rem]">
          {#each pr.labels as label (label)}
            <span class="label max-w-36 truncate rounded-full border border-border px-[0.4rem] py-[0.02rem] text-[0.6563rem]">{label}</span>
          {/each}
        </span>
      {/if}
    </div>
  {/snippet}

  {#snippet prBody(pr: PrInfo)}
    {#if pr.body.trim()}
      <details class="pr-body mt-[0.35rem] text-[0.72rem] text-muted-foreground">
        <summary class="w-fit cursor-pointer text-[0.7rem] text-primary">Description</summary>
        <div class="pr-body-text mt-[0.3rem] max-h-64 overflow-auto whitespace-pre-wrap rounded-r-sm border-l-2 border-border bg-muted/40 px-[0.5rem] py-[0.4rem] leading-[1.45] [overflow-wrap:anywhere]">{pr.body}</div>
      </details>
    {/if}
  {/snippet}

  {#snippet actionsBlock(pr: PrInfo)}
    {@const busy = forgeStore.isBusy(repoId, pr.number)}
    {@const blocked = prMergeBlockedReason(pr)}
    <div class="pr-actions mt-[0.45rem] flex flex-wrap items-center gap-1" aria-label={`Actions for pull request ${pr.number}`}>
      {#if isPrOpen(pr.state)}
        <Button class="{tb} border-primary bg-primary font-semibold text-primary-foreground hover:not-disabled:bg-primary hover:not-disabled:brightness-[1.06]" variant="outline"
          type="button"
          title={blocked ?? "Merge pull request"}
          disabled={busy || blocked !== null}
          onclick={() => openMerge(pr)}
        >
          Merge
        </Button>
        <Button class={tb} variant="outline"
          type="button"
          title={pr.is_draft ? "Mark ready for review" : "Convert back to draft"}
          disabled={busy}
          onclick={() => void toggleDraft(pr)}
        >
          {pr.is_draft ? "Ready" : "Draft"}
        </Button>
        <Button class={tb} variant="outline"
          type="button"
          title="Check out this branch locally"
          disabled={busy}
          onclick={() => void checkout(pr)}
        >
          Checkout
        </Button>
        <Button class={tb} variant="outline"
          type="button"
          title="Approve this pull request"
          disabled={busy}
          onclick={() => openText("approve", pr)}
        >
          Approve
        </Button>
        <Button class={tb} variant="outline"
          type="button"
          title="Request changes"
          disabled={busy}
          onclick={() => openText("changes", pr)}
        >
          Request changes
        </Button>
        <Button class={tb} variant="outline"
          type="button"
          title="Add a comment"
          disabled={busy}
          onclick={() => openText("comment", pr)}
        >
          Comment
        </Button>
        <Button class="{tb} danger border-current/45 text-[var(--app-deleted)]" variant="outline"
          type="button"
          title="Close without merging"
          disabled={busy}
          onclick={() => openText("close", pr)}
        >
          Close
        </Button>
      {:else}
        {#if pr.state === "CLOSED"}
          <Button class={tb} variant="outline"
            type="button"
            disabled={busy}
            onclick={() => openText("reopen", pr)}
          >
            Reopen
          </Button>
        {/if}
        <Button class={tb} variant="outline"
          type="button"
          title="Check out this branch locally"
          disabled={busy}
          onclick={() => void checkout(pr)}
        >
          Checkout
        </Button>
      {/if}
      <Button class={tb} variant="outline" type="button" onclick={() => void openUrl(pr.url)}>
        Open
      </Button>
      {#if busy}
        <span class="busy text-[0.68rem] text-muted-foreground" aria-live="polite">Working…</span>
      {/if}
    </div>
  {/snippet}

  {#snippet checksBlock(pr: PrInfo)}
    {@const checks = forgeStore.checksFor(repoId, pr.number)}
    {@const loading = forgeStore.checksLoadingFor(repoId, pr.number)}
    {@const summary = summarizeChecks(checks ?? [])}
    {@const isWatching = watching.includes(pr.number)}
    <div class="checks mt-2 border-t border-border pt-[0.375rem]">
      <div class="checks-head flex items-center gap-[0.375rem]">
        <span
          class="chip {worstChipClass(summary.worst)} {chipTone[worstChipClass(summary.worst)] ?? "bg-accent text-foreground"} flex-none rounded-full px-2 py-[0.05rem] text-[0.6563rem] font-semibold"
        >
          {checkSummaryLabel(summary)}
        </span>
        <span class="spacer ml-auto"></span>
        <Button
          class="{tb} watching {isWatching ? "border-primary text-primary" : ""}"
          variant="outline"
          type="button"
          title={isWatching
            ? "Stop auto-refreshing checks"
            : "Auto-refresh checks every 10 seconds"}
          onclick={() => toggleWatch(pr.number)}
        >
          {isWatching ? "Watching" : "Watch"}
        </Button>
        <Button class={tb} variant="outline"
          type="button"
          disabled={loading}
          onclick={() => void forgeStore.refreshChecks(repoId, pr.number)}
        >
          {loading ? "Loading…" : "Refresh checks"}
        </Button>
      </div>
      {#if checks === null}
        <p class="muted my-1 text-[0.75rem] text-muted-foreground" aria-live="polite">Loading checks…</p>
      {:else if checks.length === 0}
        <p class="muted my-1 text-[0.75rem] text-muted-foreground">No checks reported for this PR.</p>
      {:else}
        <ul class="check-list m-0 mt-1 flex list-none flex-col gap-[0.125rem] p-0" role="list" aria-label="CI checks">
          {#each checks as check (check.name)}
            <li class="check flex min-w-0 items-center gap-[0.375rem]">
              <span
                class="check-icon {checkStateClass(check.state)} {chipTone[checkStateClass(check.state)] ?? "text-muted-foreground"} w-[1em] flex-none text-center"
                aria-hidden="true"
              >
                {checkIcon(check.state)}
              </span>
              <span class="check-name truncate text-foreground" title={check.name}>{check.name}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/snippet}

  <div class="toolbar sticky top-0 z-[1] flex flex-none flex-wrap items-center gap-[0.375rem] bg-background px-2 py-[0.375rem]">
    <div
      class="tabs inline-flex flex-none gap-[0.125rem] rounded-full border border-border p-[0.1rem]"
      role="tablist"
      aria-label="GitHub panels"
    >
      <button
        class="tab cursor-pointer rounded-full border-none bg-transparent px-[0.6rem] py-[0.12rem] text-[0.72rem] {tab === "prs"
          ? "bg-accent font-semibold text-accent-foreground"
          : "text-muted-foreground"}"
        class:active={tab === "prs"}
        type="button"
        role="tab"
        aria-selected={tab === "prs"}
        onclick={() => selectTab("prs")}
      >
        Pull requests
      </button>
      <button
        class="tab cursor-pointer rounded-full border-none bg-transparent px-[0.6rem] py-[0.12rem] text-[0.72rem] {tab === "actions"
          ? "bg-accent font-semibold text-accent-foreground"
          : "text-muted-foreground"}"
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
      <span class="gh-meta text-[0.6875rem] text-muted-foreground" title="GitHub CLI version">gh {forgeStore.status.version}</span>
    {/if}
    <span class="spacer ml-auto"></span>
    {#if tab === "prs"}
      <select
        class="filter flex-none rounded-sm border border-border bg-background px-[0.25rem] py-[0.1rem] text-[0.72rem] text-foreground"
        aria-label="Pull request state filter"
        value={filter}
        onchange={(event) => setFilter(event.currentTarget.value)}
      >
        <option value="open">Open</option>
        <option value="closed">Closed</option>
        <option value="merged">Merged</option>
        <option value="all">All</option>
      </select>
      <Button class={tb} variant="outline"
        type="button"
        onclick={() => void forgeStore.refreshStatus()}
        disabled={forgeStore.statusLoading}
      >
        {forgeStore.statusLoading ? "Checking…" : "Re-check gh"}
      </Button>
      <Button class={tb} variant="outline" type="button" onclick={refresh} disabled={prsLoading}>
        {prsLoading ? "Loading…" : "Refresh"}
      </Button>
      <Button
        class="act h-auto flex-none rounded-full px-3 py-[0.2rem] text-[0.72rem]"
        type="button"
        disabled={!canCreate}
        onclick={() => (dialogOpen = true)}
      >
        Create PR
      </Button>
    {:else}
      <input
        class="filter branch-filter w-[8.5rem] flex-none rounded-sm border border-border bg-background px-[0.25rem] py-[0.1rem] text-[0.72rem] text-foreground"
        aria-label="Run branch filter"
        placeholder="Branch (any)"
        bind:value={branchFilter}
        onchange={applyRunFilters}
      />
      <select
        class="filter flex-none rounded-sm border border-border bg-background px-[0.25rem] py-[0.1rem] text-[0.72rem] text-foreground"
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
        class="filter workflow-filter max-w-[11rem] flex-none rounded-sm border border-border bg-background px-[0.25rem] py-[0.1rem] text-[0.72rem] text-foreground"
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
      <Button class={tb} variant="outline"
        type="button"
        onclick={() => void forgeStore.refreshStatus()}
        disabled={forgeStore.statusLoading}
      >
        {forgeStore.statusLoading ? "Checking…" : "Re-check gh"}
      </Button>
      <Button class={tb} variant="outline"
        type="button"
        onclick={refreshActions}
        disabled={runsLoading || workflowsLoading}
      >
        {runsLoading ? "Loading…" : "Refresh"}
      </Button>
      <Button
        class="act h-auto flex-none rounded-full px-3 py-[0.2rem] text-[0.72rem]"
        type="button"
        disabled={!context || dispatchBusy}
        onclick={openDispatchDialog}
      >
        Run workflow
      </Button>
    {/if}
  </div>

  {#if banner.kind !== "ready"}
    <div class="banner {banner.kind} mx-2 my-1 flex flex-wrap items-center gap-2 rounded-lg border border-border bg-muted/40 px-[0.625rem] py-2" role="status">
      <p class="banner-text m-0 min-w-[12rem] flex-1 text-muted-foreground">{banner.text}</p>
      {#if banner.kind === "missing"}
        <Button class={tb} variant="outline" type="button" onclick={() => void openUrl(GH_INSTALL_DOCS_URL)}>
          Install guide
        </Button>
      {:else if banner.kind === "unauthed"}
        <code class="cmd rounded-sm bg-accent px-[0.4rem] py-[0.1rem] font-mono text-xs text-foreground">{GH_AUTH_COMMAND}</code>
        <Button class={tb} variant="outline" type="button" onclick={() => void copyAuthCommand()}>Copy</Button>
      {/if}
    </div>
  {:else if contextError}
    <div class="banner missing mx-2 my-1 flex flex-wrap items-center gap-2 rounded-lg border border-border bg-muted/40 px-[0.625rem] py-2" role="status">
      <p class="banner-text m-0 min-w-[12rem] flex-1 text-muted-foreground">
        The gh flow is unavailable for this repository: {contextError}
      </p>
    </div>
  {:else if tab === "actions"}
    {#if context}
      <div class="section actions-section mx-2 mb-3 mt-2 flex flex-col gap-[0.35rem]">
        <div class="actions-summary flex flex-wrap items-baseline gap-2">
          <h3 class="section-title m-0 text-[0.72rem] font-semibold uppercase tracking-[0.04em] text-muted-foreground">Recent runs</h3>
          <span class="muted m-0 text-[0.75rem] text-muted-foreground" aria-live="polite">
            {runs.length} run{runs.length === 1 ? "" : "s"}{#if activeRunCount > 0}
              · {activeRunCount} active{/if}{runsLoading ? " · loading…" : ""}
          </span>
          {#if workflowsLoading}
            <span class="muted m-0 text-[0.75rem] text-muted-foreground">Refreshing workflows…</span>
          {/if}
        </div>
        {#if runsError}
          <p class="error-inline my-[0.15rem] text-xs text-destructive [overflow-wrap:anywhere]" role="alert">{runsError}</p>
        {/if}
        {#if workflowsError}
          <p class="error-inline my-[0.15rem] text-xs text-destructive [overflow-wrap:anywhere]" role="alert">Workflows: {workflowsError}</p>
        {/if}

        {#if runs.length === 0}
          <p class="muted my-1 text-[0.75rem] text-muted-foreground">
            {runsLoading
              ? "Loading workflow runs…"
              : runsError
                ? "Could not load workflow runs."
                : "No workflow runs match these filters."}
          </p>
        {:else}
          <ul class="wf-runs m-0 flex list-none flex-col gap-1 p-0" role="list" aria-label="Workflow runs">
            {#each runs as run (run.database_id)}
              {@const kind = runStateKind(run)}
              {@const active = runIsActive(run)}
              {@const busy = forgeStore.isRunBusy(repoId, run.database_id)}
              {@const age = prAgeLabel(run.created_at, now)}
              {@const duration = runElapsedLabel(run, now)}
              <li class="wf-run min-w-0 overflow-hidden rounded-lg border border-border bg-card">
                <div class="wf-run-line flex min-w-0 flex-wrap items-start gap-[0.375rem] px-2 py-[0.375rem]">
                  <button
                    class="wf-run-main group flex min-w-0 flex-[1_1_12rem] cursor-pointer flex-wrap items-baseline gap-x-[0.375rem] gap-y-[0.15rem] rounded-sm border-none bg-transparent p-0 text-left text-[0.7813rem] text-inherit focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
                    type="button"
                    title="Open run details, jobs and logs"
                    onclick={() => openRun(run)}
                  >
                    <span
                      class="run-chip {runStateClass(kind)} {chipTone[runStateClass(kind)] ?? "bg-accent text-foreground"} mt-[0.05rem] inline-flex flex-none items-center gap-[0.15rem] whitespace-nowrap rounded-full px-[0.45rem] py-[0.05rem] text-[0.6563rem] font-semibold"
                    >
                      <span aria-hidden="true">{runStateIcon(kind)}</span>
                      {runStateLabel(run)}
                    </span>
                    <span class="wf-run-name max-w-[40%] flex-[0_1_auto] truncate font-semibold group-hover:underline" title={run.name}>{run.name}</span>
                    <span class="wf-run-title min-w-0 flex-[1_1_8rem] truncate text-muted-foreground" title={run.display_title}>
                      {run.display_title}
                    </span>
                    <span class="wf-run-meta flex min-w-0 flex-[1_1_100%] flex-wrap items-center gap-[0.375rem] text-[0.68rem] text-muted-foreground">
                      <span class="pr-no flex-none font-mono text-[0.72rem] text-muted-foreground">#{run.number}</span>
                      {#if run.head_branch}
                        <code class="wf-branch max-w-56 truncate font-mono text-[0.68rem] text-muted-foreground" title="Head branch">{run.head_branch}</code>
                      {/if}
                      <span class="wf-event whitespace-nowrap rounded-full border border-border px-[0.35rem] text-[0.6375rem]" title="Trigger event">
                        {workflowEventLabel(run.event)}
                      </span>
                      {#if run.attempt > 1}
                        <span class="wf-attempt whitespace-nowrap tabular-nums">attempt {run.attempt}</span>
                      {/if}
                      {#if age}
                        <span class="age text-[0.6875rem] text-muted-foreground">{age} ago</span>
                      {/if}
                      {#if duration}
                        <span class="wf-duration whitespace-nowrap tabular-nums">{duration}</span>
                      {/if}
                    </span>
                  </button>
                  <span class="wf-run-actions ml-auto flex flex-none flex-wrap items-center justify-end gap-[0.2rem]">
                    {#if active}
                      <Button class="{tb} danger border-current/45 text-[var(--app-deleted)]" variant="outline"
                        type="button"
                        title="Cancel this run"
                        disabled={busy}
                        onclick={() => void cancelRun(run)}
                      >
                        {busy ? "Cancelling…" : "Cancel"}
                      </Button>
                    {/if}
                    {#if runCanRerun(run)}
                      <Button class={tb} variant="outline"
                        type="button"
                        title="Rerun all jobs"
                        disabled={busy}
                        onclick={() => void rerunRun(run, false)}
                      >
                        {busy ? "Working…" : "Rerun"}
                      </Button>
                      {#if runHasFailures(run)}
                        <Button class={tb} variant="outline"
                          type="button"
                          title="Rerun only failed jobs"
                          disabled={busy}
                          onclick={() => void rerunRun(run, true)}
                        >
                          Rerun failed
                        </Button>
                      {/if}
                    {/if}
                    <Button class="{tb} primary border-primary bg-primary font-semibold text-primary-foreground hover:not-disabled:bg-primary hover:not-disabled:brightness-[1.06]" variant="outline"
                      type="button"
                      title="Open run details, jobs and logs"
                      onclick={() => openRun(run)}
                    >
                      Details
                    </Button>
                    <Button class={tb} variant="outline"
                      type="button"
                      title="Open run on GitHub"
                      onclick={() => void openUrl(run.url)}
                    >
                      Open
                    </Button>
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
      <div class="card current mx-2 my-1 rounded-lg border border-border bg-card px-[0.625rem] py-2">
        <div class="card-head flex min-w-0 flex-wrap items-center gap-[0.375rem]">
          <span
            class="chip {prStateClass(branchPr.state)} {chipTone[prStateClass(branchPr.state)] ?? "bg-accent text-foreground"} flex-none rounded-full px-2 py-[0.05rem] text-[0.6563rem] font-semibold"
          >{prStateLabel(branchPr.state)}</span>
          {#if branchPr.is_draft}
            <span class="chip chip-draft flex-none rounded-full border border-border bg-transparent px-2 py-[0.05rem] text-[0.6563rem] font-semibold text-muted-foreground">Draft</span>
          {/if}
          <span class="pr-title min-w-0 truncate font-semibold text-foreground" title={branchPr.title}>{branchPr.title}</span>
          <span class="pr-no flex-none font-mono text-[0.72rem] text-muted-foreground">#{branchPr.number}</span>
        </div>
        <div class="card-sub mt-[0.375rem] flex flex-wrap items-center gap-2">
          <span class="refs font-mono text-[0.72rem] text-muted-foreground">{branchPr.head_ref_name} → {branchPr.base_ref_name}</span>
          {#if branchAge}
            <span class="age text-[0.6875rem] text-muted-foreground" title="Opened"
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
      <div class="card empty mx-2 my-1 flex flex-wrap items-center gap-3 rounded-lg border border-border bg-card px-[0.625rem] py-2">
        <p class="m-0 text-muted-foreground">
          No pull request for branch <code>{context.branch}</code> yet.
        </p>
        <Button
          class="act h-auto flex-none rounded-full px-3 py-[0.2rem] text-[0.72rem]"
          type="button"
          onclick={() => (dialogOpen = true)}
        >Create PR</Button>
      </div>
    {/if}

    <div class="section mx-2 mb-3 mt-2">
      <h3 class="section-title mb-[0.25rem] mt-0 text-[0.72rem] font-semibold uppercase tracking-[0.04em] text-muted-foreground">
        {filter === "open"
          ? "Open pull requests"
          : filter === "all"
            ? "All pull requests"
            : `${filter[0]?.toUpperCase() ?? ""}${filter.slice(1)} pull requests`}
      </h3>
      {#if listedPrs.length === 0}
        <p class="muted my-1 text-[0.75rem] text-muted-foreground">{emptyLabel}</p>
      {:else}
        <ul class="prs m-0 flex list-none flex-col gap-1 p-0" role="list" aria-label="Pull requests">
          {#each listedPrs as pr (pr.number)}
            <li>
              <div class="pr-line flex min-w-0 items-start gap-1">
                <button
                  class="pr-row flex-1 flex w-full min-w-0 cursor-pointer items-center gap-[0.375rem] rounded-lg border border-border bg-card px-2 py-[0.375rem] text-left text-[0.7813rem] text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
                  type="button"
                  aria-expanded={expandedNumber === pr.number}
                  title="Show checks and actions"
                  onclick={() => toggleExpanded(pr.number)}
                >
                  <span class="expand-icon w-[1em] flex-none text-center text-muted-foreground" aria-hidden="true">
                    {expandedNumber === pr.number ? "▾" : "▸"}
                  </span>
                  <span
                    class="chip {prStateClass(pr.state)} {chipTone[prStateClass(pr.state)] ?? "bg-accent text-foreground"} flex-none rounded-full px-2 py-[0.05rem] text-[0.6563rem] font-semibold"
                  >{prStateLabel(pr.state)}</span>
                  {#if pr.is_draft}
                    <span class="chip chip-draft flex-none rounded-full border border-border bg-transparent px-2 py-[0.05rem] text-[0.6563rem] font-semibold text-muted-foreground">Draft</span>
                  {/if}
                  <span class="pr-title min-w-0 truncate font-semibold text-foreground" title={pr.title}>{pr.title}</span>
                  <span class="pr-no flex-none font-mono text-[0.72rem] text-muted-foreground">#{pr.number}</span>
                  <span class="refs ml-auto max-w-[40%] flex-none truncate font-mono text-[0.72rem] text-muted-foreground">{pr.head_ref_name} → {pr.base_ref_name}</span>
                </button>
                <Button class="{tb} mt-[0.375rem]" variant="outline"
                  type="button"
                  title="Open on GitHub"
                  onclick={() => void openUrl(pr.url)}
                >
                  Open
                </Button>
              </div>
              <div class="pr-sub pb-[0.2rem] pl-[1.5rem] pr-2 pt-[0.15rem]">
                {@render prMeta(pr)}
              </div>
              {#if expandedNumber === pr.number}
                <div class="expand mb-1 ml-[1.5rem] mt-1">
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
    <label class="dialog-field mt-2 flex items-center gap-2 text-[0.8125rem]">
      <span>Workflow</span>
      <select
        class="flex-1 rounded-sm border border-border bg-background px-[0.35rem] py-[0.25rem] text-foreground"
        bind:value={dispatchWorkflow}
      >
        {#if dispatchable.length === 0}
          <option value="">No active workflows</option>
        {/if}
        {#each dispatchable as wf (wf.id)}
          <option value={wf.name}>{wf.name} — {wf.path}</option>
        {/each}
      </select>
    </label>
    <label class="dialog-field mt-2 flex items-center gap-2 text-[0.8125rem]">
      <span>Ref</span>
      <input
        class="dialog-text mt-0 w-auto min-w-0 flex-1 rounded-sm border border-border bg-background p-2 text-[0.8125rem] text-foreground"
        aria-label="Branch or tag to run on"
        placeholder={context?.branch ?? "main"}
        bind:value={dispatchRef}
      />
    </label>
    <label class="dialog-field dialog-field-top mt-2 flex items-start gap-2 text-[0.8125rem]">
      <span class="pt-[0.35rem]">Inputs</span>
      <textarea
        class="dialog-text mt-0 w-auto min-w-0 flex-1 resize-y rounded-sm border border-border bg-background p-2 text-[0.8125rem] text-foreground"
        aria-label="Workflow inputs"
        rows="4"
        placeholder={"One key=value per line\nversion=1.2.3\ndry_run=true"}
        bind:value={dispatchInputs}
      ></textarea>
    </label>
    {#if dispatchError}
      <p class="dialog-error m-0 mt-[0.6rem] text-xs text-destructive" role="alert">{dispatchError}</p>
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
    <label class="dialog-field mt-2 flex items-center gap-2 text-[0.8125rem]">
      <span>Method</span>
      <select
        class="flex-1 rounded-sm border border-border bg-background px-[0.35rem] py-[0.25rem] text-foreground"
        bind:value={mergeMethod}
      >
        <option value="squash">Squash and merge</option>
        <option value="merge">Create a merge commit</option>
        <option value="rebase">Rebase and merge</option>
      </select>
    </label>
    <label class="dialog-check mt-2 flex items-center gap-2 text-[0.8125rem] text-muted-foreground">
      <input type="checkbox" bind:checked={deleteBranch} />
      Delete the branch after merging
    </label>
    <label class="dialog-check mt-2 flex items-center gap-2 text-[0.8125rem] text-muted-foreground">
      <input type="checkbox" bind:checked={autoMerge} />
      Enable auto-merge (merge when required checks pass)
    </label>
    {#if mergeError}
      <p class="dialog-error m-0 mt-[0.6rem] text-xs text-destructive" role="alert">{mergeError}</p>
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
      class="dialog-text mt-1 w-full resize-y rounded-sm border border-border bg-background p-2 text-[0.8125rem] text-foreground"
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
      <p class="dialog-error m-0 mt-[0.6rem] text-xs text-destructive" role="alert">{textError}</p>
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
