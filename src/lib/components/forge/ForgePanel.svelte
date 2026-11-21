<script lang="ts">
  /**
   * Forge panel (M6) — gh-assisted GitHub flow.
   *
   * Banner first: gh missing → install link, gh unauthenticated → copyable
   * `gh auth login`, non-GitHub origin → explanation. When ready, the panel
   * shows four tabs in the E2 order `prs | actions | issues |
   * notifications`. "Pull requests": the current branch's PR card (state
   * chips, author/stats/labels, checks with pass/fail/pending icons +
   * refresh/watch, merge/close/reopen/edit, draft↔ready, checkout, comments
   * and reviews) and the filtered repo PR list — plus the E2 PR-detail
   * depth: the PR diff rendered through the shared DiffViewer
   * (`forge_pr_files`, Workstream C files|diff / chip-strip grammar) and
   * read-only review threads (`forge_pr_review_threads`) whose Reply /
   * Approve / Request-changes buttons open the existing write dialogs.
   * "Actions": workflow runs (the runs list the run-view breadcrumb links
   * back to). "Issues" (Phase 6): the filtered issue list with lazily
   * loaded detail (sanitized-markdown body + comments) and close/reopen.
   * "Notifications" (Phase 6): the user's unread threads with mark-read +
   * deep navigation into the PR/issue tabs (commit/release kinds open
   * GitHub). All three list tabs load on activation and refresh on demand —
   * only PR checks auto-poll. "Create PR" opens {@link CreatePrDialog},
   * "New issue" opens {@link CreateIssueDialog}; a successful create
   * refreshes context + list. E2 extras: the merge dialog remembers the
   * last-used method per repo (squash-if-allowed initially) and surfaces
   * gh rejections verbatim with a Retry; the checkout button opens a
   * confirm dialog that offers a named, restorable stash on a dirty
   * worktree (the footer popover's checkout hard-refuses instead).
   *
   * MOUNTING CONTRACT (docs/contracts.md "M6 FE contracts"): this component
   * takes `{ repoId }` and needs a `"forge"` PanelId registry entry to be
   * placeable — layout files were read-only for this lane, the orchestrator
   * adds the id + `PANEL_META` + the RepoView snippet + `TREE_PANELS`.
   */
  import { untrack } from "svelte";
  import { pollScheduler } from "$lib/stores/pollScheduler.svelte";
  import type {
    ForgeIssue,
    ForgeIssueDetail,
    ForgeNotification,
    IssueStateFilter,
    PrInfo,
    PrMergeMethod,
    PrReviewAction,
    PrStateFilter,
    RunStatusFilter,
    WorkflowRun,
    WorkflowRunFilters,
  } from "$lib/ipc/client";
  import { stashPush } from "$lib/ipc/client";
  import { forgeStore, openExternal } from "$lib/stores/forge.svelte";
  import { refreshStatus, tabStore } from "$lib/stores/tabs.svelte";
  import { emitUiEvent } from "$lib/palette/events";
  import { toast } from "$lib/toast";
  import DialogShell from "$lib/components/safety/DialogShell.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import AiMarkdown from "$lib/components/ai/AiMarkdown.svelte";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { createWidthTracker } from "$lib/layout/containerWidth.svelte";
  import { checkoutRefusal, dirtyChangeCount } from "$lib/forge/checkoutGuard";
  import {
    initialMergeMethod,
    loadMergeMethod,
    saveMergeMethod,
  } from "$lib/forge/mergeMethodPref";
  import CreatePrDialog from "./CreatePrDialog.svelte";
  import CreateIssueDialog from "./CreateIssueDialog.svelte";
  import { prFilesToFileDiffs } from "./prDiffModel";
  import { fileChangeKind, fileKindLetter } from "$lib/components/diff/rowModel";
  import type { FileDiff } from "$lib/ipc/types";
  import {
    GH_AUTH_COMMAND,
    GH_INSTALL_DOCS_URL,
    activeWorkflows,
    checkIcon,
    checkStateClass,
    checkSummaryLabel,
    htmlUrlFromApiUrl,
    isIssueOpen,
    isPrOpen,
    issueNumberFromUrl,
    issueStateClass,
    issueStateLabel,
    notificationKindLabel,
    notificationReasonLabel,
    parseWorkflowInputs,
    prAgeLabel,
    prForBranch,
    prMergeBlockedReason,
    prNumberFromNotificationUrl,
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
  import { describeError } from "$lib/errors";

  let {
    repoId,
    requestedTab = null,
  }: {
    repoId: string;
    /**
     * Sub-tab requested by a `focus-panel { panel: "forge", tab }` event
     * (E2: a run view's breadcrumb → runs list); `{tab, n}` so repeating
     * the same tab still re-applies.
     */
    requestedTab?: { tab: string; n: number } | null;
  } = $props();

  // Tailwind replacements for the removed scoped styles: chip/icon tones per
  // state class plus the shared toolbar-button utilities.
  // Geometry/type only — border, background, hover and disabled chrome come
  // from the ui Button primitive this is passed to as `class`.
  const tb =
    "flex-none gap-1 whitespace-nowrap rounded-sm px-2 py-0.5 text-ui-meta leading-[1.4] font-normal text-muted-foreground";
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

  type ForgeTab = "prs" | "issues" | "notifications" | "actions";
  let tab = $state<ForgeTab>("prs");

  let dialogOpen = $state(false);
  let issueDialogOpen = $state(false);
  /** Ensure() runs once per repo (status flips would otherwise re-trigger). */
  let ensuredRepo = $state<string | null>(null);
  /** Currently expanded PR row (its checks/actions render inline). */
  let expandedNumber = $state<number | null>(null);
  /** PRs whose checks auto-refresh every 10 s (user-toggled). */
  let watching = $state<number[]>([]);

  // Issues tab state (list + lazily loaded detail; no auto-polling).
  let issuesEnsuredFor = $state<string | null>(null);
  /** Currently expanded issue row (its body/comments render inline). */
  let expandedIssue = $state<number | null>(null);

  // Notifications tab state (refresh on open / manual; no auto-polling).
  let notificationsEnsuredFor = $state<string | null>(null);

  // PR edit (title/body) dialog.
  let editOpen = $state(false);
  let editTarget = $state<PrInfo | null>(null);
  let editTitle = $state("");
  let editBody = $state("");
  let editError = $state("");

  // Issue close/reopen dialog.
  type IssueTextAction = "close" | "reopen";
  let issueTextOpen = $state(false);
  let issueTextAction = $state<IssueTextAction>("close");
  let issueTextTarget = $state<ForgeIssue | null>(null);
  let issueTextBody = $state("");
  let issueTextError = $state("");

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

  // Checkout confirm dialog (E2 §9.4): on a dirty worktree it offers a
  // NAMED, restorable stash (`stash_push` with the entered message; the
  // Stashes panel lists and restores it) before `pr_checkout` runs.
  let checkoutOpen = $state(false);
  let checkoutTarget = $state<PrInfo | null>(null);
  let checkoutStash = $state(true);
  let checkoutStashName = $state("");
  let checkoutError = $state("");

  // PR-detail depth (E2 §9.4): the pull request diff (`forge_pr_files` →
  // DiffViewer) and the read-only review threads, one PR open at a time.
  let filesNumber = $state<number | null>(null);
  let threadsNumber = $state<number | null>(null);
  /** The rendered PR diff viewer (file chips/rows reveal a file). */
  let prDiffViewer = $state<ReturnType<typeof DiffViewer> | null>(null);
  /** Panel-width tracker — the diff follows the Workstream C grammar. */
  const diffTracker = createWidthTracker();

  $effect(() => {
    if (ensuredRepo === repoId) return;
    ensuredRepo = repoId;
    expandedNumber = null;
    expandedIssue = null;
    watching = [];
    runsEnsuredFor = null;
    issuesEnsuredFor = null;
    notificationsEnsuredFor = null;
    filesNumber = null;
    threadsNumber = null;
    branchFilter = "";
    statusFilter = "";
    workflowFilter = "";
    dispatchOpen = false;
    checkoutOpen = false;
    checkoutTarget = null;
    void forgeStore.ensure(repoId);
    const active = untrack(() => tab);
    if (active === "actions") {
      runsEnsuredFor = repoId;
      void forgeStore.ensureActions(repoId);
    } else if (active === "issues") {
      issuesEnsuredFor = repoId;
      void forgeStore.ensureIssues(repoId);
    } else if (active === "notifications") {
      notificationsEnsuredFor = repoId;
      void forgeStore.ensureNotifications(repoId);
    }
  });

  // Check watcher (E1 migration): one poll-scheduler lane per toggled PR
  // (`forge:panel-checks:<repo>:<n>`, 10 s) instead of a raw component
  // interval — the scheduler adds the hidden-window pause and serializes
  // runs; cleanup on unmount unregisters every lane.
  $effect(() => {
    const numbers = watching;
    const targetRepo = repoId;
    if (numbers.length === 0) return;
    const unsubscribes = numbers.map((number) =>
      pollScheduler.register(`forge:panel-checks:${targetRepo}:${number}`, {
        intervalMs: 10_000,
        work: async () => {
          await forgeStore.refreshChecks(targetRepo, number);
        },
      }),
    );
    return () => {
      for (const unsubscribe of unsubscribes) unsubscribe();
    };
  });

  // A focus-panel sub-tab request (run-view breadcrumb → runs list) opens
  // the requested tab once per event; unknown names are ignored.
  $effect(() => {
    const tabName = requestedTab?.tab;
    if (
      tabName === "prs" ||
      tabName === "actions" ||
      tabName === "issues" ||
      tabName === "notifications"
    ) {
      selectTab(tabName);
    }
  });

  // PR-detail depth loads ride the same lazily-loaded pattern as the issue
  // detail: fetch when a PR's block opens; `untrack` keeps the fetch from
  // depending on the loading state it writes.
  $effect(() => {
    const number = filesNumber;
    const targetRepo = repoId;
    if (number === null) return;
    untrack(() => void forgeStore.loadPrFiles(targetRepo, number));
  });
  $effect(() => {
    const number = threadsNumber;
    const targetRepo = repoId;
    if (number === null) return;
    untrack(() => void forgeStore.loadReviewThreads(targetRepo, number));
  });

  // gh status of the repo's machine (the device for device repos).
  const ghStatus = $derived(forgeStore.statusFor(repoId));
  const ghStatusLoading = $derived(forgeStore.statusLoadingFor(repoId));
  const banner = $derived(statusBanner(ghStatus));
  const context = $derived(forgeStore.contextFor(repoId));
  const contextError = $derived(forgeStore.contextErrorFor(repoId));
  const prs = $derived(forgeStore.prsFor(repoId));
  const prsLoading = $derived(forgeStore.prsLoadingFor(repoId));
  const prsError = $derived(forgeStore.prsErrorFor(repoId));
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
      : prsError
        ? "Could not load pull requests."
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

  // Actions-tab active-runs refresh (E1 migration): one scheduler lane
  // (`forge:panel-runs:<repo>`, 10 s, silent) — the toolbar must not flash
  // "Loading…" on a background tick, hidden windows pause for free.
  $effect(() => {
    if (tab !== "actions" || activeRunCount === 0) return;
    const targetRepo = repoId;
    const filters = currentRunFilters();
    const unsubscribe = pollScheduler.register(`forge:panel-runs:${targetRepo}`, {
      intervalMs: 10_000,
      work: async () => {
        await forgeStore.refreshRuns(targetRepo, filters, true);
      },
    });
    return unsubscribe;
  });

  // Panel lists (E1 §9.3: "panel lists on-open + 60 s"): while the panel is
  // mounted the PR list (and the run list on the Actions tab) re-read on a
  // 60 s scheduler lane; on-open loading stays with the ensure() bundle.
  $effect(() => {
    const targetRepo = repoId;
    const unsubscribe = pollScheduler.register(`forge:panel-lists:${targetRepo}`, {
      intervalMs: 60_000,
      work: async () => {
        const onActions = untrack(() => tab === "actions");
        if (onActions) {
          await Promise.all([
            forgeStore.refreshPrs(targetRepo, untrack(() => forgeStore.filterFor(targetRepo))),
            forgeStore.refreshRuns(
              targetRepo,
              untrack(() => currentRunFilters()),
              true,
            ),
          ]);
        } else {
          await forgeStore.refreshPrs(targetRepo, untrack(() => forgeStore.filterFor(targetRepo)));
        }
      },
    });
    return unsubscribe;
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
  const canCreateIssue = $derived(banner.kind === "ready");
  const mergeBusy = $derived(
    mergeTarget ? forgeStore.isBusy(repoId, mergeTarget.number) : false,
  );
  const textBusy = $derived(
    textTarget ? forgeStore.isBusy(repoId, textTarget.number) : false,
  );

  // Checkout safety (E2 §9.4): the decision reads the tab store's latest
  // RepoStatus — the same payload the STATUS panel renders.
  const tabStatus = $derived(
    tabStore.tabs.find((tab) => tab.id === repoId)?.status ?? null,
  );
  const checkoutRefusalText = $derived(checkoutRefusal(tabStatus));
  const checkoutDirtyCount = $derived(dirtyChangeCount(tabStatus));
  /** Repo root — the stable per-repo key for persisted prefs. */
  const repoRoot = $derived(
    tabStore.tabs.find((tab) => tab.id === repoId)?.root ?? "",
  );
  const checkoutBusy = $derived(
    checkoutTarget ? forgeStore.isBusy(repoId, checkoutTarget.number) : false,
  );

  /** Opens a URL in the system browser (toast on failure). */
  async function openUrl(url: string): Promise<void> {
    const ok = await openExternal(url);
    if (!ok) toast(`Could not open ${url}`, { kind: "error" });
  }

  /** Opens a notification subject URL: REST API URLs convert to html pages. */
  function openSubjectUrl(url: string): void {
    void openUrl(htmlUrlFromApiUrl(url) ?? url);
  }

  /** Html page URL of one of the context repo's issues. */
  function issuePageUrl(number: number): string {
    return `https://github.com/${context?.owner ?? ""}/${context?.repo ?? ""}/issues/${number}`;
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
    return describeError(err);
  }

  /** One-letter change badge for a PR file row/chip (viewer grammar). */
  function fileChangeLetter(file: FileDiff): string {
    return fileKindLetter(fileChangeKind(file));
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
    // Every list tab loads on first activation only (TTL-guarded in the
    // store); refreshes are explicit. Issues and notifications never
    // auto-poll — unlike the watched PR checks and active runs.
    if (next === "actions" && runsEnsuredFor !== repoId) {
      runsEnsuredFor = repoId;
      void forgeStore.ensureActions(repoId);
    } else if (next === "issues" && issuesEnsuredFor !== repoId) {
      issuesEnsuredFor = repoId;
      void forgeStore.ensureIssues(repoId);
    } else if (next === "notifications" && notificationsEnsuredFor !== repoId) {
      notificationsEnsuredFor = repoId;
      void forgeStore.ensureNotifications(repoId);
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
    // E2 §9.4: the last-used method per repo pre-selects; with no memory
    // yet, squash-if-allowed else merge (see `initialMergeMethod`).
    mergeMethod = loadMergeMethod(repoRoot) ?? initialMergeMethod(pr);
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
      // A successful merge is what makes the method worth remembering.
      saveMergeMethod(repoRoot, mergeMethod);
      mergeOpen = false;
      toast(
        autoMerge
          ? `Auto-merge enabled for #${pr.number}`
          : `Merged #${pr.number}`,
        { kind: "success" },
      );
      mergeTarget = null;
    } catch (err) {
      // Branch-protection (and every other gh) rejection surfaces verbatim;
      // the dialog stays open — Retry re-submits the same request.
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

  /** Default stash message for the checkout confirm dialog. */
  function stashNameFor(pr: PrInfo): string {
    return `Before checkout of #${pr.number} (${pr.head_ref_name})`;
  }

  /**
   * Panel checkout (E2 §9.4): opens a confirm dialog that offers a NAMED,
   * restorable stash when the worktree is dirty (the footer popover's
   * checkout hard-refuses instead — see `checkoutRefusal`). An in-progress
   * operation (merge/rebase/…) blocks without a stash offer — there is
   * nothing coherent to stash mid-sequence.
   */
  function requestCheckout(pr: PrInfo): void {
    checkoutTarget = pr;
    checkoutStash = checkoutDirtyCount > 0;
    checkoutStashName = stashNameFor(pr);
    checkoutError = "";
    checkoutOpen = true;
  }

  async function confirmCheckout(): Promise<void> {
    const pr = checkoutTarget;
    if (pr === null) return;
    checkoutError = "";
    try {
      if (checkoutDirtyCount > 0 && checkoutStash) {
        const name = checkoutStashName.trim() || stashNameFor(pr);
        await stashPush(repoId, name);
        toast(`Stashed changes as "${name}"`, { kind: "success" });
      }
      await forgeStore.checkoutPr(repoId, pr.number);
      await refreshStatus(repoId);
      toast(`Checked out #${pr.number} (${pr.head_ref_name})`, {
        kind: "success",
      });
      checkoutOpen = false;
      checkoutTarget = null;
    } catch (err) {
      // gh's refusal (protection rules, conflicts…) surfaces verbatim; the
      // dialog stays open so the stash choice can be adjusted and retried.
      checkoutError = errorText(err);
    }
  }

  // -- PR-detail depth (E2 §9.4) ---------------------------------------------

  function togglePrFiles(number: number): void {
    filesNumber = filesNumber === number ? null : number;
  }

  function toggleThreads(number: number): void {
    threadsNumber = threadsNumber === number ? null : number;
  }

  /** Chip/row click: scroll the PR diff to the file's header row. */
  function revealPrFile(path: string): void {
    prDiffViewer?.revealFile(path);
  }

  // -- PR edit dialog -------------------------------------------------------

  function openEdit(pr: PrInfo): void {
    editTarget = pr;
    editTitle = pr.title;
    editBody = pr.body;
    editError = "";
    editOpen = true;
  }

  const editBusy = $derived(
    editTarget ? forgeStore.isBusy(repoId, editTarget.number) : false,
  );
  const editChanged = $derived(
    !!editTarget &&
      (editTitle.trim() !== editTarget.title || editBody !== editTarget.body),
  );

  async function confirmEdit(): Promise<void> {
    const pr = editTarget;
    if (!pr) return;
    if (!editTitle.trim() && editBody.trim().length === 0) {
      editError = "Nothing to edit — keep a title or a description.";
      return;
    }
    editError = "";
    try {
      await forgeStore.editPr(repoId, pr.number, editTitle.trim(), editBody);
      editOpen = false;
      toast(`Edited #${pr.number}`, { kind: "success" });
      editTarget = null;
    } catch (err) {
      editError = errorText(err);
    }
  }

  // -- issues tab -----------------------------------------------------------

  const issues = $derived(forgeStore.issuesFor(repoId));
  const issuesLoading = $derived(forgeStore.issuesLoadingFor(repoId));
  const issuesError = $derived(forgeStore.issuesErrorFor(repoId));
  const issueFilter = $derived(forgeStore.issueFilterFor(repoId));

  function setIssueFilter(value: string): void {
    expandedIssue = null;
    void forgeStore.refreshIssues(repoId, value as IssueStateFilter);
  }

  /** Expands/collapses one issue row (expanding lazily loads its detail). */
  function toggleIssueExpanded(number: number): void {
    expandedIssue = expandedIssue === number ? null : number;
    if (expandedIssue !== null) void forgeStore.loadIssueDetail(repoId, number);
  }

  function refreshIssues(): void {
    void forgeStore.refreshIssues(repoId);
  }

  function onIssueCreated(): void {
    void forgeStore.refreshIssues(repoId);
  }

  function openIssueText(action: IssueTextAction, issue: ForgeIssue): void {
    issueTextAction = action;
    issueTextTarget = issue;
    issueTextBody = "";
    issueTextError = "";
    issueTextOpen = true;
  }

  const issueTextBusy = $derived(
    issueTextTarget ? forgeStore.isIssueBusy(repoId, issueTextTarget.number) : false,
  );
  const issueTextTitle = $derived.by(() => {
    const n = issueTextTarget?.number ?? 0;
    return issueTextAction === "close" ? `Close #${n}` : `Reopen #${n}`;
  });
  const issueTextConfirm = $derived(
    issueTextAction === "close" ? "Close" : "Reopen",
  );
  const issueTextSuccess = $derived(
    issueTextAction === "close" ? "Closed" : "Reopened",
  );

  async function confirmIssueText(): Promise<void> {
    const issue = issueTextTarget;
    if (!issue) return;
    issueTextError = "";
    try {
      const body = issueTextBody.trim();
      if (issueTextAction === "close") {
        await forgeStore.closeIssue(repoId, issue.number, body || null);
      } else {
        await forgeStore.reopenIssue(repoId, issue.number, body || null);
      }
      issueTextOpen = false;
      toast(`${issueTextSuccess} #${issue.number}`, { kind: "success" });
      issueTextTarget = null;
      // The store reset the detail cache (state changed); an expanded row
      // must not sit on the stale entry.
      if (expandedIssue === issue.number) {
        void forgeStore.loadIssueDetail(repoId, issue.number);
      }
    } catch (err) {
      issueTextError = errorText(err);
    }
  }

  // -- notifications tab ----------------------------------------------------

  const notifications = $derived(forgeStore.notificationsFor(repoId));
  const notificationsLoading = $derived(forgeStore.notificationsLoadingFor(repoId));
  const notificationsError = $derived(forgeStore.notificationsErrorFor(repoId));
  let markAllBusy = $state(false);

  function refreshNotifications(): void {
    void forgeStore.refreshNotifications(repoId);
  }

  async function markRead(item: ForgeNotification): Promise<void> {
    try {
      await forgeStore.markNotificationRead(repoId, item.id);
    } catch (err) {
      toast(errorText(err), { kind: "error" });
    }
  }

  async function markAllRead(): Promise<void> {
    markAllBusy = true;
    try {
      await forgeStore.markAllNotificationsRead(repoId);
    } catch (err) {
      toast(errorText(err), { kind: "error" });
    } finally {
      markAllBusy = false;
    }
  }

  /**
   * Deep navigation for one notification: pull/issue threads switch to the
   * matching tab and expand the target in the refreshed list (falling back
   * to the GitHub page when it is not in the visible filter); commit and
   * other kinds have no panel surface and open GitHub instead.
   */
  async function openNotification(item: ForgeNotification): Promise<void> {
    const htmlUrl = htmlUrlFromApiUrl(item.subject_url) ?? item.subject_url;
    if (item.kind === "pull") {
      const number = prNumberFromNotificationUrl(item.subject_url);
      if (number === null) return openSubjectUrl(item.subject_url);
      selectTab("prs");
      const list = await forgeStore.refreshPrs(repoId);
      if (list.some((pr) => pr.number === number)) {
        expandedNumber = number;
      } else {
        openSubjectUrl(item.subject_url);
      }
      return;
    }
    if (item.kind === "issue") {
      const number = issueNumberFromUrl(item.subject_url);
      if (number === null) return openSubjectUrl(item.subject_url);
      selectTab("issues");
      const list = await forgeStore.refreshIssues(repoId);
      if (list.some((issue) => issue.number === number)) {
        expandedIssue = number;
        void forgeStore.loadIssueDetail(repoId, number);
      } else {
        openSubjectUrl(item.subject_url);
      }
      return;
    }
    openSubjectUrl(item.subject_url);
  }
</script>

<aside
  class="forge-panel flex min-h-0 flex-col overflow-y-auto text-ui-base"
  aria-label="Pull requests"
  bind:this={diffTracker.el}
>
  {#snippet prMeta(pr: PrInfo)}
    {@const review = reviewDecisionLabel(pr.review_decision)}
    <div class="pr-meta mt-ui-3 flex flex-wrap items-center gap-ui-375 text-ui-micro text-muted-foreground">
      {#if pr.author}
        <Hint label="Author">
          {#snippet child({ props })}
            <span class="author flex-none" {...props}>by {pr.author}</span>
          {/snippet}
        </Hint>
      {/if}
      {#if pr.additions || pr.deletions || pr.changed_files}
        <Hint label={`${pr.additions} additions, ${pr.deletions} deletions, ${pr.changed_files} changed files`}>
          {#snippet child({ props })}
            <span
              class="stats inline-flex flex-none items-baseline gap-ui-2 font-mono tabular-nums"
              {...props}
            >
              <span class="add text-[var(--app-added)]">+{pr.additions}</span>
              <span class="del text-[var(--app-deleted)]">−{pr.deletions}</span>
              <span class="files ml-ui-15">{pr.changed_files} file{pr.changed_files === 1 ? "" : "s"}</span>
            </span>
          {/snippet}
        </Hint>
      {/if}
      {#if review}
        <span class="review flex-none rounded-full bg-accent px-2 py-[0.02rem] text-foreground">{review}</span>
      {/if}
      {#if pr.mergeable === "CONFLICTING"}
        <span class="warn flex-none rounded-full bg-destructive/15 px-2 py-[0.02rem] font-semibold text-destructive">Merge conflicts</span>
      {/if}
      {#if pr.labels.length > 0}
        <span class="labels inline-flex min-w-0 flex-wrap gap-ui-2">
          {#each pr.labels as label (label)}
            <span class="label max-w-36 truncate rounded-full border border-border px-2 py-[0.02rem] text-ui-micro">{label}</span>
          {/each}
        </span>
      {/if}
    </div>
  {/snippet}

  {#snippet prBody(pr: PrInfo)}
    {#if pr.body.trim()}
      <details class="pr-body mt-ui-35 text-ui-meta text-muted-foreground">
        <summary class="w-fit cursor-pointer text-ui-micro text-primary">Description</summary>
        <div class="pr-body-text mt-ui-3 max-h-64 overflow-auto whitespace-pre-wrap rounded-r-sm border-l-2 border-border bg-muted/40 px-2 py-ui-4 leading-[1.45] [overflow-wrap:anywhere]">{pr.body}</div>
      </details>
    {/if}
  {/snippet}

  {#snippet actionsBlock(pr: PrInfo)}
    {@const busy = forgeStore.isBusy(repoId, pr.number)}
    {@const blocked = prMergeBlockedReason(pr)}
    <div class="pr-actions mt-ui-45 flex flex-wrap items-center gap-1" aria-label={`Actions for pull request ${pr.number}`}>
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
          onclick={() => requestCheckout(pr)}
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
        <Button class={tb} variant="outline"
          type="button"
          title="Edit title and description"
          disabled={busy}
          onclick={() => openEdit(pr)}
        >
          Edit
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
          onclick={() => requestCheckout(pr)}
        >
          Checkout
        </Button>
      {/if}
      <Button class={tb} variant="outline" type="button" onclick={() => void openUrl(pr.url)}>
        Open
      </Button>
      {#if busy}
        <span class="busy text-ui-micro text-muted-foreground" aria-live="polite">Working…</span>
      {/if}
    </div>
  {/snippet}

  {#snippet checksBlock(pr: PrInfo)}
    {@const checks = forgeStore.checksFor(repoId, pr.number)}
    {@const loading = forgeStore.checksLoadingFor(repoId, pr.number)}
    {@const checksError = forgeStore.checksErrorFor(repoId, pr.number)}
    {@const summary = summarizeChecks(checks ?? [])}
    {@const isWatching = watching.includes(pr.number)}
    <div class="checks mt-2 border-t border-border pt-ui-375">
      <div class="checks-head flex items-center gap-ui-375">
        <span
          class="chip {worstChipClass(summary.worst)} {chipTone[worstChipClass(summary.worst)] ?? "bg-accent text-foreground"} flex-none rounded-full px-2 py-px text-ui-micro font-semibold"
        >
          {checkSummaryLabel(summary)}
        </span>
        <span class="spacer ml-auto"></span>
        <Hint label={isWatching
          ? "Stop auto-refreshing checks"
          : "Auto-refresh checks every 10 seconds"}>
          {#snippet child({ props })}
            <Button
              class="{tb} watching {isWatching ? "border-primary text-primary" : ""}"
              variant="outline"
              {...props}
              type="button"
              onclick={() => toggleWatch(pr.number)}
            >
              {isWatching ? "Watching" : "Watch"}
            </Button>
          {/snippet}
        </Hint>
        <Button class={tb} variant="outline"
          type="button"
          disabled={loading}
          onclick={() => void forgeStore.refreshChecks(repoId, pr.number)}
        >
          {loading ? "Loading…" : "Refresh checks"}
        </Button>
      </div>
      {#if checksError}
        <p class="error my-1 text-ui-meta text-destructive [overflow-wrap:anywhere]" role="alert">Could not load checks: {checksError}</p>
      {:else if checks === null}
        <p class="muted my-1 text-ui-meta text-muted-foreground" aria-live="polite">Loading checks…</p>
      {:else if checks.length === 0}
        <p class="muted my-1 text-ui-meta text-muted-foreground">No checks reported for this PR.</p>
      {:else}
        <ul class="check-list m-0 mt-1 flex list-none flex-col gap-ui-125 p-0" role="list" aria-label="CI checks">
          {#each checks as check (check.name)}
            <li class="check flex min-w-0 items-center gap-ui-375">
              <span
                class="check-icon {checkStateClass(check.state)} {chipTone[checkStateClass(check.state)] ?? "text-muted-foreground"} w-[1em] flex-none text-center"
                aria-hidden="true"
              >
                {checkIcon(check.state)}
              </span>
              <Hint label={check.name}>
                {#snippet child({ props })}
                  <span class="check-name truncate text-foreground" {...props}>{check.name}</span>
                {/snippet}
              </Hint>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/snippet}

  {#snippet prFilesBlock(pr: PrInfo)}
    {@const files = forgeStore.prFilesFor(repoId, pr.number)}
    {@const filesLoading = forgeStore.prFilesLoadingFor(repoId, pr.number)}
    {@const filesError = forgeStore.prFilesErrorFor(repoId, pr.number)}
    {@const viewerFiles = files === null ? [] : prFilesToFileDiffs(files)}
    <div class="pr-files mt-2 border-t border-border pt-ui-375">
      <div class="files-head flex items-center gap-ui-375">
        <span class="files-label flex-none text-ui-meta font-semibold text-muted-foreground">Files changed</span>
        {#if pr.changed_files > 0}
          <span class="file-count flex-none text-ui-micro text-muted-foreground">
            {pr.changed_files} file{pr.changed_files === 1 ? "" : "s"} · <span class="add text-[var(--app-added)]">+{pr.additions}</span> <span class="del text-[var(--app-deleted)]">−{pr.deletions}</span>
          </span>
        {/if}
        <span class="spacer ml-auto"></span>
        <Button class={tb} variant="outline"
          type="button"
          title={filesNumber === pr.number
            ? "Hide the pull request diff"
            : "Show the pull request diff"}
          onclick={() => togglePrFiles(pr.number)}
        >
          {filesNumber === pr.number ? "Hide diff" : "View diff"}
        </Button>
      </div>
      {#if filesNumber === pr.number}
        {#if filesError}
          <p class="error my-1 text-ui-meta text-destructive [overflow-wrap:anywhere]" role="alert">Could not load files: {filesError}</p>
          <Button class={tb} variant="outline" type="button" onclick={() => void forgeStore.loadPrFiles(repoId, pr.number)}>
            Retry
          </Button>
        {:else if files === null}
          <p class="muted my-1 text-ui-meta text-muted-foreground" aria-live="polite">
            {filesLoading ? "Loading files…" : ""}
          </p>
        {:else if viewerFiles.length === 0}
          <p class="muted my-1 text-ui-meta text-muted-foreground">This pull request has no file changes.</p>
        {:else if diffTracker.narrow}
          <!-- Narrow container (Workstream C grammar): horizontal chip
               strip above a unified diff (DiffViewer auto-unifies via
               containerNarrow). -->
          <div class="pr-diff mt-1 flex h-96 min-h-0 flex-col overflow-hidden rounded-sm border border-border bg-background">
            <ul class="filelist flex flex-none items-center gap-1 overflow-x-auto border-b border-border bg-muted/40 py-1 pr-2 pl-2 text-ui-meta" aria-label="Pull request changed files">
              {#each viewerFiles as file (file.path)}
                <li class="fchip flex-none">
                  <button
                    class="flex max-w-48 cursor-pointer items-center gap-1.5 rounded-full border border-border bg-card px-2 py-0.5 text-ui-meta hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                    type="button"
                    title={file.path}
                    onclick={() => revealPrFile(file.path)}
                  >
                    <span class="fstat flex-none font-mono text-ui-micro font-bold {file.additions > 0 && file.deletions === 0
                      ? "text-[var(--app-added)]"
                      : file.deletions > 0 && file.additions === 0
                        ? "text-[var(--app-deleted)]"
                        : "text-[var(--app-modified)]"}">{fileChangeLetter(file)}</span>
                    <span class="min-w-0 truncate font-mono text-foreground">{file.path}</span>
                  </button>
                </li>
              {/each}
            </ul>
            <div class="min-h-0 flex-1">
              <DiffViewer
                bind:this={prDiffViewer}
                files={viewerFiles}
                containerNarrow={diffTracker.narrow}
              />
            </div>
          </div>
        {:else}
          <!-- Wide container: changed-files column | diff (Workstream C
               grammar; the split is a plain grid — no user resize here). -->
          <div class="pr-diff mt-1 grid h-96 min-h-0 grid-cols-[minmax(10rem,14rem)_minmax(0,1fr)] overflow-hidden rounded-sm border border-border bg-background">
            <ul class="filelist m-0 flex min-h-0 list-none flex-col gap-px overflow-y-auto py-1 text-ui-meta" aria-label="Pull request changed files">
              {#each viewerFiles as file (file.path)}
                <li>
                  <button
                    class="flex w-full cursor-pointer items-center gap-2 rounded-none border-none bg-transparent px-2 py-0.5 text-left hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                    type="button"
                    title={file.path}
                    onclick={() => revealPrFile(file.path)}
                  >
                    <span class="fstat flex-none font-mono text-ui-micro font-bold {file.additions > 0 && file.deletions === 0
                      ? "text-[var(--app-added)]"
                      : file.deletions > 0 && file.additions === 0
                        ? "text-[var(--app-deleted)]"
                        : "text-[var(--app-modified)]"}">{fileChangeLetter(file)}</span>
                    <Hint label={file.path}>
                      {#snippet child({ props })}
                        <span class="fpath min-w-0 flex-1 truncate font-mono text-foreground" {...props}>{file.path}</span>
                      {/snippet}
                    </Hint>
                    <span class="fstat-counts flex-none font-mono text-ui-micro text-muted-foreground">+{file.additions} −{file.deletions}</span>
                  </button>
                </li>
              {/each}
            </ul>
            <div class="min-h-0 border-l border-border">
              <DiffViewer
                bind:this={prDiffViewer}
                files={viewerFiles}
                containerNarrow={diffTracker.narrow}
              />
            </div>
          </div>
        {/if}
      {/if}
    </div>
  {/snippet}

  {#snippet prThreadsBlock(pr: PrInfo)}
    {@const threads = forgeStore.reviewThreadsFor(repoId, pr.number)}
    {@const threadsLoading = forgeStore.reviewThreadsLoadingFor(repoId, pr.number)}
    {@const threadsError = forgeStore.reviewThreadsErrorFor(repoId, pr.number)}
    <div class="pr-threads mt-2 border-t border-border pt-ui-375">
      <div class="threads-head flex items-center gap-ui-375">
        <span class="threads-label flex-none text-ui-meta font-semibold text-muted-foreground">Review threads</span>
        {#if threads !== null && threads.length > 0}
          <span class="thread-count flex-none text-ui-micro text-muted-foreground">{threads.length}</span>
        {/if}
        <span class="spacer ml-auto"></span>
        <Button class={tb} variant="outline"
          type="button"
          title={threadsNumber === pr.number
            ? "Hide the review threads"
            : "Show the review threads"}
          onclick={() => toggleThreads(pr.number)}
        >
          {threadsNumber === pr.number ? "Hide threads" : "Show threads"}
        </Button>
      </div>
      {#if threadsNumber === pr.number}
        {#if threadsError}
          <p class="error my-1 text-ui-meta text-destructive [overflow-wrap:anywhere]" role="alert">Could not load review threads: {threadsError}</p>
          <Button class={tb} variant="outline" type="button" onclick={() => void forgeStore.loadReviewThreads(repoId, pr.number)}>
            Retry
          </Button>
        {:else if threads === null}
          <p class="muted my-1 text-ui-meta text-muted-foreground" aria-live="polite">
            {threadsLoading ? "Loading review threads…" : ""}
          </p>
        {:else if threads.length === 0}
          <p class="muted my-1 text-ui-meta text-muted-foreground">No review comments on this pull request yet.</p>
        {:else}
          <!-- Read-only thread display (E2): state rides `outdated`, authors
               and existing replies render inline; the Reply button opens the
               existing comment dialog (`pr_comment`). -->
          <ul class="thread-list m-0 mt-1 flex list-none flex-col gap-ui-35 p-0" role="list" aria-label="Review threads">
            {#each threads as thread (thread.id)}
              <li class="thread rounded-lg border border-border bg-card px-2 py-ui-35">
                <div class="thread-head flex flex-wrap items-baseline gap-ui-375 text-ui-micro text-muted-foreground">
                  <Hint label={thread.line !== null ? `${thread.path}:${thread.line}` : thread.path}>
                    {#snippet child({ props })}
                      <span class="thread-path min-w-0 max-w-48 truncate font-mono text-foreground" {...props}>
                        {thread.path}{thread.line !== null ? `:${thread.line}` : ""}
                      </span>
                    {/snippet}
                  </Hint>
                  {#if thread.outdated}
                    <span class="outdated flex-none rounded-full bg-accent px-2 py-px">Outdated</span>
                  {/if}
                  <span class="author flex-none font-semibold text-foreground">{thread.author || "ghost"}</span>
                  {#if prAgeLabel(thread.created_at)}
                    <span class="age flex-none">commented {prAgeLabel(thread.created_at)} ago</span>
                  {/if}
                  <span class="spacer ml-auto"></span>
                  <Button class={tb} variant="outline"
                    type="button"
                    title="Add a comment on this pull request"
                    onclick={() => openText("comment", pr)}
                  >
                    Reply
                  </Button>
                </div>
                <div class="thread-body mt-1 text-ui-meta text-muted-foreground">
                  <AiMarkdown text={thread.body} />
                </div>
                {#if thread.replies.length > 0}
                  <ul class="replies m-0 mt-1 flex list-none flex-col gap-ui-35 border-l-2 border-border pl-2 pt-1" role="list" aria-label="Replies">
                    {#each thread.replies as reply (reply.id)}
                      <li class="reply">
                        <div class="reply-head flex flex-wrap items-baseline gap-ui-375 text-ui-micro text-muted-foreground">
                          <span class="author flex-none font-semibold text-foreground">{reply.author || "ghost"}</span>
                          {#if prAgeLabel(reply.created_at)}
                            <span class="age flex-none">replied {prAgeLabel(reply.created_at)} ago</span>
                          {/if}
                        </div>
                        <div class="reply-body mt-ui-15 text-ui-meta text-muted-foreground">
                          <AiMarkdown text={reply.body} />
                        </div>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </li>
            {/each}
          </ul>
          <p class="thread-note my-1 text-ui-micro text-muted-foreground">
            Read-only here — Reply posts a pull-request comment.
          </p>
        {/if}
      {/if}
    </div>
  {/snippet}

  <div class="toolbar sticky top-0 z-1 flex flex-none flex-wrap items-center gap-ui-375 bg-background px-2 py-ui-375">
    <div
      class="tabs inline-flex flex-none gap-ui-125 rounded-full border border-border p-ui-1"
      role="tablist"
      aria-label="GitHub panels"
    >
      <button
        class="tab cursor-pointer rounded-full border-none bg-transparent px-2.5 py-0.5 text-ui-meta {tab === "prs"
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
      <!-- E2 §9.4 tab order: prs | actions | issues | notifications. -->
      <button
        class="tab cursor-pointer rounded-full border-none bg-transparent px-2.5 py-0.5 text-ui-meta {tab === "actions"
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
      <button
        class="tab cursor-pointer rounded-full border-none bg-transparent px-2.5 py-0.5 text-ui-meta {tab === "issues"
          ? "bg-accent font-semibold text-accent-foreground"
          : "text-muted-foreground"}"
        class:active={tab === "issues"}
        type="button"
        role="tab"
        aria-selected={tab === "issues"}
        onclick={() => selectTab("issues")}
      >
        Issues
      </button>
      <button
        class="tab cursor-pointer rounded-full border-none bg-transparent px-2.5 py-0.5 text-ui-meta {tab === "notifications"
          ? "bg-accent font-semibold text-accent-foreground"
          : "text-muted-foreground"}"
        class:active={tab === "notifications"}
        type="button"
        role="tab"
        aria-selected={tab === "notifications"}
        onclick={() => selectTab("notifications")}
      >
        Notifications
      </button>
    </div>
    {#if ghStatus?.available}
      <Hint label={ghStatus.authed ? "GitHub CLI version" : "GitHub CLI version (not authenticated)"}>
        {#snippet child({ props })}
          <span class="gh-meta text-ui-micro text-muted-foreground" {...props}>gh {ghStatus.version}</span>
        {/snippet}
      </Hint>
    {/if}
    <span class="spacer ml-auto"></span>
    {#if tab === "prs"}
      <select
        class="filter flex-none rounded-sm border border-border bg-background px-1 py-px text-ui-meta text-foreground"
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
        onclick={() => void forgeStore.refreshStatus(repoId)}
        disabled={ghStatusLoading}
      >
        {ghStatusLoading ? "Checking…" : "Re-check gh"}
      </Button>
      <Button class={tb} variant="outline" type="button" onclick={refresh} disabled={prsLoading}>
        {prsLoading ? "Loading…" : "Refresh"}
      </Button>
      <Button
        class="act h-auto flex-none rounded-full px-3 py-0.5 text-ui-meta"
        type="button"
        disabled={!canCreate}
        onclick={() => (dialogOpen = true)}
      >
        Create PR
      </Button>
    {:else if tab === "issues"}
      <select
        class="filter flex-none rounded-sm border border-border bg-background px-1 py-px text-ui-meta text-foreground"
        aria-label="Issue state filter"
        value={issueFilter}
        onchange={(event) => setIssueFilter(event.currentTarget.value)}
      >
        <option value="open">Open</option>
        <option value="closed">Closed</option>
        <option value="all">All</option>
      </select>
      <Button class={tb} variant="outline"
        type="button"
        onclick={() => void forgeStore.refreshStatus(repoId)}
        disabled={ghStatusLoading}
      >
        {ghStatusLoading ? "Checking…" : "Re-check gh"}
      </Button>
      <Button class={tb} variant="outline" type="button" onclick={refreshIssues} disabled={issuesLoading}>
        {issuesLoading ? "Loading…" : "Refresh"}
      </Button>
      <Button
        class="act h-auto flex-none rounded-full px-3 py-0.5 text-ui-meta"
        type="button"
        disabled={!canCreateIssue}
        onclick={() => (issueDialogOpen = true)}
      >
        New issue
      </Button>
    {:else if tab === "notifications"}
      <Button class={tb} variant="outline"
        type="button"
        onclick={() => void forgeStore.refreshStatus(repoId)}
        disabled={ghStatusLoading}
      >
        {ghStatusLoading ? "Checking…" : "Re-check gh"}
      </Button>
      <Button class={tb} variant="outline" type="button" onclick={refreshNotifications} disabled={notificationsLoading}>
        {notificationsLoading ? "Loading…" : "Refresh"}
      </Button>
      <Button
        class="act h-auto flex-none rounded-full px-3 py-0.5 text-ui-meta"
        type="button"
        disabled={markAllBusy || notifications.length === 0}
        onclick={() => void markAllRead()}
      >
        Mark all read
      </Button>
    {:else}
      <input
        class="filter branch-filter w-[8.5rem] flex-none rounded-sm border border-border bg-background px-1 py-px text-ui-meta text-foreground"
        aria-label="Run branch filter"
        placeholder="Branch (any)"
        bind:value={branchFilter}
        onchange={applyRunFilters}
      />
      <select
        class="filter flex-none rounded-sm border border-border bg-background px-1 py-px text-ui-meta text-foreground"
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
        class="filter workflow-filter max-w-[11rem] flex-none rounded-sm border border-border bg-background px-1 py-px text-ui-meta text-foreground"
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
        onclick={() => void forgeStore.refreshStatus(repoId)}
        disabled={ghStatusLoading}
      >
        {ghStatusLoading ? "Checking…" : "Re-check gh"}
      </Button>
      <Button class={tb} variant="outline"
        type="button"
        onclick={refreshActions}
        disabled={runsLoading || workflowsLoading}
      >
        {runsLoading ? "Loading…" : "Refresh"}
      </Button>
      <Button
        class="act h-auto flex-none rounded-full px-3 py-0.5 text-ui-meta"
        type="button"
        disabled={!context || dispatchBusy}
        onclick={openDispatchDialog}
      >
        Run workflow
      </Button>
    {/if}
  </div>

  {#if banner.kind !== "ready"}
    <div class="banner {banner.kind} mx-2 my-1 flex flex-wrap items-center gap-2 rounded-lg border border-border bg-muted/40 px-2.5 py-2" role="status">
      <p class="banner-text m-0 min-w-[12rem] flex-1 text-muted-foreground">{banner.text}</p>
      {#if banner.kind === "missing"}
        <Button class={tb} variant="outline" type="button" onclick={() => void openUrl(GH_INSTALL_DOCS_URL)}>
          Install guide
        </Button>
      {:else if banner.kind === "unauthed"}
        <code class="cmd rounded-sm bg-accent px-2 py-px font-mono text-xs text-foreground">{GH_AUTH_COMMAND}</code>
        <Button class={tb} variant="outline" type="button" onclick={() => void copyAuthCommand()}>Copy</Button>
      {/if}
    </div>
  {:else if contextError}
    <div class="banner missing mx-2 my-1 flex flex-wrap items-center gap-2 rounded-lg border border-border bg-muted/40 px-2.5 py-2" role="status">
      <p class="banner-text m-0 min-w-[12rem] flex-1 text-muted-foreground">
        The gh flow is unavailable for this repository: {contextError}
      </p>
    </div>
  {:else if tab === "actions"}
    {#if context}
      <div class="section actions-section mx-2 mb-3 mt-2 flex flex-col gap-ui-35">
        <div class="actions-summary flex flex-wrap items-baseline gap-2">
          <h3 class="section-title m-0 text-ui-meta font-semibold uppercase tracking-[0.04em] text-muted-foreground">Recent runs</h3>
          <span class="muted m-0 text-ui-meta text-muted-foreground" aria-live="polite">
            {runs.length} run{runs.length === 1 ? "" : "s"}{#if activeRunCount > 0}
              · {activeRunCount} active{/if}{runsLoading ? " · loading…" : ""}
          </span>
          {#if workflowsLoading}
            <span class="muted m-0 text-ui-meta text-muted-foreground">Refreshing workflows…</span>
          {/if}
        </div>
        {#if runsError}
          <p class="error-inline my-ui-15 text-xs text-destructive [overflow-wrap:anywhere]" role="alert">{runsError}</p>
        {/if}
        {#if workflowsError}
          <p class="error-inline my-ui-15 text-xs text-destructive [overflow-wrap:anywhere]" role="alert">Workflows: {workflowsError}</p>
        {/if}

        {#if runs.length === 0}
          <p class="muted my-1 text-ui-meta text-muted-foreground">
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
                <div class="wf-run-line flex min-w-0 flex-wrap items-start gap-ui-375 px-2 py-ui-375">
                  <Hint label="Open run details, jobs and logs">
                    {#snippet child({ props })}
                      <button
                        class="wf-run-main group flex min-w-0 flex-[1_1_12rem] cursor-pointer flex-wrap items-baseline gap-x-[0.375rem] gap-y-[0.15rem] rounded-sm border-none bg-transparent p-0 text-left text-ui-meta text-inherit focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
                        {...props}
                        type="button"
                        onclick={() => openRun(run)}
                      >
                        <span
                          class="run-chip {runStateClass(kind)} {chipTone[runStateClass(kind)] ?? "bg-accent text-foreground"} mt-[0.05rem] inline-flex flex-none items-center gap-ui-15 whitespace-nowrap rounded-full px-2 py-px text-ui-micro font-semibold"
                        >
                          <span aria-hidden="true">{runStateIcon(kind)}</span>
                          {runStateLabel(run)}
                        </span>
                        <Hint label={run.name}>
                          {#snippet child({ props })}
                            <span class="wf-run-name max-w-[40%] flex-[0_1_auto] truncate font-semibold group-hover:underline" {...props}>{run.name}</span>
                          {/snippet}
                        </Hint>
                        <Hint label={run.display_title}>
                          {#snippet child({ props })}
                            <span class="wf-run-title min-w-0 flex-[1_1_8rem] truncate text-muted-foreground" {...props}>
                              {run.display_title}
                            </span>
                          {/snippet}
                        </Hint>
                        <span class="wf-run-meta flex min-w-0 flex-[1_1_100%] flex-wrap items-center gap-ui-375 text-ui-micro text-muted-foreground">
                          <span class="pr-no flex-none font-mono text-ui-meta text-muted-foreground">#{run.number}</span>
                          {#if run.head_branch}
                            <Hint label="Head branch">
                              {#snippet child({ props })}
                                <code class="wf-branch max-w-56 truncate font-mono text-ui-micro text-muted-foreground" {...props}>{run.head_branch}</code>
                              {/snippet}
                            </Hint>
                          {/if}
                          <Hint label="Trigger event">
                            {#snippet child({ props })}
                              <span class="wf-event whitespace-nowrap rounded-full border border-border px-ui-35 text-ui-micro" {...props}>
                                {workflowEventLabel(run.event)}
                              </span>
                            {/snippet}
                          </Hint>
                          {#if run.attempt > 1}
                            <span class="wf-attempt whitespace-nowrap tabular-nums">attempt {run.attempt}</span>
                          {/if}
                          {#if age}
                            <span class="age text-ui-micro text-muted-foreground">{age} ago</span>
                          {/if}
                          {#if duration}
                            <span class="wf-duration whitespace-nowrap tabular-nums">{duration}</span>
                          {/if}
                        </span>
                      </button>
                    {/snippet}
                  </Hint>
                  <span class="wf-run-actions ml-auto flex flex-none flex-wrap items-center justify-end gap-ui-2">
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
                    <Hint label="Open run details, jobs and logs">
                      {#snippet child({ props })}
                        <Button class="{tb} primary border-primary bg-primary font-semibold text-primary-foreground hover:not-disabled:bg-primary hover:not-disabled:brightness-[1.06]" variant="outline"
                          {...props}
                          type="button"
                          onclick={() => openRun(run)}
                        >
                          Details
                        </Button>
                      {/snippet}
                    </Hint>
                    <Hint label="Open run on GitHub">
                      {#snippet child({ props })}
                        <Button class={tb} variant="outline"
                          {...props}
                          type="button"
                          onclick={() => void openUrl(run.url)}
                        >
                          Open
                        </Button>
                      {/snippet}
                    </Hint>
                  </span>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  {:else if tab === "issues"}
    {#if context}
      <div class="section issues-section mx-2 mb-3 mt-2">
        <h3 class="section-title mb-1 mt-0 text-ui-meta font-semibold uppercase tracking-[0.04em] text-muted-foreground">
          {issueFilter === "open"
            ? "Open issues"
            : issueFilter === "all"
              ? "All issues"
              : "Closed issues"}
        </h3>
        {#if issuesError}
          <p class="error-inline my-ui-15 text-xs text-destructive [overflow-wrap:anywhere]" role="alert">{issuesError}</p>
        {/if}
        {#if issues.length === 0}
          <p class="muted my-1 text-ui-meta text-muted-foreground">
            {issuesLoading
              ? "Loading issues…"
              : issuesError
                ? "Could not load issues."
                : "No issues match this filter."}
          </p>
        {:else}
          <ul class="issues m-0 flex list-none flex-col gap-1 p-0" role="list" aria-label="Issues">
            {#each issues as issue (issue.number)}
              <li>
                <div class="issue-line flex min-w-0 items-start gap-1">
                  <Hint label="Show body and comments">
                    {#snippet child({ props })}
                      <button
                        class="issue-row flex-1 flex w-full min-w-0 cursor-pointer items-center gap-ui-375 rounded-lg border border-border bg-card px-2 py-ui-375 text-left text-ui-meta text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
                        {...props}
                        type="button"
                        aria-expanded={expandedIssue === issue.number}
                        onclick={() => toggleIssueExpanded(issue.number)}
                      >
                        <span class="expand-icon w-[1em] flex-none text-center text-muted-foreground" aria-hidden="true">
                          {expandedIssue === issue.number ? "▾" : "▸"}
                        </span>
                        <span
                          class="chip {issueStateClass(issue.state)} {chipTone[issueStateClass(issue.state)] ?? "bg-accent text-foreground"} flex-none rounded-full px-2 py-px text-ui-micro font-semibold"
                        >{issueStateLabel(issue.state)}</span>
                        <Hint label={issue.title}>
                          {#snippet child({ props })}
                            <span class="issue-title min-w-0 truncate font-semibold text-foreground" {...props}>{issue.title}</span>
                          {/snippet}
                        </Hint>
                        <span class="pr-no flex-none font-mono text-ui-meta text-muted-foreground">#{issue.number}</span>
                        {#if issue.labels.length > 0}
                          <span class="labels inline-flex min-w-0 max-w-[35%] flex-none flex-wrap gap-ui-2">
                            {#each issue.labels as label (label)}
                              <span class="label max-w-36 truncate rounded-full border border-border px-2 py-[0.02rem] text-ui-micro">{label}</span>
                            {/each}
                          </span>
                        {/if}
                        {#if issue.author}
                          <span class="issue-author ml-auto flex-none text-ui-micro text-muted-foreground">by {issue.author}</span>
                        {/if}
                        {#if prAgeLabel(issue.updated_at)}
                          <span class="age flex-none text-ui-micro text-muted-foreground">updated {prAgeLabel(issue.updated_at)} ago</span>
                        {/if}
                      </button>
                    {/snippet}
                  </Hint>
                  <Hint label="Open on GitHub">
                    {#snippet child({ props })}
                      <Button class="{tb} mt-ui-375" variant="outline"
                        {...props}
                        type="button"
                        onclick={() => void openUrl(issuePageUrl(issue.number))}
                      >
                        Open
                      </Button>
                    {/snippet}
                  </Hint>
                </div>
                {#if expandedIssue === issue.number}
                  {@const detail = forgeStore.issueDetailFor(repoId, issue.number)}
                  {@const detailLoading = forgeStore.issueDetailLoadingFor(repoId, issue.number)}
                  {@const detailError = forgeStore.issueDetailErrorFor(repoId, issue.number)}
                  {@const issueBusy = forgeStore.isIssueBusy(repoId, issue.number)}
                  <div class="expand mb-1 ml-[1.5rem] mt-1">
                    {#if detailError}
                      <p class="error my-1 text-ui-meta text-destructive [overflow-wrap:anywhere]" role="alert">Could not load issue: {detailError}</p>
                    {:else if detail === null}
                      <p class="muted my-1 text-ui-meta text-muted-foreground" aria-live="polite">
                        {detailLoading ? "Loading issue…" : ""}
                      </p>
                    {:else}
                      {#if detail.body.trim()}
                        <div class="issue-body mt-1 max-h-72 overflow-auto rounded-r-sm border-l-2 border-border bg-muted/40 px-2 py-ui-4 text-ui-meta text-muted-foreground">
                          <AiMarkdown text={detail.body} />
                        </div>
                      {/if}
                      <div class="issue-actions mt-ui-3 flex flex-wrap items-center gap-1" aria-label={`Actions for issue ${issue.number}`}>
                        {#if isIssueOpen(detail.state)}
                          <Button class="{tb} danger border-current/45 text-[var(--app-deleted)]" variant="outline"
                            type="button"
                            title="Close this issue"
                            disabled={issueBusy}
                            onclick={() => openIssueText("close", detail)}
                          >
                            Close
                          </Button>
                        {:else}
                          <Button class={tb} variant="outline"
                            type="button"
                            title="Reopen this issue"
                            disabled={issueBusy}
                            onclick={() => openIssueText("reopen", detail)}
                          >
                            Reopen
                          </Button>
                        {/if}
                        <Button class={tb} variant="outline" type="button" onclick={() => void openUrl(detail.url || issuePageUrl(issue.number))}>
                          Open
                        </Button>
                        {#if issueBusy}
                          <span class="busy text-ui-micro text-muted-foreground" aria-live="polite">Working…</span>
                        {/if}
                      </div>
                      {#if detail.comments.length > 0}
                        <div class="comments mt-2 border-t border-border pt-ui-375">
                          <h4 class="m-0 text-ui-meta font-semibold text-muted-foreground">
                            {detail.comments.length} comment{detail.comments.length === 1 ? "" : "s"}
                          </h4>
                          <ul class="comment-list m-0 mt-1 flex list-none flex-col gap-ui-35 p-0" role="list" aria-label="Issue comments">
                            {#each detail.comments as comment, idx (idx)}
                              <li class="comment rounded-lg border border-border bg-card px-2 py-ui-35">
                                <div class="comment-head flex flex-wrap items-baseline gap-ui-375 text-ui-micro text-muted-foreground">
                                  <span class="author font-semibold text-foreground">{comment.author || "ghost"}</span>
                                  {#if prAgeLabel(comment.created_at)}
                                    <span class="age">commented {prAgeLabel(comment.created_at)} ago</span>
                                  {/if}
                                </div>
                                <div class="comment-body mt-1 text-ui-meta text-muted-foreground">
                                  <AiMarkdown text={comment.body} />
                                </div>
                              </li>
                            {/each}
                          </ul>
                        </div>
                      {/if}
                    {/if}
                  </div>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  {:else if tab === "notifications"}
    {#if context}
      <div class="section notifications-section mx-2 mb-3 mt-2">
        <h3 class="section-title mb-1 mt-0 text-ui-meta font-semibold uppercase tracking-[0.04em] text-muted-foreground">
          Unread notifications
          <span class="ml-1 normal-case tracking-normal text-ui-micro text-muted-foreground">across your repositories</span>
        </h3>
        {#if notificationsError}
          <p class="error-inline my-ui-15 text-xs text-destructive [overflow-wrap:anywhere]" role="alert">{notificationsError}</p>
        {/if}
        {#if notifications.length === 0}
          <p class="muted my-1 text-ui-meta text-muted-foreground">
            {notificationsLoading
              ? "Loading notifications…"
              : notificationsError
                ? "Could not load notifications."
                : "No unread notifications — you're all caught up."}
          </p>
        {:else}
          <ul class="notifications m-0 flex list-none flex-col gap-1 p-0" role="list" aria-label="Notifications">
            {#each notifications as item (item.id)}
              <li class="notification min-w-0 overflow-hidden rounded-lg border border-border bg-card">
                <div class="notification-line flex min-w-0 flex-wrap items-start gap-ui-375 px-2 py-ui-375">
                  <Hint label={notificationKindLabel(item.kind)}>
                    {#snippet child({ props })}
                      <button
                        class="notif-main group flex min-w-0 flex-[1_1_14rem] cursor-pointer flex-wrap items-baseline gap-x-ui-35 gap-y-ui-15 rounded-sm border-none bg-transparent p-0 text-left text-ui-meta text-inherit focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
                        {...props}
                        type="button"
                        onclick={() => void openNotification(item)}
                      >
                        <span class="notif-kind flex-none rounded-full border border-border px-2 py-px text-ui-micro text-muted-foreground">
                          {notificationKindLabel(item.kind)}
                        </span>
                        <Hint label={item.subject_title}>
                          {#snippet child({ props })}
                            <span class="notif-title min-w-0 flex-[1_1_auto] truncate font-semibold text-foreground group-hover:underline" {...props}>{item.subject_title}</span>
                          {/snippet}
                        </Hint>
                        <span class="notif-meta flex min-w-0 flex-[1_1_100%] flex-wrap items-center gap-ui-375 text-ui-micro text-muted-foreground">
                          <span class="repo flex-none">{item.repository}</span>
                          {#if item.reason}
                            <span class="reason flex-none rounded-full bg-accent px-2 py-px text-foreground">{notificationReasonLabel(item.reason)}</span>
                          {/if}
                          {#if prAgeLabel(item.updated_at)}
                            <span class="age text-ui-micro text-muted-foreground">{prAgeLabel(item.updated_at)} ago</span>
                          {/if}
                        </span>
                      </button>
                    {/snippet}
                  </Hint>
                  <span class="notif-actions ml-auto flex flex-none flex-wrap items-center justify-end gap-ui-2">
                    <Button class={tb} variant="outline"
                      type="button"
                      title="Mark this notification read"
                      onclick={() => void markRead(item)}
                    >
                      Mark read
                    </Button>
                    <Hint label="Open on GitHub">
                      {#snippet child({ props })}
                        <Button class={tb} variant="outline"
                          {...props}
                          type="button"
                          onclick={() => openSubjectUrl(item.subject_url)}
                        >
                          Open
                        </Button>
                      {/snippet}
                    </Hint>
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
      <div class="card current mx-2 my-1 rounded-lg border border-border bg-card px-2.5 py-2">
        <div class="card-head flex min-w-0 flex-wrap items-center gap-ui-375">
          <span
            class="chip {prStateClass(branchPr.state)} {chipTone[prStateClass(branchPr.state)] ?? "bg-accent text-foreground"} flex-none rounded-full px-2 py-px text-ui-micro font-semibold"
          >{prStateLabel(branchPr.state)}</span>
          {#if branchPr.is_draft}
            <span class="chip chip-draft flex-none rounded-full border border-border bg-transparent px-2 py-px text-ui-micro font-semibold text-muted-foreground">Draft</span>
          {/if}
          <Hint label={branchPr.title}>
            {#snippet child({ props })}
              <span class="pr-title min-w-0 truncate font-semibold text-foreground" {...props}>{branchPr.title}</span>
            {/snippet}
          </Hint>
          <span class="pr-no flex-none font-mono text-ui-meta text-muted-foreground">#{branchPr.number}</span>
        </div>
        <div class="card-sub mt-ui-375 flex flex-wrap items-center gap-2">
          <span class="refs font-mono text-ui-meta text-muted-foreground">{branchPr.head_ref_name} → {branchPr.base_ref_name}</span>
          {#if branchAge}
            <Hint label="Opened">
              {#snippet child({ props })}
                <span class="age text-ui-micro text-muted-foreground" {...props}
                  >opened {branchAge === "just now" ? "just now" : `${branchAge} ago`}</span
                >
              {/snippet}
            </Hint>
          {/if}
        </div>
        {@render prMeta(branchPr)}
        {@render prBody(branchPr)}
        {@render actionsBlock(branchPr)}
        {#if isPrOpen(branchPr.state)}
          {@render checksBlock(branchPr)}
        {/if}
        {@render prFilesBlock(branchPr)}
        {@render prThreadsBlock(branchPr)}
      </div>
    {:else}
      <div class="card empty mx-2 my-1 flex flex-wrap items-center gap-3 rounded-lg border border-border bg-card px-2.5 py-2">
        <p class="m-0 text-muted-foreground">
          No pull request for branch <code>{context.branch}</code> yet.
        </p>
        <Button
          class="act h-auto flex-none rounded-full px-3 py-0.5 text-ui-meta"
          type="button"
          onclick={() => (dialogOpen = true)}
        >Create PR</Button>
      </div>
    {/if}

    <div class="section mx-2 mb-3 mt-2">
      <h3 class="section-title mb-1 mt-0 text-ui-meta font-semibold uppercase tracking-[0.04em] text-muted-foreground">
        {filter === "open"
          ? "Open pull requests"
          : filter === "all"
            ? "All pull requests"
            : `${filter[0]?.toUpperCase() ?? ""}${filter.slice(1)} pull requests`}
      </h3>
      {#if prsError}
        <p class="error-inline my-ui-15 text-xs text-destructive [overflow-wrap:anywhere]" role="alert">{prsError}</p>
      {/if}
      {#if listedPrs.length === 0}
        <p class="muted my-1 text-ui-meta text-muted-foreground">{emptyLabel}</p>
      {:else}
        <ul class="prs m-0 flex list-none flex-col gap-1 p-0" role="list" aria-label="Pull requests">
          {#each listedPrs as pr (pr.number)}
            <li>
              <div class="pr-line flex min-w-0 items-start gap-1">
                <Hint label="Show checks and actions">
                  {#snippet child({ props })}
                    <button
                      class="pr-row flex-1 flex w-full min-w-0 cursor-pointer items-center gap-ui-375 rounded-lg border border-border bg-card px-2 py-ui-375 text-left text-ui-meta text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
                      {...props}
                      type="button"
                      aria-expanded={expandedNumber === pr.number}
                      onclick={() => toggleExpanded(pr.number)}
                    >
                      <span class="expand-icon w-[1em] flex-none text-center text-muted-foreground" aria-hidden="true">
                        {expandedNumber === pr.number ? "▾" : "▸"}
                      </span>
                      <span
                        class="chip {prStateClass(pr.state)} {chipTone[prStateClass(pr.state)] ?? "bg-accent text-foreground"} flex-none rounded-full px-2 py-px text-ui-micro font-semibold"
                      >{prStateLabel(pr.state)}</span>
                      {#if pr.is_draft}
                        <span class="chip chip-draft flex-none rounded-full border border-border bg-transparent px-2 py-px text-ui-micro font-semibold text-muted-foreground">Draft</span>
                      {/if}
                      <Hint label={pr.title}>
                        {#snippet child({ props })}
                          <span class="pr-title min-w-0 truncate font-semibold text-foreground" {...props}>{pr.title}</span>
                        {/snippet}
                      </Hint>
                      <span class="pr-no flex-none font-mono text-ui-meta text-muted-foreground">#{pr.number}</span>
                      <span class="refs ml-auto max-w-[40%] flex-none truncate font-mono text-ui-meta text-muted-foreground">{pr.head_ref_name} → {pr.base_ref_name}</span>
                    </button>
                  {/snippet}
                </Hint>
                <Hint label="Open on GitHub">
                  {#snippet child({ props })}
                    <Button class="{tb} mt-ui-375" variant="outline"
                      {...props}
                      type="button"
                      onclick={() => void openUrl(pr.url)}
                    >
                      Open
                    </Button>
                  {/snippet}
                </Hint>
              </div>
              <div class="pr-sub pb-ui-2 pl-[1.5rem] pr-2 pt-ui-15">
                {@render prMeta(pr)}
              </div>
              {#if expandedNumber === pr.number}
                <div class="expand mb-1 ml-[1.5rem] mt-1">
                  {@render prBody(pr)}
                  {@render actionsBlock(pr)}
                  {#if isPrOpen(pr.state)}
                    {@render checksBlock(pr)}
                  {/if}
                  {@render prFilesBlock(pr)}
                  {@render prThreadsBlock(pr)}
                </div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}

  <CreatePrDialog {repoId} open={dialogOpen} onClose={() => (dialogOpen = false)} onCreated={onCreated} />

  <CreateIssueDialog
    {repoId}
    open={issueDialogOpen}
    onClose={() => (issueDialogOpen = false)}
    onCreated={onIssueCreated}
  />

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
    <Label class="dialog-field mt-2 flex items-center gap-2 text-ui-base">
      <span>Workflow</span>
      <select
        class="flex-1 rounded-sm border border-border bg-background px-ui-35 py-0.5 text-foreground"
        bind:value={dispatchWorkflow}
      >
        {#if dispatchable.length === 0}
          <option value="">No active workflows</option>
        {/if}
        {#each dispatchable as wf (wf.id)}
          <option value={wf.name}>{wf.name} — {wf.path}</option>
        {/each}
      </select>
    </Label>
    <Label class="dialog-field mt-2 flex items-center gap-2 text-ui-base">
      <span>Ref</span>
      <input
        class="dialog-text mt-0 w-auto min-w-0 flex-1 rounded-sm border border-border bg-background p-2 text-ui-base text-foreground"
        aria-label="Branch or tag to run on"
        placeholder={context?.branch ?? "main"}
        bind:value={dispatchRef}
      />
    </Label>
    <Label class="dialog-field dialog-field-top mt-2 flex items-start gap-2 text-ui-base">
      <span class="pt-ui-35">Inputs</span>
      <textarea
        class="dialog-text mt-0 w-auto min-w-0 flex-1 resize-y rounded-sm border border-border bg-background p-2 text-ui-base text-foreground"
        aria-label="Workflow inputs"
        rows="4"
        placeholder={"One key=value per line\nversion=1.2.3\ndry_run=true"}
        bind:value={dispatchInputs}
      ></textarea>
    </Label>
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
    <Label class="dialog-field mt-2 flex items-center gap-2 text-ui-base">
      <span>Method</span>
      <select
        class="flex-1 rounded-sm border border-border bg-background px-ui-35 py-0.5 text-foreground"
        bind:value={mergeMethod}
      >
        <option value="squash">Squash and merge</option>
        <option value="merge">Create a merge commit</option>
        <option value="rebase">Rebase and merge</option>
      </select>
    </Label>
    <Label class="dialog-check mt-2 flex items-center gap-2 text-ui-base text-muted-foreground">
      <input type="checkbox" bind:checked={deleteBranch} />
      Delete the branch after merging
    </Label>
    <Label class="dialog-check mt-2 flex items-center gap-2 text-ui-base text-muted-foreground">
      <input type="checkbox" bind:checked={autoMerge} />
      Enable auto-merge (merge when required checks pass)
    </Label>
    {#if mergeError}
      <!-- Branch-protection (and other gh) rejections surface verbatim above;
           Retry re-submits the same request without touching the fields. -->
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
      {#if mergeError}
        <button
          class="sdlg-secondary"
          type="button"
          disabled={mergeBusy}
          onclick={() => void confirmMerge()}
        >
          Retry
        </button>
      {/if}
      <button class="sdlg-primary" type="submit" disabled={mergeBusy}>
        {autoMerge ? "Enable auto-merge" : "Merge"}
      </button>
    {/snippet}
  </DialogShell>

  <DialogShell
    bind:open={checkoutOpen}
    labelledBy="forge-checkout-title"
    onCancel={() => (checkoutTarget = null)}
    onsubmit={() => void confirmCheckout()}
  >
    <h2 class="sdlg-title" id="forge-checkout-title">Check out #{checkoutTarget?.number}</h2>
    <p class="sdlg-message">
      Switches this repository to
      <code class="rounded-sm bg-accent px-1 font-mono">{checkoutTarget?.head_ref_name}</code>.
    </p>
    {#if checkoutRefusalText !== null}
      <p class="dialog-error m-0 mt-2 text-xs text-destructive" role="alert">
        {checkoutRefusalText}{checkoutDirtyCount > 0
          ? ` (${checkoutDirtyCount} path${checkoutDirtyCount === 1 ? "" : "s"})`
          : ""}{checkoutDirtyCount > 0
          ? " — stash them to check out this pull request."
          : " — finish or abort it before checking out."}
      </p>
      {#if checkoutDirtyCount > 0}
        <Label class="dialog-check mt-2 flex items-center gap-2 text-ui-base text-muted-foreground">
          <input type="checkbox" bind:checked={checkoutStash} />
          Stash the changes (restore later from the Stashes panel)
        </Label>
        {#if checkoutStash}
          <Label class="dialog-field mt-2 flex items-center gap-2 text-ui-base">
            <span>Stash name</span>
            <input
              class="dialog-text mt-0 w-auto min-w-0 flex-1 rounded-sm border border-border bg-background p-2 text-ui-base text-foreground"
              aria-label="Stash name"
              bind:value={checkoutStashName}
            />
          </Label>
        {/if}
      {/if}
    {/if}
    {#if checkoutError}
      <p class="dialog-error m-0 mt-[0.6rem] text-xs text-destructive" role="alert">{checkoutError}</p>
    {/if}
    {#snippet actions()}
      <button
        class="sdlg-secondary"
        type="button"
        onclick={() => {
          checkoutOpen = false;
          checkoutTarget = null;
        }}
      >
        Cancel
      </button>
      <button
        class="sdlg-primary"
        type="submit"
        disabled={checkoutBusy ||
          (checkoutRefusalText !== null &&
            (checkoutDirtyCount === 0 || !checkoutStash))}
      >
        {checkoutBusy
          ? "Checking out…"
          : checkoutDirtyCount > 0 && checkoutStash
            ? "Stash & check out"
            : "Check out"}
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
      class="dialog-text mt-1 w-full resize-y rounded-sm border border-border bg-background p-2 text-ui-base text-foreground"
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

  <DialogShell
    bind:open={editOpen}
    labelledBy="forge-edit-title"
    width="34rem"
    onCancel={() => (editTarget = null)}
    onsubmit={() => void confirmEdit()}
  >
    <h2 class="sdlg-title" id="forge-edit-title">Edit pull request</h2>
    <p class="sdlg-message">#{editTarget?.number} — only the title and description can be edited.</p>
    <Label class="dialog-field dialog-field-top mt-2 flex items-start gap-2 text-ui-base">
      <span class="pt-ui-35">Title</span>
      <input
        class="dialog-text mt-0 w-auto min-w-0 flex-1 rounded-sm border border-border bg-background p-2 text-ui-base text-foreground"
        aria-label="Pull request title"
        bind:value={editTitle}
        data-autofocus
      />
    </Label>
    <Label class="dialog-field dialog-field-top mt-2 flex items-start gap-2 text-ui-base">
      <span class="pt-ui-35">Description</span>
      <textarea
        class="dialog-text mt-0 w-auto min-w-0 flex-1 resize-y rounded-sm border border-border bg-background p-2 text-ui-base text-foreground"
        aria-label="Pull request description"
        rows="8"
        bind:value={editBody}
      ></textarea>
    </Label>
    {#if editError}
      <p class="dialog-error m-0 mt-[0.6rem] text-xs text-destructive" role="alert">{editError}</p>
    {/if}
    {#snippet actions()}
      <button
        class="sdlg-secondary"
        type="button"
        disabled={editBusy}
        onclick={() => {
          editOpen = false;
          editTarget = null;
        }}
      >
        Cancel
      </button>
      <button class="sdlg-primary" type="submit" disabled={editBusy || !editChanged}>
        {editBusy ? "Saving…" : "Save"}
      </button>
    {/snippet}
  </DialogShell>

  <DialogShell
    bind:open={issueTextOpen}
    labelledBy="forge-issue-text-title"
    onCancel={() => (issueTextTarget = null)}
    onsubmit={() => void confirmIssueText()}
  >
    <h2 class="sdlg-title" id="forge-issue-text-title">{issueTextTitle}</h2>
    <p class="sdlg-message">{issueTextTarget?.title ?? ""} — optional comment.</p>
    <textarea
      class="dialog-text mt-1 w-full resize-y rounded-sm border border-border bg-background p-2 text-ui-base text-foreground"
      rows="5"
      bind:value={issueTextBody}
      placeholder={issueTextAction === "close"
        ? "Why is this being closed? (optional)"
        : "Why is this being reopened? (optional)"}
      data-autofocus
    ></textarea>
    {#if issueTextError}
      <p class="dialog-error m-0 mt-[0.6rem] text-xs text-destructive" role="alert">{issueTextError}</p>
    {/if}
    {#snippet actions()}
      <button
        class="sdlg-secondary"
        type="button"
        disabled={issueTextBusy}
        onclick={() => {
          issueTextOpen = false;
          issueTextTarget = null;
        }}
      >
        Cancel
      </button>
      <button
        class={issueTextAction === "close" ? "sdlg-danger" : "sdlg-primary"}
        type="submit"
        disabled={issueTextBusy}
      >
        {issueTextConfirm}
      </button>
    {/snippet}
  </DialogShell>
</aside>
