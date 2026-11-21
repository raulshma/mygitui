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
   * Right (E1): the GitHub chips (PR → Checks → Runs, one popover at a
   * time, worst-state collapse below 1024px), the in-progress git state
 * (merge/rebase/…), the auto-fetch + GitHub cadence popover, and the
   * remote device connection.
   *
   * §10 Stacked (<800): the matrix trims the bar to "branch + changes
   * only" — the sync item, the ops/AI rows and the whole extras cluster
   * hide (`wm-stacked:hidden`); their information stays reachable through
   * the header's Sync, the full-screen panels and the forge view.
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
  import { refreshStatus, tabStore, closeTab, openTab } from "$lib/stores/tabs.svelte";
  import { autofetch } from "$lib/stores/autofetch.svelte";
  import { remoteDevices } from "$lib/stores/remoteDevices.svelte";
  import {
    startWatchHealth,
    watchChipKind,
    watchHealth,
  } from "$lib/stores/watchHealth.svelte";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
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
  import { appSettings } from "$lib/state/appSettings.svelte";
  import { conflictSource, conflictSourceLabel } from "$lib/components/panels/panelModel";
  import { panelIconData } from "$lib/layout/layoutModel";
  import { toast } from "$lib/toast";
  import { describeError } from "$lib/errors";
  import { ai } from "$lib/ai/ai.svelte";
  import { aiElapsedLabel, aiFeatureLabel, aiPhaseLabel } from "$lib/ai/registry";
  import { budgetLabel, budgetMonthStart, budgetView } from "$lib/ai/budget";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { showMenuAtElement } from "$lib/components/menu/contextMenuStore.svelte";
  import { floating } from "$lib/components/overlay/floating";
  import { Button } from "$lib/components/ui/button/index.js";
  import ForgeChips from "$lib/components/forge/ForgeChips.svelte";
  import { forgeChips } from "$lib/forge/chips.svelte";
  import type { ChipId } from "$lib/forge/indication.svelte";
  import {
    activeSnapshotIntervalMs,
  } from "$lib/forge/chips.svelte";

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

  // E1: the GitHub snapshot lanes. start() probes `gh` status once (the
  // watchHealth idiom); the effect below tracks the open tabs, the active
  // tab and the cadence setting and re-syncs the scheduler lanes — the
  // ACTIVE repo at the configurable cadence (0 = off), inactive-open repos
  // at the fixed 5-minute rate. Tab switches promote via one forced refresh.
  forgeChips.start();
  $effect(() => {
    void tabStore.tabs;
    void tabStore.activeId;
    void appSettings.forgeSnapshotSeconds;
    forgeChips.syncLanes();
  });
  $effect(() => {
    const id = repoId;
    if (id !== null) forgeChips.promote(id);
  });

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

  // -- auto-fetch cadence popover (plan §8.6 / §9.3) -----------------------------
  //
  // The cadence chip is icon + tooltip (the "auto 1m" text went away) and
  // opens a popover that CHANGES the git-fetch cadence instead of only
  // linking to the Remotes panel. Structured for Workstream E1: the
  // popover renders cadence sections, and §9.3 wants a GitHub section
  // rendered next to the git-fetch one — E1 appends it below without
  // touching the chip.

  let cadenceOpen = $state(false);
  let cadenceAnchor = $state<HTMLElement | null>(null);
  let cadenceEl = $state<HTMLElement | null>(null);

  /**
   * The open GitHub chip popover (E1): PR → Checks → Runs, one at a time —
   * owned here so the chips and the cadence popover stay mutually exclusive
   * (the chips' bindable writes it; toggling the cadence clears it).
   */
  let openChip = $state<ChipId | null>(null);

  /** Minutes options (0 = off); the radio rows of the git-fetch section. */
  const CADENCE_OPTIONS = [0, 1, 5, 15, 30, 60] as const;

  /** Seconds options (0 = off); the radio rows of the GitHub section. */
  const FORGE_CADENCE_OPTIONS = [0, 30, 60, 300] as const;

  function toggleCadence(event: MouseEvent): void {
    const trigger = event.currentTarget as HTMLElement;
    openChip = null; // one popover at a time across the right cluster
    if (cadenceOpen) {
      cadenceOpen = false;
      cadenceAnchor?.focus();
    } else {
      cadenceAnchor = trigger;
      cadenceOpen = true;
    }
  }

  // A chip popover opened by the chips component closes the cadence popover.
  $effect(() => {
    if (openChip !== null) cadenceOpen = false;
  });

  /** Writes the per-repo override (0 clears it — the app default applies). */
  function setCadence(minutes: number): void {
    if (active === null) return;
    autofetch.setInterval(active.root, minutes);
  }

  /** Writes the global GitHub snapshot cadence (seconds; 0 = off). */
  function setForgeCadence(seconds: number): void {
    appSettings.update({ forgeSnapshotSeconds: seconds });
  }

  const cadenceTitle = $derived(
    autofetchMinutes > 0
      ? `Auto-fetch every ${autofetchMinutes} min`
      : "Auto-fetch off — click to set a cadence",
  );

  /** The persisted entry is an override when config carries this root. */
  const cadenceIsOverride = $derived(
    active !== null && autofetch.config[active.root] !== undefined,
  );

  // The popover is portaled to <body> (BranchSwitcher idiom): outside-click
  // closes, Escape closes and restores the trigger's focus.
  $effect(() => {
    if (!cadenceOpen) return;
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target as Node | null;
      if (cadenceEl?.contains(target) || cadenceAnchor?.contains(target)) return;
      cadenceOpen = false;
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.stopPropagation();
        cadenceOpen = false;
        cadenceAnchor?.focus();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  });

  // -- AI default-model chip (plan §6.3 / §8.6, deferred from Workstream B) ------
  //
  // ONE global default model lives in the AI settings; this chip SHOWS it
  // and offers the same change set compactly: server default, the current
  // custom id, a free-text id, or the full settings dialog. Per-repo
  // overrides stay in the composers' AiModelPicker carets — this chip never
  // touches `modelChoice`.

  const aiChipVisible = $derived(ai.isEnabled());
  const aiBackendLabel = $derived(
    ai.config.backend === "openrouter" ? "OpenRouter" : "OpenCode",
  );
  /** The global default model id for the active backend ("" = server default). */
  const aiDefaultModel = $derived(
    ai.config.backend === "openrouter"
      ? (ai.config.openrouterModel ?? "")
      : (ai.config.opencodeModel ?? ""),
  );
  const aiChipLabel = $derived(aiDefaultModel === "" ? "AI" : aiDefaultModel);
  const aiChipTitle = $derived(
    aiDefaultModel === ""
      ? `AI model — server default (${aiBackendLabel}) — click to change`
      : `AI model — ${aiDefaultModel} — click to change`,
  );

  /** Free-text entry for the custom default model id (anchored card). */
  let aiModelOpen = $state(false);
  let aiModelText = $state("");
  let aiChipEl = $state<HTMLElement | null>(null);

  function openAiModelMenu(event: MouseEvent): void {
    const entries = [
      {
        id: "ai-model-default",
        label: `${aiDefaultModel === "" ? "✓ " : ""}Default (${aiBackendLabel})`,
        run: () => {
          if (ai.config.backend === "openrouter") ai.setOpenrouterModel(undefined);
          else ai.setOpencodeModel(undefined);
        },
      },
      ...(aiDefaultModel !== ""
        ? [
            {
              id: "ai-model-current",
              label: `✓ ${aiDefaultModel}`,
              run: () => {
                if (ai.config.backend === "openrouter") {
                  ai.setOpenrouterModel(aiDefaultModel);
                } else {
                  ai.setOpencodeModel(aiDefaultModel);
                }
              },
            },
          ]
        : []),
      { separator: true as const },
      {
        id: "ai-model-custom",
        label: "Custom model id…",
        run: () => {
          aiModelText = aiDefaultModel;
          aiModelOpen = true;
        },
      },
      { separator: true as const },
      {
        id: "ai-model-settings",
        label: "AI settings…",
        run: () => emitUiEvent("open-ai-settings"),
      },
    ];
    showMenuAtElement(event.currentTarget as HTMLElement, entries, {
      align: "end",
      above: true,
    });
  }

  function applyAiModel(): void {
    const text = aiModelText.trim();
    if (ai.config.backend === "openrouter") {
      ai.setOpenrouterModel(text.length > 0 ? text : undefined);
    } else {
      ai.setOpencodeModel(text.length > 0 ? text : undefined);
    }
    aiModelOpen = false;
  }

  /** Remote-tab device connection (`null` for local repos). */
  const deviceStatus = $derived(
    active?.deviceId ? remoteDevices.statusOf(active.deviceId) : null,
  );

  // -- watcher health chip (item 62) --------------------------------------------
  //
  // Only ever visible when the ACTIVE repo's watcher is NOT healthy: the
  // poll fallback shows a muted "watch: poll (fallback)" chip, a dead
  // watcher shows the danger chip with a Reopen action (reload the tab —
  // repo_open revives the watcher). Healthy + recommended is the default
  // backend state and emits nothing, so the bar stays calm the rest of the
  // time. The subscription is lazy and module-owned: the first StatusBar
  // mount starts it, for the app's lifetime.

  startWatchHealth();

  const watchState = $derived(repoId !== null ? watchHealth.state(repoId) : null);
  const watchKind = $derived(watchChipKind(watchState));
  const watchTitle = $derived.by(() => {
    if (watchState === null) return "";
    // `since` is ms; the formatter wants seconds (the 1s ticker below keeps
    // it fresh while the chip is visible).
    const when = formatRelativeTime(Math.floor(watchState.since / 1000), {
      now: Math.floor(now / 1000),
    });
    if (watchKind === "dead") {
      return `Watcher for this repository is not working (changed ${when}) — changes will not refresh until the repository is reopened`;
    }
    return `FS watcher unavailable for this repository (changed ${when}) — change detection polls instead`;
  });

  /** Reopen confirmation (a tab reload is the revive path). */
  let watchReopenOpen = $state(false);

  /** Revives the repo: closes the tab and re-opens its root (repo_open). */
  async function reopenWatchRepo(): Promise<void> {
    const tab = tabStore.active;
    if (tab === null) return;
    watchReopenOpen = false;
    try {
      await closeTab(tab.id);
      await openTab(tab.root);
    } catch (err) {
      // A refused open (e.g. the folder vanished) must not land on the
      // runtime error banner — the tab is already closed, so toast it.
      toast(`Reopen failed: ${describeError(err)}`, { kind: "error" });
    }
  }

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
    // The watch-health chip's relative "changed …" label shares the ticker.
    if (aiRows.length === 0 && watchKind === null) return;
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

  // -- AI usage budget chip (item 58) ------------------------------------------
  //
  // Soft budget: month-to-date tokens (from the LOCAL visible job history
  // — the 200-record activity ring, so runs that rotated out or other
  // machines' runs are not counted) against the configured monthly token
  // budget. Shown only when a budget is set; at >= 100% the chip turns
  // into a warning but runs are never blocked (soft by design — the
  // percentage is a local lower bound, not a bill).

  const budget = $derived.by(() => {
    const limit = appSettings.aiMonthlyTokenBudget;
    if (limit <= 0) return null;
    const start = budgetMonthStart(Date.now(), appSettings.aiBudgetResetDay);
    return budgetView(ai.jobs.usageSince(start).totalTokens, limit);
  });
  const budgetTitle = $derived(
    budget === null
      ? ""
      : `AI usage: ${budget.spent} of ${budget.budget} tokens this month (local history) — soft budget, runs are not blocked`,
  );
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
          class="sb-item inline-flex h-full flex-none items-center gap-ui-2 px-ui-2 font-medium transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary disabled:pointer-events-none wm-stacked:hidden"
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
       inside the fixed-height bar — the workspace above never moves.
       Stacked (§10 "branch + changes only"): hidden — the header's Sync and
       the panels' own surfaces carry the same information below the floor. -->
  <div
    class="running-ops flex min-w-0 flex-1 items-center gap-ui-3 overflow-hidden wm-stacked:hidden"
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
            {row.job.message ?? aiPhaseLabel(row.job.phase) ?? "working"}{#if row.job.agentic !==
              null &&
              row.job.agentic.controller.pending.length > 0} ·
            {row.job.agentic.controller.pending.length} approval{row.job.agentic.controller.pending.length === 1
              ? ""
              : "s"} needed{/if}
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

  <!-- Right: hook output + repo state + cadence + device connection.
       Stacked (§10 "branch + changes only"): hidden — the worst-state chip
       and every other item it hosts are reachable through their panels. -->
  <div class="extras flex flex-none items-center gap-ui-2 pr-ui-2 wm-stacked:hidden">
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
    {#if watchKind !== null && watchState !== null}
      <!-- Watcher health (item 62): ONLY shown when the repo is not
           healthy — muted for the poll fallback, danger + Reopen when the
           watcher is dead. Healthy + recommended emits nothing, so the bar
           stays calm otherwise. -->
      <span
        class="sb-watch inline-flex flex-none items-center gap-ui-2 rounded-full px-ui-3 py-px {watchKind === 'dead'
          ? 'bg-destructive/15 font-medium text-destructive'
          : 'text-muted-foreground'}"
        role="status"
        data-kind={watchKind}
      >
        <Icon icon={watchKind === "dead" ? iconAlert : iconRefresh} size={12} />
        {watchKind === "dead" ? "watch: dead" : "watch: poll (fallback)"}
        {#if watchKind === "dead"}
          <Hint label="Reopen this repository to revive its watcher">
            {#snippet child({ props })}
              <button
                {...props}
                class="inline-flex cursor-pointer items-center gap-ui-1 rounded-full border-0 bg-transparent p-0 font-medium text-inherit underline decoration-dotted underline-offset-2 hover:opacity-80 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-destructive"
                type="button"
                onclick={() => (watchReopenOpen = true)}
              >
                Reopen
              </button>
            {/snippet}
          </Hint>
        {/if}
      </span>
    {/if}
    {#if budget !== null}
      <!-- Soft AI budget (item 58): subtle while under, warning at 100%+
           — and only ever a warning: runs are never blocked. -->
      <Hint label={budgetTitle}>
        {#snippet child({ props })}
          <button
            {...props}
            class="sb-budget inline-flex h-full flex-none items-center gap-ui-2 rounded-full px-ui-2 tabular-nums transition-colors duration-[120ms] {budget.level ===
            "over"
              ? "bg-destructive/15 font-medium text-destructive"
              : "text-muted-foreground hover:bg-accent hover:text-accent-foreground"} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            aria-label={budgetTitle}
            onclick={() => emitUiEvent("open-ai-settings")}
          >
            {budgetLabel(budget.percent ?? 0)}
          </button>
        {/snippet}
      </Hint>
    {/if}
    {#if aiChipVisible}
      <!-- Workstream D §8.6 (deferred from §6.3): the global default AI
           model as a compact chip — click opens the change menu (server
           default / current id / custom id / settings). Per-repo run
           overrides stay in the composers' AiModelPicker carets. -->
      <span class="sb-ai relative inline-flex flex-none items-center" bind:this={aiChipEl}>
        <Hint label={aiChipTitle}>
          {#snippet child({ props })}
            <button
              {...props}
              class="sb-ai-chip inline-flex h-full max-w-40 flex-none items-center gap-ui-2 px-ui-2 transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
              type="button"
              aria-label={aiChipTitle}
              aria-haspopup="menu"
              onclick={openAiModelMenu}
            >
              <span class="inline-flex flex-none items-center text-primary" aria-hidden="true">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
                  <path
                    d="M12 2l2.09 6.26L20 10.35l-5.91 2.09L12 18.7l-2.09-6.26L4 10.35l5.91-2.09L12 2z"
                  />
                  <path d="M19 15l1.18 3.32L23.5 19.5l-3.32 1.18L19 24l-1.18-3.32L14.5 19.5l3.32-1.18L19 15z" />
                </svg>
              </span>
              <span class="min-w-0 truncate">{aiChipLabel}</span>
            </button>
          {/snippet}
        </Hint>
        {#if aiModelOpen}
          <!-- Free-text entry for the custom default id: portaled + clamped
               (AiModelPicker's compact grammar); Enter applies, Esc cancels. -->
          <div
            class="fixed left-0 top-0 z-60 flex items-center gap-1 rounded-lg border border-border bg-popover p-1.5 text-popover-foreground shadow-overlay"
            use:floating={{ anchor: aiChipEl ?? undefined, above: true, alignEnd: true }}
          >
            <input
              class="h-6 w-44 rounded-sm border border-border bg-background px-1.5 font-mono text-ui-meta text-foreground placeholder:text-muted-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
              placeholder="provider/model"
              spellcheck="false"
              aria-label="Custom model id"
              bind:value={aiModelText}
              onkeydown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  applyAiModel();
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  aiModelOpen = false;
                }
              }}
            />
            <button
              class="shrink-0 rounded-sm px-1.5 py-px text-ui-meta text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
              type="button"
              onclick={applyAiModel}
            >
              Apply
            </button>
            <button
              class="shrink-0 rounded-sm px-1.5 py-px text-ui-meta text-muted-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
              type="button"
              onclick={() => (aiModelOpen = false)}
            >
              Cancel
            </button>
          </div>
        {/if}
      </span>
    {/if}
    {#if repoId !== null}
      <!-- E1 §9.2: the GitHub chips (PR → Checks → Runs), before the
           cadence icon. One popover at a time across the cluster via the
           bindable open-chip state; <1024px collapses to the worst chip. -->
      <ForgeChips bind:open={openChip} />
      <!-- Workstream D §8.6: the cadence is icon + tooltip (the "auto 1m"
           text went away) and the click opens the cadence popover — the
           single place to change the git-fetch cadence (Workstream E1
           appends its GitHub section there). Always rendered for an open
           repo so "off" stays changeable; off renders dimmed. -->
      <Hint label={cadenceTitle}>
        {#snippet child({ props })}
          <button
            {...props}
            bind:this={cadenceAnchor}
            class="sb-autofetch inline-flex h-full flex-none items-center gap-ui-2 px-ui-2 transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {autofetchMinutes >
            0
              ? ""
              : "opacity-60"}"
            type="button"
            aria-label={cadenceTitle}
            aria-haspopup="dialog"
            aria-expanded={cadenceOpen}
            onclick={toggleCadence}
          >
            <Icon icon={iconRefresh} size={12} />
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

<!-- Watcher-dead reopen (item 62): a tab reload is the revive path —
     repo_close + repo_open re-registers the repository and restarts the
     watcher fresh (the backend emits the revival watch state). -->
<ConfirmDialog
  bind:open={() => watchReopenOpen, (v) => { if (!v) watchReopenOpen = false; }}
  title="Reopen this repository?"
  message="The file watcher is not working, so changes will not refresh. Reopening the tab restarts it (unsaved editor buffers are kept)."
  confirmLabel="Reopen"
  onConfirm={() => void reopenWatchRepo()}
/>

{#if cadenceOpen}
  <!-- Auto-fetch cadence popover (§8.6): radio rows change the git-fetch
       cadence in place; the section structure is what Workstream E1
       extends — §9.3 wants its GitHub cadence rendered here next to the
       git-fetch section. Portaled (bottom-of-screen anchor opens upward
       via `above`). -->
  <div
    bind:this={cadenceEl}
    class="cadence-popover fixed z-60 flex w-72 max-w-[calc(100vw-1rem)] flex-col gap-2 rounded-lg border border-border bg-popover p-3 text-ui-base text-popover-foreground shadow-md"
    role="dialog"
    aria-label="Auto-fetch cadence"
    use:floating={{ anchor: cadenceAnchor ?? undefined, above: true, alignEnd: true, gap: 6 }}
  >
    <div class="flex items-center gap-2">
      <span class="min-w-0 flex-1 text-ui-meta font-semibold">Auto-fetch cadence</span>
      <button
        class="inline-flex size-5 cursor-pointer items-center justify-center rounded-sm p-0 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        type="button"
        aria-label="Close auto-fetch cadence"
        onclick={() => {
          cadenceOpen = false;
          cadenceAnchor?.focus();
        }}
      >
        <Icon icon={iconClose} size={12} />
      </button>
    </div>
    <section aria-label="Git fetch cadence" class="flex flex-col gap-0.5">
      <span class="text-ui-micro font-semibold uppercase tracking-[0.04em] text-muted-foreground">Git fetch</span>
      <!-- Pressed toggle-buttons rather than a role="radiogroup": a radio
           group wants roving arrow-key focus, and these are plain dialog
           rows — the dot + pressed state carry the selection honestly. -->
      <div role="group" aria-label="Fetch every">
        {#each CADENCE_OPTIONS as minutes (minutes)}
          <button
            class="flex w-full cursor-pointer items-center gap-2 rounded-sm px-1.5 py-1 text-ui-meta hover:bg-accent aria-pressed:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            aria-pressed={autofetchMinutes === minutes}
            onclick={() => setCadence(minutes)}
          >
            <span
              class="inline-flex size-3 flex-none items-center justify-center rounded-full border {autofetchMinutes ===
              minutes
                ? "border-primary"
                : "border-muted-foreground/50"}"
              aria-hidden="true"
            >
              {#if autofetchMinutes === minutes}
                <span class="size-1.5 rounded-full bg-primary"></span>
              {/if}
            </span>
            {minutes === 0 ? "Off" : minutes === 1 ? "Every minute" : `Every ${minutes} minutes`}
          </button>
        {/each}
      </div>
    </section>
    <p class="text-ui-micro leading-[1.4] text-muted-foreground">
      {cadenceIsOverride
        ? "Per-repository override. \"Off\" clears it and follows the app default."
        : `Following the app default${appSettings.defaultAutofetchMinutes > 0 ? ` (${appSettings.defaultAutofetchMinutes} min)` : " (off)"}.`}
    </p>
    <!-- Workstream E1 (§9.3): the "GitHub" cadence section renders next to
         the git-fetch one — the active repo's snapshot cadence in seconds
         (30 s floor; 0 = off, matching the plan's git-fetch semantics).
         Inactive-open repos always poll at the fixed 5-minute rate. -->
    <section aria-label="GitHub cadence" class="flex flex-col gap-0.5">
      <span class="text-ui-micro font-semibold uppercase tracking-[0.04em] text-muted-foreground">GitHub</span>
      <div role="group" aria-label="GitHub snapshot every">
        {#each FORGE_CADENCE_OPTIONS as seconds (seconds)}
          <button
            class="flex w-full cursor-pointer items-center gap-2 rounded-sm px-1.5 py-1 text-ui-meta hover:bg-accent aria-pressed:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            aria-pressed={appSettings.forgeSnapshotSeconds === seconds}
            onclick={() => setForgeCadence(seconds)}
          >
            <span
              class="inline-flex size-3 flex-none items-center justify-center rounded-full border {appSettings.forgeSnapshotSeconds ===
              seconds
                ? "border-primary"
                : "border-muted-foreground/50"}"
              aria-hidden="true"
            >
              {#if appSettings.forgeSnapshotSeconds === seconds}
                <span class="size-1.5 rounded-full bg-primary"></span>
              {/if}
            </span>
            {seconds === 0
              ? "Off"
              : seconds < 60
                ? `Every ${seconds} seconds`
                : seconds === 60
                  ? "Every minute"
                  : `Every ${Math.round(seconds / 60)} minutes`}
          </button>
        {/each}
      </div>
    </section>
    <p class="text-ui-micro leading-[1.4] text-muted-foreground">
      {activeSnapshotIntervalMs(appSettings.forgeSnapshotSeconds) > 0
        ? "Pull requests, checks and runs for the active repository. Other open repositories poll every 5 minutes."
        : "Background GitHub polling is off — the chips refresh when you open their popovers."}
    </p>
    <Button
      class="h-6 flex-none self-start rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
      variant="outline"
      type="button"
      onclick={() => {
        if (repoId !== null) emitUiEvent("focus-panel", { panel: "remotes", repoId });
        cadenceOpen = false;
        cadenceAnchor?.focus();
      }}
    >More fetch settings — Remotes panel</Button>
  </div>
{/if}
</div>
