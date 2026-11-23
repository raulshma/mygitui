<script lang="ts">
  /**
   * GitHub footer chips (E1, plan §9.2) — the three right-cluster chips
   * (PR → Checks → Runs) and their popovers, rendered inside the StatusBar.
   *
   * Each chip is a button (icon + color + text, never color alone; the
   * aria-label carries the full state text) opening ONE popover at a time —
   * exclusivity with the cadence popover is wired through the bindable
   * `open` prop (StatusBar clears it when the cadence chip opens, and
   * closes the cadence when a chip opens). Escape/outside close and focus
   * return follow the StatusBar's own popover idiom.
   *
   * Chip states derive from the poll scheduler's snapshot lanes through
   * `$lib/forge/chips.svelte.ts` — pure `indication.svelte.ts` states.
   * Degraded states (no gh / unauthed / non-GitHub remote / offline /
   * rate-limited) open a GUIDANCE popover with copyable install/login
   * commands instead of raw error strings (§9.1).
   *
   * Popover writes (§9.2): refresh, open-in-panel, checkout, rerun-failed,
   * cancel. Panel-only writes (merge, create PR, download log) stay in the
   * panel. Below 1024px window width (narrow/stacked) the three chips
   * collapse into ONE worst-state chip + attention count whose popover
   * lists all three objects — a failure never hides.
   */
  import { untrack } from "svelte";
  import type { IconifyIcon } from "@iconify/types";
  import { tabStore, refreshStatus } from "$lib/stores/tabs.svelte";
  import { forgeStore, openExternal } from "$lib/stores/forge.svelte";
  import { forgeChips } from "$lib/forge/chips.svelte";
import { checkoutRefusal } from "$lib/forge/checkoutGuard";
  import {
    degradedGuidance,
    prHeadDiverged,
    retryLabel,
    summarize,
    type ChipId,
    type ForgeChipState,
  } from "$lib/forge/indication.svelte";
  import { widthMode } from "$lib/layout/widthMode.svelte";
  import { emitUiEvent } from "$lib/palette/events";
  import { runIsActive, runHasFailures, runStateLabel } from "./forgeModel";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { floating } from "$lib/components/overlay/floating";
  import { toast } from "$lib/toast";

  let {
    open = $bindable(null),
  }: {
    /** The currently open chip popover (`null` = none). */
    open?: ChipId | null;
  } = $props();

  const iconPr = $derived(appIcon("pull-request") ?? appIcon("merge-type")!);
  const iconChecks = $derived(appIcon("check-circle")!);
  const iconRuns = $derived(appIcon("play-arrow")!);
  const iconAlert = $derived(appIcon("triangle-alert")!);
  const iconError = $derived(appIcon("error")!);
  const iconCheck = $derived(appIcon("check")!);
  const iconClose = $derived(appIcon("close")!);
  const iconUpload = $derived(appIcon("cloud-upload")!);
  const iconExternal = $derived(appIcon("open-in-new")!);

  const repoId = $derived(tabStore.active?.id ?? null);
  /** <1024px window: one worst-state chip + attention count (§9.2). */
  const collapsed = $derived(
    widthMode.current === "narrow" || widthMode.current === "stacked",
  );

  /** 1s ticker for the running chips' elapsed labels (only while needed). */
  let now = $state(Date.now());
  const chips = $derived(repoId === null ? null : forgeChips.chipsFor(repoId, now));
  /** The (id, state) pairs the chips render, in PR → Checks → Runs order. */
  const chipList = $derived(
    chips === null
      ? []
      : ([
          ["pr", chips.pr],
          ["checks", chips.checks],
          ["runs", chips.runs],
        ] as const),
  );
  const chipsNeedTick = $derived(
    chipList.some(([, state]) => state.kind === "running"),
  );
  $effect(() => {
    if (!chipsNeedTick) return;
    now = Date.now();
    const timer = setInterval(() => (now = Date.now()), 1_000);
    return () => clearInterval(timer);
  });

  let chipAnchor = $state<HTMLElement | null>(null);
  let popoverEl = $state<HTMLElement | null>(null);

  function toggleChip(id: ChipId, event: MouseEvent): void {
    const trigger = event.currentTarget as HTMLElement;
    if (open === id) {
      open = null;
      chipAnchor?.focus();
    } else {
      chipAnchor = trigger;
      open = id;
    }
  }

  function closeChip(): void {
    open = null;
    chipAnchor?.focus();
  }

  // The popover is portaled geometry-wise (fixed + floating, like the
  // cadence popover): outside-click closes, Escape closes and restores the
  // trigger's focus.
  $effect(() => {
    if (open === null) return;
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target as Node | null;
      if (popoverEl?.contains(target) || chipAnchor?.contains(target)) return;
      open = null;
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.stopPropagation();
        closeChip();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  });

  /** Full-state text of a chip (the aria-label / tooltip body). */
  function chipTitle(id: ChipId, state: ForgeChipState): string {
    const which = id === "pr" ? "Pull request" : id === "checks" ? "Checks" : "Run";
    const base =
      state.text === "" ? `${which}: none` : `${which}: ${state.text}`;
    if (state.kind === "degraded") {
      const retry = retryLabel(state.retryInMs);
      return `${base}${retry === "" ? "" : ` (${retry})`} — click for guidance`;
    }
    return `${base} — click for details`;
  }

  /** Tone class per chip state kind (icon + color + text grammar). */
  function chipTone(state: ForgeChipState): string {
    switch (state.kind) {
      case "ok":
      case "running":
        return "text-primary-ink";
      case "failing":
        return "text-destructive";
      case "warning":
        return "text-[var(--app-modified)]";
      default:
        return "text-muted-foreground";
    }
  }

  /** Icon glyph per state (null = the running spinner takes the slot). */
  function chipIcon(state: ForgeChipState, which: ChipId): IconifyIcon | null {
    if (state.kind === "running") return null;
    if (state.kind === "degraded") return iconError;
    if (state.kind === "failing") return iconAlert;
    if (state.kind === "unpushed") return iconUpload;
    if (which === "pr") return iconPr;
    if (which === "checks") return iconChecks;
    return iconRuns;
  }

  // -- popover data -----------------------------------------------------------

  const snapshot = $derived(
    repoId === null ? null : forgeStore.snapshotStaleFor(repoId),
  );
  const pr = $derived(snapshot?.pr ?? null);
  /** The checks the popover leads with: HEAD check-runs (the chip's data). */
  const headChecks = $derived(snapshot?.head_checks ?? []);
  const diverged = $derived(prHeadDiverged(snapshot));
  const prHeadChecks = $derived(snapshot?.pr_head_checks ?? null);
  const run = $derived(snapshot?.run ?? null);
  const runDetail = $derived(
    run !== null && repoId !== null
      ? forgeStore.detailFor(repoId, run.database_id)
      : null,
  );
  const runDetailLoading = $derived(
    run !== null && repoId !== null
      ? forgeStore.detailLoadingFor(repoId, run.database_id)
      : false,
  );
  const runBusy = $derived(
    run !== null && repoId !== null
      ? forgeStore.isRunBusy(repoId, run.database_id)
      : false,
  );

  // Opening the runs popover fetches the run's jobs/steps on demand (the
  // chip itself only carries the run row).
  $effect(() => {
    if (open !== "runs" || run === null || repoId === null) return;
    const targetRepo = repoId;
    const id = run.database_id;
    untrack(() => void forgeStore.loadRunDetail(targetRepo, id));
  });

  // -- popover actions (writes allowed from popovers, §9.2) -------------------

  async function refreshNow(): Promise<void> {
    if (repoId === null) return;
    await forgeChips.refresh(repoId);
  }

  async function checkoutBranchPr(): Promise<void> {
    if (repoId === null || pr === null) return;
    // E2 §9.4: the popover checkout HARD-REFUSES on a dirty worktree (the
    // panel's Checkout offers a named stash instead) — the toast links to
    // the STATUS panel where the changes can be handled.
    const refusal = checkoutRefusal(
      tabStore.tabs.find((tab) => tab.id === repoId)?.status ?? null,
    );
    if (refusal !== null) {
      toast(`Cannot check out #${pr.number}: ${refusal}.`, {
        kind: "error",
        action: {
          label: "Open Status",
          run: () => emitUiEvent("focus-panel", { panel: "status", repoId }),
        },
      });
      return;
    }
    try {
      await forgeStore.checkoutPr(repoId, pr.number);
      await refreshStatus(repoId);
      toast(`Checked out #${pr.number}`, { kind: "success" });
    } catch (err) {
      toast(`Checkout failed: ${describe(err)}`, { kind: "error" });
    }
  }

  async function rerunFailed(): Promise<void> {
    if (repoId === null || run === null) return;
    try {
      await forgeStore.rerunRun(repoId, run.database_id, true);
      toast(`Rerunning failed jobs of #${run.number}`, { kind: "success" });
    } catch (err) {
      toast(describe(err), { kind: "error" });
    }
  }

  async function cancelRun(): Promise<void> {
    if (repoId === null || run === null) return;
    try {
      await forgeStore.cancelRun(repoId, run.database_id);
      toast(`Cancelled run #${run.number}`, { kind: "success" });
    } catch (err) {
      toast(describe(err), { kind: "error" });
    }
  }

  function describe(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  function openInPanel(): void {
    if (repoId === null) return;
    emitUiEvent("focus-panel", { panel: "forge", repoId });
    open = null;
  }

  function openRunView(): void {
    if (repoId === null || run === null) return;
    emitUiEvent("workflow-run-open", {
      repoId,
      runId: run.database_id,
    });
    open = null;
  }

  async function openDocs(url: string): Promise<void> {
    const ok = await openExternal(url);
    if (!ok) toast(`Could not open ${url}`, { kind: "error" });
  }

  async function copyCommand(command: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(command);
      toast(`Copied "${command}" to the clipboard`, { kind: "success" });
    } catch {
      toast("Clipboard is not available", { kind: "error" });
    }
  }

  /** The degraded guidance of the open chip (null → data popover instead). */
  const guidance = $derived.by(() => {
    if (chips === null || open === null) return null;
    const state =
      open === "pr" ? chips.pr : open === "checks" ? chips.checks : chips.runs;
    if (state.kind !== "degraded") return null;
    return degradedGuidance(state.degraded, state.message);
  });
</script>

{#if chips !== null && repoId !== null}
  {#if collapsed}
    <!-- §9.2 <1024px: ONE worst-state chip + attention count. A failure
         never hides — the collapse rank puts failing first. -->
    {#if chips.worst !== null}
      {@const worstId = chips.worst.id}
      {@const state = chips.worst.state}
      <Hint label={chipTitle(worstId, state)}>
        {#snippet child({ props })}
          <button
            {...props}
            class="sb-forge inline-flex h-full max-w-44 flex-none items-center gap-ui-2 px-ui-2 tabular-nums transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {chipTone(state)}"
            type="button"
            aria-label={chipTitle(worstId, state)}
            aria-haspopup="dialog"
            aria-expanded={open === worstId}
            data-chip={worstId}
            onclick={(e) => toggleChip(worstId, e)}
          >
            <span
              class="inline-flex size-3.5 flex-none items-center justify-center"
              aria-hidden="true"
            >
              {#if state.kind === "running"}
                <Spinner class="size-3" role="presentation" aria-hidden="true" />
              {:else if chipIcon(state, worstId) !== null}
                <Icon icon={chipIcon(state, worstId)!} size={12} />
              {/if}
            </span>
            <span class="min-w-0 truncate">{state.text}</span>
            {#if chips.attentionCount > 1}
              <span
                class="inline-flex flex-none items-center rounded-full bg-destructive/15 px-1 text-[10px] leading-[1.5] font-medium text-destructive"
                aria-label={`${chips.attentionCount} items need attention`}
              >
                {chips.attentionCount}
              </span>
            {/if}
          </button>
        {/snippet}
      </Hint>
    {/if}
  {:else}
    {#each chipList as [id, state] (id)}
      <Hint label={chipTitle(id, state)}>
        {#snippet child({ props })}
          <button
            {...props}
            class="sb-forge inline-flex h-full max-w-44 flex-none items-center gap-ui-2 px-ui-2 tabular-nums transition-colors duration-[120ms] hover:bg-accent hover:text-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {chipTone(state)}"
            type="button"
            aria-label={chipTitle(id, state)}
            aria-haspopup="dialog"
            aria-expanded={open === id}
            data-chip={id}
            onclick={(e) => toggleChip(id, e)}
          >
            <span
              class="inline-flex size-3.5 flex-none items-center justify-center"
              aria-hidden="true"
            >
              {#if state.kind === "running"}
                <Spinner class="size-3" role="presentation" aria-hidden="true" />
              {:else if chipIcon(state, id) !== null}
                <Icon icon={chipIcon(state, id)!} size={12} />
              {/if}
            </span>
            <span class="min-w-0 truncate">{state.text}</span>
          </button>
        {/snippet}
      </Hint>
    {/each}
  {/if}

  {#if open !== null}
    <!-- One chip popover at a time (bindable `open` is exclusive with the
         StatusBar's cadence popover). -->
    <div
      bind:this={popoverEl}
      class="forge-chip-popover fixed z-60 flex max-h-80 w-80 max-w-[calc(100vw-1rem)] flex-col gap-2 overflow-y-auto rounded-lg border border-border bg-popover p-3 text-ui-base text-popover-foreground shadow-md"
      role="dialog"
      aria-label="GitHub status"
      use:floating={{ anchor: chipAnchor ?? undefined, above: true, alignEnd: true, gap: 6 }}
    >
      {#if guidance !== null}
        <!-- §9.1: degraded states show guidance with copyable commands —
             never raw contextError strings. -->
        {@const command = guidance.command}
        {@const docsUrl = guidance.docsUrl}
        <div class="flex items-start gap-2">
          <span class="mt-0.5 inline-flex flex-none text-destructive" aria-hidden="true">
            <Icon icon={iconError} size={14} />
          </span>
          <div class="min-w-0 flex-1">
            <p class="text-ui-meta font-semibold">{guidance.title}</p>
            <p class="mt-1 text-ui-meta leading-[1.45] text-muted-foreground">
              {guidance.message}
            </p>
            {#if command !== null}
              <div class="mt-2 flex items-center gap-1">
                <code
                  class="min-w-0 flex-1 truncate rounded-sm border border-border bg-muted px-1.5 py-0.5 font-mono text-ui-micro text-foreground"
                >{command}</code>
                <Button
                  class="h-6 flex-none rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
                  variant="outline"
                  type="button"
                  onclick={() => void copyCommand(command)}
                >Copy</Button>
              </div>
            {/if}
            {#if docsUrl !== null}
              <button
                class="mt-2 inline-flex cursor-pointer items-center gap-1 text-ui-micro text-primary-ink underline decoration-dotted underline-offset-2 hover:opacity-80 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                type="button"
                onclick={() => void openDocs(docsUrl)}
              >
                <span class="inline-flex flex-none" aria-hidden="true">
                  <Icon icon={iconExternal} size={11} />
                </span>
                Install docs
              </button>
            {/if}
          </div>
          <button
            class="inline-flex size-5 flex-none cursor-pointer items-center justify-center rounded-sm p-0 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            aria-label="Close GitHub status"
            onclick={closeChip}
          >
            <Icon icon={iconClose} size={12} />
          </button>
        </div>
      {:else if collapsed}
        <!-- §9.2 <1024px: the collapsed chip's popover LISTS ALL THREE
             objects (a failure never hides) plus the worst chip's actions. -->
        <div class="flex items-center justify-between gap-2">
          <p class="min-w-0 flex-1 text-ui-meta font-semibold">GitHub status</p>
          <button
            class="inline-flex size-5 flex-none cursor-pointer items-center justify-center rounded-sm p-0 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            aria-label="Close GitHub status"
            onclick={closeChip}
          >
            <Icon icon={iconClose} size={12} />
          </button>
        </div>
        <ul class="flex flex-col gap-1">
          {#each chipList as [id, state] (id)}
            <li class="flex items-center gap-2 text-ui-meta">
              <span
                class="inline-flex w-3 flex-none items-center justify-center {chipTone(state)}"
                aria-hidden="true"
              >
                {#if state.kind === "running"}
                  <Spinner class="size-2.5" role="presentation" aria-hidden="true" />
                {:else if chipIcon(state, id) !== null}
                  <Icon icon={chipIcon(state, id)!} size={11} />
                {/if}
              </span>
              <span class="w-14 flex-none text-ui-micro uppercase tracking-[0.04em] text-muted-foreground">
                {id === "pr" ? "PR" : id}
              </span>
              <span class="min-w-0 flex-1 truncate tabular-nums">
                {state.text === "" ? "none" : state.text}
                {#if id === "pr" && pr !== null && state.text !== "" && pr.title !== ""}
                  <span class="text-muted-foreground">· {pr.title}</span>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
        {#if snapshot?.pushed === false}
          <p class="flex items-center gap-1 text-ui-micro text-[var(--app-modified)]">
            <span class="inline-flex flex-none" aria-hidden="true">
              <Icon icon={iconUpload} size={11} />
            </span>
            HEAD is not on the remote — push to start its checks.
          </p>
        {/if}
        <div class="flex flex-wrap gap-1">
          {#if pr !== null && pr.state === "OPEN"}
            <Button
              class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
              variant="outline"
              type="button"
              onclick={() => void checkoutBranchPr()}
            >Checkout</Button>
          {/if}
          {#if run !== null && runIsActive(run) && !runBusy}
            <Button
              class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
              variant="outline"
              type="button"
              onclick={() => void cancelRun()}
            >Cancel run</Button>
          {/if}
          {#if run !== null && runHasFailures(run) && !runBusy}
            <Button
              class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
              variant="outline"
              type="button"
              onclick={() => void rerunFailed()}
            >Rerun failed</Button>
          {/if}
          <Button
            class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
            variant="outline"
            type="button"
            onclick={openInPanel}
          >Open in panel</Button>
          <Button
            class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
            variant="outline"
            type="button"
            onclick={() => void refreshNow()}
          >Refresh</Button>
        </div>
      {:else if open === "pr"}
        <!-- PR chip popover: title, review decision, checks summary,
             checkout / open-in-panel (§9.2). -->
        {#if pr !== null}
          <div class="flex items-start justify-between gap-2">
            <p class="min-w-0 flex-1 text-ui-meta font-semibold">
              #{pr.number} · {pr.title}
            </p>
            <button
              class="inline-flex size-5 flex-none cursor-pointer items-center justify-center rounded-sm p-0 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
              type="button"
              aria-label="Close pull request details"
              onclick={closeChip}
            >
              <Icon icon={iconClose} size={12} />
            </button>
          </div>
          <div class="flex flex-wrap gap-1 text-ui-micro text-muted-foreground">
            <span class="rounded-full bg-muted px-2 py-px">
              {pr.state}{pr.is_draft ? " · draft" : ""}
            </span>
            {#if pr.review_decision !== ""}
              <span class="rounded-full bg-muted px-2 py-px">
                {pr.review_decision.replaceAll("_", " ").toLowerCase()}
              </span>
            {/if}
          </div>
          <p class="text-ui-micro text-muted-foreground">
            {summarize(headChecks)} on local HEAD{diverged && prHeadChecks !== null
              ? ` · PR head: ${summarize(prHeadChecks)}`
              : ""}
          </p>
          <div class="flex flex-wrap gap-1">
            {#if pr.state === "OPEN"}
              <Button
                class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
                variant="outline"
                type="button"
                onclick={() => void checkoutBranchPr()}
              >Checkout</Button>
            {/if}
            <Button
              class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
              variant="outline"
              type="button"
              onclick={openInPanel}
            >Open in panel</Button>
            <Button
              class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
              variant="outline"
              type="button"
              onclick={() => void refreshNow()}
            >Refresh</Button>
          </div>
        {:else}
          <p class="text-ui-meta text-muted-foreground">
            No pull request for this branch.
          </p>
        {/if}
      {:else if open === "checks"}
        <!-- Checks chip popover: HEAD check-runs; a PR-head row appears when
             the two diverge (§9.2). -->
        <div class="flex items-center justify-between gap-2">
          <p class="min-w-0 flex-1 text-ui-meta font-semibold">Checks on local HEAD</p>
          <button
            class="inline-flex size-5 flex-none cursor-pointer items-center justify-center rounded-sm p-0 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            aria-label="Close checks details"
            onclick={closeChip}
          >
            <Icon icon={iconClose} size={12} />
          </button>
        </div>
        {#if snapshot?.pushed === false}
          <p class="flex items-center gap-1 text-ui-micro text-[var(--app-modified)]">
            <span class="inline-flex flex-none" aria-hidden="true">
              <Icon icon={iconUpload} size={11} />
            </span>
            HEAD is not on the remote — push to start its checks.
          </p>
        {/if}
        {#if headChecks.length === 0}
          <p class="text-ui-micro text-muted-foreground">No check-runs reported.</p>
        {:else}
          <ul class="flex flex-col gap-0.5">
            {#each headChecks as check (check.name)}
              <li class="flex items-center gap-2 text-ui-meta">
                <span
                  class="inline-flex w-3 flex-none justify-center {check.state === 'pass'
                    ? 'text-primary-ink'
                    : check.state === 'fail'
                      ? 'text-destructive'
                      : 'text-muted-foreground'}"
                  aria-hidden="true"
                >
                  {#if check.state === "pass"}
                    <Icon icon={iconCheck} size={11} />
                  {:else if check.state === "fail"}
                    <Icon icon={iconAlert} size={11} />
                  {:else if check.state === "pending"}
                    <Spinner class="size-2.5" role="presentation" aria-hidden="true" />
                  {:else}
                    –
                  {/if}
                </span>
                <span class="min-w-0 flex-1 truncate">{check.name}</span>
                <span class="flex-none text-ui-micro text-muted-foreground">
                  {check.state}
                </span>
              </li>
            {/each}
          </ul>
        {/if}
        {#if diverged && prHeadChecks !== null}
          <div class="border-t border-border pt-1">
            <p class="text-ui-micro font-semibold text-muted-foreground">
              PR head (diverged from local HEAD)
            </p>
            <p class="text-ui-micro text-muted-foreground">
              {summarize(prHeadChecks)}
            </p>
          </div>
        {/if}
        <div class="flex flex-wrap gap-1">
          <Button
            class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
            variant="outline"
            type="button"
            onclick={openInPanel}
          >Open in panel</Button>
          <Button
            class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
            variant="outline"
            type="button"
            onclick={() => void refreshNow()}
          >Refresh</Button>
        </div>
      {:else if open === "runs"}
        <!-- Runs chip popover: run summary + jobs; rerun-failed / cancel /
             open run view (§9.2). -->
        {#if run !== null}
          <div class="flex items-start justify-between gap-2">
            <p class="min-w-0 flex-1 text-ui-meta font-semibold">
              #{run.number} · {run.display_title || run.name}
            </p>
            <button
              class="inline-flex size-5 flex-none cursor-pointer items-center justify-center rounded-sm p-0 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
              type="button"
              aria-label="Close run details"
              onclick={closeChip}
            >
              <Icon icon={iconClose} size={12} />
            </button>
          </div>
          <p class="text-ui-micro text-muted-foreground">
            {runStateLabel(run)} · {run.head_branch}
          </p>
          {#if runDetailLoading}
            <p class="flex items-center gap-1 text-ui-micro text-muted-foreground">
              <Spinner class="size-2.5" role="presentation" aria-hidden="true" />
              Loading jobs…
            </p>
          {:else if runDetail !== null && runDetail.jobs.length > 0}
            <ul class="flex flex-col gap-0.5">
              {#each runDetail.jobs as job (job.database_id)}
                <li class="flex items-center gap-2 text-ui-meta">
                  <span
                    class="inline-flex w-3 flex-none justify-center {job.conclusion ===
                    'success'
                      ? 'text-primary-ink'
                      : ['failure', 'timed_out', 'action_required', 'startup_failure'].includes(
                          job.conclusion,
                        )
                        ? 'text-destructive'
                        : 'text-muted-foreground'}"
                    aria-hidden="true"
                  >
                    {#if job.conclusion === "success"}
                      <Icon icon={iconCheck} size={11} />
                    {:else if job.status !== "completed"}
                      <Spinner class="size-2.5" role="presentation" aria-hidden="true" />
                    {:else if job.conclusion !== ""}
                      <Icon icon={iconAlert} size={11} />
                    {/if}
                  </span>
                  <span class="min-w-0 flex-1 truncate">{job.name}</span>
                  <span class="flex-none text-ui-micro text-muted-foreground">
                    {job.status === "completed"
                      ? job.conclusion.replaceAll("_", " ") || "done"
                      : job.status.replaceAll("_", " ")}
                  </span>
                </li>
              {/each}
            </ul>
          {:else if runDetail !== null}
            <p class="text-ui-micro text-muted-foreground">No jobs reported.</p>
          {/if}
          <div class="flex flex-wrap gap-1">
            {#if runIsActive(run) && !runBusy}
              <Button
                class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
                variant="outline"
                type="button"
                onclick={() => void cancelRun()}
              >Cancel</Button>
            {/if}
            {#if runHasFailures(run) && !runBusy}
              <Button
                class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
                variant="outline"
                type="button"
                onclick={() => void rerunFailed()}
              >Rerun failed</Button>
            {/if}
            <Button
              class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
              variant="outline"
              type="button"
              onclick={openRunView}
            >Open run view</Button>
            <Button
              class="h-6 rounded-sm px-2 text-ui-micro leading-[1.5] font-normal"
              variant="outline"
              type="button"
              onclick={() => void refreshNow()}
            >Refresh</Button>
          </div>
        {:else}
          <p class="text-ui-meta text-muted-foreground">
            No workflow runs for this branch.
          </p>
        {/if}
      {/if}
    </div>
  {/if}
{/if}
