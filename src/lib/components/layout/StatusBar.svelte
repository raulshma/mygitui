<script lang="ts">
  /**
   * Global status bar (VS Code-style) — the workspace's bottom chrome.
   *
   * A fixed-height strip rendered once per workspace below the main area
   * (App.svelte): transient indicators appear *inside* the bar, so nothing
   * above it ever resizes. (The old OpLogStrip progress row mounted and
   * unmounted in flow below the repo header, shifting the whole workspace on
   * every auto-fetch tick.)
   *
   * Left: the active repo's branch (click opens the branch switcher), a Sync
   * item (click runs the contextual pull/push/fetch verb) and the
   * working-copy change count (click focuses the Status panel). Middle: one
   * item per in-flight backend op across all open repos — the ONE spinner
   * surface for git work, so header/commit controls never duplicate it;
   * progress, percent and the cooperative Cancel live here, and every
   * network row is click-through: rows whose repo has hook output
   * (pre-push, commit hooks) open the log overlay, while fetch/pull/push
   * rows without output yet open the recent-ops popover for their repo.
   * Hook/CLI output also rides a chip on the right and expands into the
   * same fixed overlay above the bar.
   * When the ACTIVE repo has no op in flight but completed-op history, the
   * same area shows a quiet outcome chip (success check / alert icon) that
   * opens the recent-ops popover (`OpHistoryPopover`) — the after-the-fact
   * "what ran and what failed" surface the transient op row never gave.
   * Right: the in-progress git state
 * (merge/rebase/…), the repo's auto-fetch cadence (click opens the Remotes
   * panel), and the remote device connection.
   *
   * Reads the store singletons directly (no props), like the Toaster. The
   * bar itself is never conditionally sized — that constancy is the
   * anti-twitch contract.
   */
  import {
    cancelOp,
    kindLabel,
    opHistoryFor,
    opLogsFor,
    opStore,
    type ActiveOp,
  } from "$lib/stores/ops.svelte";
  import { refreshStatus, tabStore } from "$lib/stores/tabs.svelte";
  import { autofetch } from "$lib/stores/autofetch.svelte";
  import { remoteDevices } from "$lib/stores/remoteDevices.svelte";
  import OpLogStrip from "$lib/components/panels/OpLogStrip.svelte";
  import OpHistoryPopover from "$lib/components/layout/OpHistoryPopover.svelte";
  import { emitUiEvent } from "$lib/palette/events";
  import {
    fetchDefaultRemote,
    pullCurrentBranch,
    pushCurrentBranch,
    syncSkipMessage,
    syncSuccessMessage,
    type SyncVerb,
  } from "$lib/git/sync";
  import { formatRelativeTime } from "$lib/git/format";
  import { conflictSource, conflictSourceLabel } from "$lib/components/panels/panelModel";
  import { panelIconData } from "$lib/layout/layoutModel";
  import { toast } from "$lib/toast";
  import { describeError } from "$lib/errors";
  import { ai } from "$lib/ai/ai.svelte";
  import { aiElapsedLabel, aiFeatureLabel, aiPhaseLabel } from "$lib/ai/registry";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";

  const iconBranch = $derived(appIcon("account-tree")!);
  const iconSync = $derived(appIcon("sync")!);
  const iconPull = $derived(appIcon("arrow-downward")!);
  const iconPush = $derived(appIcon("arrow-upward")!);
  const iconClose = $derived(appIcon("close")!);
  const iconCloud = $derived(appIcon("cloud")!);
  const iconAlert = $derived(appIcon("triangle-alert")!);
  const iconRefresh = $derived(appIcon("refresh")!);
  const iconChanges = $derived(panelIconData("status"));
  const iconOutput = $derived(appIcon("terminal")!);
  const iconDone = $derived(appIcon("check-circle")!);
  const iconBan = $derived(appIcon("ban")!);

  /** "1 commit" / "3 commits" for tooltips; counts capped like the header. */
  function commitCount(count: number): string {
    return `${count} commit${count === 1 ? "" : "s"}`;
  }
  function badgeCount(count: number): string {
    return count > 99 ? "99+" : String(count);
  }

  // -- active repo context ---------------------------------------------------

  const active = $derived(tabStore.active);
  const repoId = $derived(active?.id ?? null);
  const status = $derived(active?.status ?? null);

  const branchLabel = $derived.by(() => {
    if (status === null) return null;
    if (status.detached) {
      return status.head ? `detached @ ${status.head.slice(0, 7)}` : "detached";
    }
    return status.branch ?? "unknown";
  });

  /** Unique changed paths (the ActivityBar badge's number). */
  const changeCount = $derived(
    status === null ? 0 : new Set(status.entries.map((e) => e.path)).size,
  );

  /** In-progress git state (merge/rebase/sequencer/bisect), or `null`. */
  const opStateLabel = $derived.by(() => {
    if (status === null) return null;
    const source = conflictSource(status);
    if (source !== null) return `${conflictSourceLabel(source)} in progress`;
    if (status.bisecting) return "Bisect in progress";
    return null;
  });

  /** Per-repo override else global default; 0 = off. Reactive via $state. */
  const autofetchMinutes = $derived(
    active === null ? 0 : autofetch.getInterval(active.root),
  );

  /** Remote-tab device connection (`null` for local repos). */
  const deviceStatus = $derived(
    active?.deviceId ? remoteDevices.statusOf(active.deviceId) : null,
  );

  // -- hook/CLI output (pre-push, commit hooks) ------------------------------

  /** Newest output session for the active repo, kept after the op ends. */
  const logSession = $derived(
    repoId === null ? null : opLogsFor(repoId),
  );
  const logTitle = $derived(
    logSession === null ? "" : `${kindLabel(logSession.kind)} output`,
  );
  const logCount = $derived(
    logSession === null
      ? 0
      : logSession.lines.length,
  );
  const logFailed = $derived(
    logSession !== null && logSession.done && logSession.error !== null,
  );

  /**
   * Repo whose log session the overlay shows (`null` = closed). Usually the
   * active repo (the right-side chip), but a click on a running op row —
   * e.g. the "pushing to …" item, where a pre-push hook streams its verify
   * output — opens that op's repo instead, background tabs included.
   */
  let logRepoId = $state<string | null>(null);
  const logChipOpen = $derived(repoId !== null && logRepoId === repoId);

  /** Show/hide the overlay for one repo (chip + op-row click-through). */
  function toggleLog(target: string): void {
    logRepoId = logRepoId === target ? null : target;
    histRepoId = null; // the log overlay and the popover are exclusive
  }

  /**
   * Explicit toggle (no auto-close), but a *new* session opens itself once:
   * the old in-flow strip appeared as soon as hook output arrived, and this
   * preserves that cue without ever shifting the workspace. The overlay
   * stays put when the user closes it; only the next op reopens it. Seen
   * op ids are tracked per repo so switching tabs doesn't replay the cue
   * for a session the user has already seen.
   */
  let seenLogOps = $state<Record<string, string>>({});
  $effect(() => {
    const session = logSession;
    if (session === null || repoId === null) return;
    if (seenLogOps[repoId] === session.op_id) return;
    seenLogOps = { ...seenLogOps, [repoId]: session.op_id };
    logRepoId = repoId;
    histRepoId = null; // the cue opening the overlay closes the popover
  });

  // The session a closed overlay pointed at can be dismissed out from under
  // it (strip's ✕, store reset): an overlay with nothing to show is closed.
  $effect(() => {
    if (logRepoId !== null && opLogsFor(logRepoId) === null) logRepoId = null;
  });

  // -- sync item (same verbs + helpers as the header quick actions) ----------

  const VERB_LABELS: Record<SyncVerb, string> = {
    fetch: "Fetch",
    pull: "Pull",
    push: "Push",
  };
  function isSyncVerb(kind: string): kind is SyncVerb {
    return kind === "fetch" || kind === "pull" || kind === "push";
  }

  /**
   * Sync ops already running for the active repo (any entry point), except
   * a background auto-fetch: it must not put the sync control into a busy
   * state. The middle op row still shows it (with its cancel), and the
   * backend queue serializes anything the user starts behind it.
   */
  const syncOps = $derived(
    repoId === null
      ? []
      : opStore
          .opsFor(repoId)
          .filter(
            (op) =>
              isSyncVerb(op.kind) &&
              !(op.kind === "fetch" && autofetch.isFetching(repoId)),
          ),
  );

  /**
   * Set while this component's invoke is awaiting the backend — identity-
   * tagged with the repo (RepoView swaps repos under this component, so an
   * untagged flag would spin on every other project until the RPC settles).
   */
  let pending = $state.raw<{ repoId: string; verb: SyncVerb } | null>(null);
  const pendingVerb = $derived(
    pending !== null && pending.repoId === repoId ? pending.verb : null,
  );

  const busy = $derived(syncOps.length > 0 || pendingVerb !== null);
  const runningVerb = $derived<SyncVerb | null>(
    pendingVerb ?? (syncOps[0]?.kind as SyncVerb | null) ?? null,
  );

  const ahead = $derived(status?.ahead ?? 0);
  const behind = $derived(status?.behind ?? 0);
  /** Pull when behind, push when ahead, otherwise refresh (fetch). */
  const primaryVerb = $derived<SyncVerb>(
    behind > 0 ? "pull" : ahead > 0 ? "push" : "fetch",
  );
  /** Pull/push need a branch; fetch works even before status loads. */
  const canUseBranch = $derived(
    status?.branch != null && status?.detached !== true,
  );

  const syncTitle = $derived.by(() => {
    if (runningVerb !== null) return `${VERB_LABELS[runningVerb]} in progress…`;
    if (primaryVerb === "fetch") return "Fetch from the primary remote (prune)";
    if (primaryVerb === "pull") {
      return canUseBranch
        ? `Pull ${commitCount(behind)} into ${status?.branch}`
        : "Pull is unavailable without a current branch";
    }
    return canUseBranch
      ? `Push ${commitCount(ahead)} to the remote`
      : "Push is unavailable without a current branch";
  });

  const syncAriaLabel = $derived.by(() => {
    if (runningVerb !== null) return `${VERB_LABELS[runningVerb]} in progress…`;
    if (primaryVerb === "fetch") return "Sync with remote (fetch)";
    if (primaryVerb === "pull") {
      return `Sync with remote (pull ${status?.branch ?? "current branch"}${behind > 0 ? `, ${behind} behind` : ""})`;
    }
    return `Sync with remote (push ${status?.branch ?? "current branch"}${ahead > 0 ? `, ${ahead} ahead` : ""})`;
  });

  async function runSync(): Promise<void> {
    if (repoId === null || busy) return;
    const verb = primaryVerb;
    const branch = status?.branch ?? null;
    if (verb !== "fetch" && !canUseBranch) return;

    // Identity-checked so a finishing run never clears a newer run started
    // for another repo while this one was still in flight.
    const entry = { repoId, verb };
    pending = entry;
    try {
      const outcome =
        verb === "fetch"
          ? await fetchDefaultRemote(repoId)
          : verb === "pull"
            ? await pullCurrentBranch(repoId, branch)
            : await pushCurrentBranch(repoId, branch);
      if (outcome.kind === "skipped") {
        toast(syncSkipMessage(outcome.reason, verb), { kind: "error" });
        return;
      }
      toast(syncSuccessMessage(verb, outcome, branch), { kind: "success" });
      void refreshStatus(repoId);
    } catch (err) {
      toast(`${VERB_LABELS[verb]} failed: ${describeError(err)}`, {
        kind: "error",
      });
    } finally {
      if (pending === entry) pending = null;
    }
  }

  // -- running ops (all repos) ------------------------------------------------

  /**
   * One flat row per in-flight backend op, across every open repo (ops are
   * global: an auto-fetch on this repo or a push started from a panel all
   * land here). Rows for a background repo carry its tab name. `logKind` is
   * the repo's hook/CLI output session (pre-commit / pre-push lines), when
   * one exists — such rows are click-through to the log overlay, and
   * log-less network rows (fetch/pull/push) to the recent-ops popover.
   */
  const opRows = $derived(
    Object.entries(opStore.ops).flatMap(([opRepoId, list]) => {
      const log = opStore.logs[opRepoId] ?? null;
      return list.map((op: ActiveOp) => ({
        op,
        repoId: opRepoId,
        repoName:
          opRepoId === repoId
            ? null
            : (tabStore.tabs.find((t) => t.id === opRepoId)?.name ?? null),
        logKind: log?.kind ?? null,
      }));
    }),
  );

  // -- completed-op history (recent-ops chip + popover) ----------------------

  /**
   * Completed ops of the active repo, newest first. The quiet chip that
   * surfaces them renders ONLY while no op is in flight for the repo (the
   * in-flight row owns the ops area; the chip takes the idle slot) — it
   * shows the newest entry's outcome as its icon and opens the
   * {@link OpHistoryPopover} for the full ring. Icon-only by design: the
   * chip must stay quiet next to (and never be confused with) live rows.
   */
  const opHistory = $derived(repoId === null ? [] : opHistoryFor(repoId));
  /** The newest entry's outcome drives the chip's icon + color. */
  const lastOutcome = $derived(opHistory[0]?.outcome ?? null);
  const showHistoryChip = $derived(
    repoId !== null && opHistory.length > 0 && !opStore.busy(repoId),
  );

  /** Chip tooltip / accessible name: the newest op in one line. */
  const historyTitle = $derived.by(() => {
    const last = opHistory[0];
    if (last === undefined) return "Recent operations";
    return `Recent operations — last: ${kindLabel(last.kind)} ${last.outcome}, ${formatRelativeTime(Math.round(last.finished_at / 1000))}`;
  });

  /**
   * Repo whose history the popover shows (`null` = closed). Opened by the
   * idle chip (active repo) or by clicking a network op row with no log
   * session — any repo, anchored to the row button. One popover at a time:
   * opening (or auto-opening) the log overlay closes it and vice versa.
   */
  let histRepoId = $state<string | null>(null);
  /** Whether the idle chip's own popover is the open one (its aria state). */
  const histChipOpen = $derived(repoId !== null && histRepoId === repoId);
  let histTriggerEl = $state<HTMLButtonElement | null>(null);

  function toggleHistory(trigger: HTMLButtonElement): void {
    if (repoId === null) return;
    histRepoId = histRepoId === repoId ? null : repoId;
    histTriggerEl = trigger;
    logRepoId = null; // the log overlay and the popover are exclusive
  }

  /**
   * Same toggle from an op row: the trigger is the row's main segment, so
   * the panel anchors there instead of the idle chip — the "view the op"
   * click-through for network rows that have no hook output to show.
   */
  function toggleRowHistory(target: string, trigger: HTMLButtonElement): void {
    histRepoId = histRepoId === target ? null : target;
    histTriggerEl = trigger;
    logRepoId = null; // the log overlay and the popover are exclusive
  }

  /** Closes the popover and returns focus to its trigger (Escape/outside). */
  function closeHistory(): void {
    histRepoId = null;
    histTriggerEl?.focus();
  }

  // A popover whose history was evicted (tab close clears it with the
  // other per-repo caches) has nothing left to list — close it, like the
  // log-overlay effect above retires a dismissed session. A row-opened
  // popover also closes once its repo's ops drain: the row button it is
  // anchored to is gone then (the active repo instead re-anchors to the
  // idle chip the finished op leaves behind, so it stays to show the
  // outcome).
  $effect(() => {
    if (histRepoId === null) return;
    if (
      opHistoryFor(histRepoId).length === 0 &&
      opStore.opsFor(histRepoId).length === 0
    ) {
      histRepoId = null;
      return;
    }
    if (histRepoId !== repoId && !opStore.busy(histRepoId)) {
      histRepoId = null;
    }
  });

  // -- AI jobs (M13) ------------------------------------------------------------

  /**
   * One row per in-flight AI run, same treatment as backend ops: phase,
   * elapsed, cancel. Hidden entirely by the AI master switch or the
   * status-bar toggle — the workspace above never moves either way.
   */
  const aiRows = $derived(
    ai.isEnabled() && ai.isStatusBarEnabled()
      ? Object.values(ai.jobs.active).map((job) => ({
          job,
          repoName:
            job.repoId === repoId
              ? null
              : (job.repoName ??
                tabStore.tabs.find((t) => t.id === job.repoId)?.name ??
                null),
        }))
      : [],
  );

  /** 1s ticker for the elapsed labels — runs only while rows are visible. */
  let now = $state(Date.now());
  $effect(() => {
    if (aiRows.length === 0) return;
    now = Date.now();
    const timer = setInterval(() => (now = Date.now()), 1_000);
    return () => clearInterval(timer);
  });

  /** Jobs the user asked to cancel; "Cancelling…" until the row drops. */
  let aiCancelling = $state<Record<string, true>>({});

  function cancelAiJob(id: string): void {
    aiCancelling[id] = true;
    ai.jobs.cancel(id);
  }

  // Entries are only ever added above, so a job that reaches a terminal
  // state (cancel honored or not) must drop its flag or the map grows
  // without bound across a session.
  $effect(() => {
    const live = new Set(Object.keys(ai.jobs.active));
    for (const id of Object.keys(aiCancelling)) {
      if (!live.has(id)) delete aiCancelling[id];
    }
  });
</script>

<!-- The hook-output overlay is a sibling of the footer, not a child: the
     footer's overflow-hidden would clip it. Anchored to this wrapper's top
     edge, it floats over the workspace without shifting anything. -->
<div class="status-bar-root relative min-w-0 flex-none">
  {#if logRepoId !== null && opLogsFor(logRepoId) !== null}
    <div
      class="op-log-layer absolute inset-x-0 bottom-full z-10 shadow-md [&>section]:border-b-0"
    >
      <OpLogStrip repoId={logRepoId} />
    </div>
  {/if}
  <!-- The recent-ops popover portals itself to <body> via the floating
       action, so unlike the log overlay it needs no layer wrapper here. -->
  {#if histRepoId !== null}
    <OpHistoryPopover
      repoId={histRepoId}
      anchor={histTriggerEl ?? undefined}
      onClose={closeHistory}
    />
  {/if}
  <footer
    class="status-bar flex h-6 min-w-0 flex-none items-center gap-ui-3 overflow-hidden border-t border-border bg-muted/80 px-ui-2 text-ui-micro text-muted-foreground"
    aria-label="Status bar"
    aria-busy={opRows.length > 0 || aiRows.length > 0}
  >
  <!-- Left: the active repo's branch / sync / changes. -->
  <div class="repo-state flex min-w-0 flex-none items-center gap-ui-1">
    {#if branchLabel !== null}
      <Hint
        label={status?.detached
          ? "Detached HEAD — click to pick a branch"
          : "Switch branch"}
      >
        {#snippet child({ props })}
          <button
            {...props}
            class="sb-item inline-flex h-full max-w-[16rem] flex-none items-center gap-ui-2 px-ui-2 font-medium transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            onclick={() => {
              if (repoId !== null) emitUiEvent("branch-switcher-open", { repoId });
            }}
          >
            <Icon icon={iconBranch} size={12} />
            <span class="truncate">{branchLabel}</span>
          </button>
        {/snippet}
      </Hint>
    {/if}

    <Hint label={syncTitle}>
      {#snippet child({ props })}
        <button
          {...props}
          class="sb-item inline-flex h-full flex-none items-center gap-ui-2 px-ui-2 font-medium transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary disabled:pointer-events-none"
          type="button"
          disabled={busy || (primaryVerb !== "fetch" && !canUseBranch)}
          aria-label={syncAriaLabel}
          aria-busy={runningVerb !== null}
          onclick={() => void runSync()}
        >
          <span
            class="sync-icon inline-flex size-3.5 flex-none items-center justify-center"
            aria-hidden="true"
          >
            <Icon icon={iconSync} size={14} />
          </span>
          {#if behind > 0 && runningVerb === null}
            <span
              class="count behind inline-flex flex-none items-center gap-ui-1 tabular-nums"
            >
              <Icon icon={iconPull} size={12} />
              {badgeCount(behind)}
            </span>
          {/if}
          {#if ahead > 0 && runningVerb === null}
            <span
              class="count ahead inline-flex flex-none items-center gap-ui-1 tabular-nums"
            >
              <Icon icon={iconPush} size={12} />
              {badgeCount(ahead)}
            </span>
          {/if}
        </button>
      {/snippet}
    </Hint>

    {#if status !== null}
      <Hint
        label={`${changeCount} working-copy change${changeCount === 1 ? "" : "s"} — click to open the Status panel`}
      >
        {#snippet child({ props })}
          <button
            {...props}
            class="sb-item inline-flex h-full flex-none items-center gap-ui-2 px-ui-2 transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            onclick={() => {
              if (repoId !== null) emitUiEvent("focus-panel", { panel: "status", repoId });
            }}
          >
            <Icon icon={iconChanges} size={12} />
            <span class="tabular-nums">
              {changeCount} change{changeCount === 1 ? "" : "s"}
            </span>
          </button>
        {/snippet}
      </Hint>
    {/if}
  </div>

  <!-- Middle: every in-flight backend op, any repo. Items appear and vanish
       inside the fixed-height bar — the workspace above never moves. -->
  <div
    class="running-ops flex min-w-0 flex-1 items-center gap-ui-3 overflow-hidden"
    aria-label="Running operations"
  >
    {#snippet opMain(row: (typeof opRows)[number])}
      <span
        class="inline-flex size-3.5 flex-none items-center justify-center"
        aria-hidden="true"
      >
        <Spinner class="size-3" role="presentation" aria-hidden="true" />
      </span>
      {#if row.repoName !== null}
        <span class="flex-none font-medium text-foreground"
          >{row.repoName}</span
        >
      {/if}
      <span class="flex-none font-medium text-foreground"
        >{kindLabel(row.op.kind)}</span
      >
      <span class="min-w-0 flex-1 truncate text-left">{row.op.message}</span>
    {/snippet}
    {#each opRows as row (row.op.op_id)}
      <div
        class="sb-op flex min-w-0 flex-1 items-center gap-ui-2"
        aria-label={`${kindLabel(row.op.kind)} in progress`}
      >
        {#if row.logKind !== null}
          <!-- Hook/CLI output exists for this op's repo: the row's main
               segment opens the log overlay (same click-through as the AI
               rows), so clicking "pushing to …" reveals the pre-push /
               pre-commit hook lines behind it. -->
          <Hint
            label={logRepoId === row.repoId
              ? `Hide ${kindLabel(row.logKind)} output`
              : `Show ${kindLabel(row.logKind)} output`}
          >
            {#snippet child({ props })}
              <button
                {...props}
                class="flex h-full min-w-0 flex-1 items-center gap-ui-2 text-left transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                type="button"
                aria-expanded={logRepoId === row.repoId}
                onclick={() => toggleLog(row.repoId)}
              >
                {@render opMain(row)}
              </button>
            {/snippet}
          </Hint>
        {:else if isSyncVerb(row.op.kind)}
          <!-- No hook output for this repo, but a network op (fetch/pull/
               push): the row's main segment opens the recent-ops popover
               for its repo — the same click-through as the log rows,
               falling back to the op history + in-flight list when there
               is no output to show. -->
          <Hint
            label={histRepoId === row.repoId
              ? "Hide recent operations"
              : "Show recent operations"}
          >
            {#snippet child({ props })}
              <button
                {...props}
                class="flex h-full min-w-0 flex-1 items-center gap-ui-2 text-left transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                type="button"
                aria-expanded={histRepoId === row.repoId}
                aria-haspopup="dialog"
                onclick={(e) => toggleRowHistory(row.repoId, e.currentTarget)}
              >
                {@render opMain(row)}
              </button>
            {/snippet}
          </Hint>
        {:else}
          <!-- Neither hook output nor a network op: the row stays inert. -->
          <div class="flex min-w-0 flex-1 items-center gap-ui-2">
            {@render opMain(row)}
          </div>
        {/if}
        {#if row.op.pct !== null}
          <span class="flex-none tabular-nums">{Math.round(row.op.pct)}%</span>
        {/if}
        {#if row.op.cancelling}
          <span
            class="inline-flex flex-none items-center gap-ui-2 pr-ui-2"
            role="status"
          >
            Cancelling…
          </span>
        {:else}
          <Hint label={`Cancel ${kindLabel(row.op.kind)}`}>
            {#snippet child({ props })}
              <!-- Full-bar 24px hit target (WCAG 2.5.8) with the same
                   rectangular hover as the sibling bar items; the icon
                   stays at its visual 12px size. -->
              <button
                {...props}
                class="inline-flex h-full w-6 flex-none items-center justify-center transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary [&_svg:not([class*='size-'])]:size-3"
                type="button"
                aria-label={`Cancel ${kindLabel(row.op.kind)}`}
                onclick={() => void cancelOp(row.op.op_id)}
              >
                <Icon icon={iconClose} size={12} />
              </button>
            {/snippet}
          </Hint>
        {/if}
      </div>
    {/each}
    {#each aiRows as row (row.job.id)}
      <div
        class="sb-ai flex min-w-0 flex-1 items-center gap-ui-2"
        role="status"
        aria-label={`${aiFeatureLabel(row.job.feature)} (AI) in progress`}
        aria-busy="true"
      >
        <!-- The shared sparkle mark (same two-path glyph as the AI
             feature buttons), pulsing while the model works. -->
        <span
          class="inline-flex size-3.5 flex-none items-center justify-center text-primary"
          aria-hidden="true"
        >
          <svg
            class="animate-pulse"
            width="12"
            height="12"
            viewBox="0 0 24 24"
            fill="currentColor"
          >
            <path
              d="M12 2l2.09 6.26L20 10.35l-5.91 2.09L12 18.7l-2.09-6.26L4 10.35l5.91-2.09L12 2z"
            />
            <path d="M19 15l1.18 3.32L23.5 19.5l-3.32 1.18L19 24l-1.18-3.32L14.5 19.5l3.32-1.18L19 15z" />
          </svg>
        </span>
        <!-- Click-through: the row's main segment opens the assistant
             panel's Activity tab (job history + usage). -->
        <button
          class="flex h-full min-w-0 flex-1 items-center gap-ui-2 text-left transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
          type="button"
          aria-label="Open AI activity"
          onclick={() => emitUiEvent("ai-open-activity")}
        >
          {#if row.repoName !== null}
            <span class="flex-none font-medium text-foreground"
              >{row.repoName}</span
            >
          {/if}
          <span class="flex-none font-medium text-foreground"
            >{aiFeatureLabel(row.job.feature)}</span
          >
          <span class="min-w-0 flex-1 truncate text-left">
            {row.job.message ?? aiPhaseLabel(row.job.phase) ?? "working"}
          </span>
          <span class="flex-none tabular-nums">{aiElapsedLabel(now - row.job.startedAt)}</span>
        </button>
        {#if aiCancelling[row.job.id]}
          <span
            class="inline-flex flex-none items-center gap-ui-2 pr-ui-2"
            role="status"
          >
            <Spinner class="size-3" role="presentation" aria-hidden="true" />
            Cancelling…
          </span>
        {:else}
          <Hint label={`Cancel ${aiFeatureLabel(row.job.feature)}`}>
            {#snippet child({ props })}
              <button
                {...props}
                class="inline-flex h-full w-6 flex-none items-center justify-center transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary [&_svg:not([class*='size-'])]:size-3"
                type="button"
                aria-label={`Cancel ${aiFeatureLabel(row.job.feature)}`}
                onclick={() => cancelAiJob(row.job.id)}
              >
                <Icon icon={iconClose} size={12} />
              </button>
            {/snippet}
          </Hint>
        {/if}
      </div>
    {/each}
    {#if showHistoryChip}
      <!-- Idle ops area with completed history: the quiet outcome chip
           (newest entry's result as its icon) opens the recent-ops
           popover. Hidden entirely while the repo has an op in flight —
           the spinner row owns the area then. -->
      <Hint label={historyTitle}>
        {#snippet child({ props })}
          <button
            {...props}
            bind:this={histTriggerEl}
            class="sb-hist inline-flex h-full flex-none items-center px-ui-2 transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            aria-label={historyTitle}
            aria-haspopup="dialog"
            aria-expanded={histChipOpen}
            onclick={(e) => toggleHistory(e.currentTarget)}
          >
            <span
              class="inline-flex size-3.5 flex-none items-center justify-center {lastOutcome ===
              "failed"
                ? "text-destructive"
                : lastOutcome === "succeeded"
                  ? "text-primary"
                  : "text-muted-foreground"}"
              aria-hidden="true"
            >
              {#if lastOutcome === "failed"}
                <Icon icon={iconAlert} size={12} />
              {:else if lastOutcome === "succeeded"}
                <Icon icon={iconDone} size={12} />
              {:else}
                <Icon icon={iconBan} size={12} />
              {/if}
            </span>
          </button>
        {/snippet}
      </Hint>
    {/if}
  </div>

  <!-- Right: hook output + repo state + cadence + device connection. -->
  <div class="extras flex flex-none items-center gap-ui-2 pr-ui-2">
    {#if logSession !== null}
      <Hint label={logChipOpen ? `Hide ${logTitle}` : `Show ${logTitle}`}>
        {#snippet child({ props })}
          <button
            {...props}
            class="sb-log inline-flex h-full flex-none items-center gap-ui-2 px-ui-2 transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            aria-label={logChipOpen ? `Hide ${logTitle}` : `Show ${logTitle}`}
            aria-expanded={logChipOpen}
            aria-busy={!logSession.done}
            onclick={() => {
              if (repoId !== null) toggleLog(repoId);
            }}
          >
            <span
              class="inline-flex size-3.5 flex-none items-center justify-center"
              class:text-primary={logSession.done && logSession.error === null}
              class:text-destructive={logFailed}
              aria-hidden="true"
            >
              {#if logFailed}
                <Icon icon={iconAlert} size={12} />
              {:else if logSession.done}
                <Icon icon={iconDone} size={12} />
              {:else}
                <Icon icon={iconOutput} size={12} />
              {/if}
            </span>
            <span class="truncate">{logTitle}</span>
            <span class="flex-none tabular-nums">{logCount}</span>
          </button>
        {/snippet}
      </Hint>
    {/if}
    {#if opStateLabel !== null}
      <span
        class="sb-state inline-flex flex-none items-center gap-ui-2 rounded-full bg-destructive/15 px-ui-3 py-px font-medium text-destructive"
        role="status"
      >
        <Icon icon={iconAlert} size={12} />
        {opStateLabel}
      </span>
    {/if}
    {#if autofetchMinutes > 0}
      <Hint label={`Auto-fetch every ${autofetchMinutes} min — click to open the Remotes panel`}>
        {#snippet child({ props })}
          <button
            {...props}
            class="sb-autofetch inline-flex h-full flex-none items-center gap-ui-2 px-ui-2 tabular-nums transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            onclick={() => {
              if (repoId !== null)
                emitUiEvent("focus-panel", { panel: "remotes", repoId });
            }}
          >
            <Icon icon={iconRefresh} size={12} />
            auto {autofetchMinutes}m
          </button>
        {/snippet}
      </Hint>
    {/if}
    {#if deviceStatus !== null}
      <Hint
        label={deviceStatus === "connected"
          ? "Remote repository — device connected"
          : deviceStatus === "connecting"
            ? "Remote repository — device connecting…"
            : "Remote repository — device offline"}
      >
        {#snippet child({ props })}
          <span
            {...props}
            class="sb-device inline-flex flex-none items-center gap-ui-2 px-ui-1"
            class:text-foreground={deviceStatus === "connected"}
            data-status={deviceStatus}
          >
            <span
              class="inline-flex flex-none items-center"
              class:text-primary={deviceStatus === "connected"}
              class:opacity-50={deviceStatus === "disconnected"}
              aria-hidden="true"
            >
              <Icon icon={iconCloud} size={12} />
            </span>
            {deviceStatus === "connected"
              ? "connected"
              : deviceStatus === "connecting"
                ? "connecting…"
                : "offline"}
          </span>
        {/snippet}
      </Hint>
    {/if}
  </div>
</footer>
</div>
