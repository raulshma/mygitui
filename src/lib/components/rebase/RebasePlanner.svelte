<script lang="ts">
  /**
   * Interactive rebase planner (M3 FE lane E2).
   *
   * Mounted by `HistoryView` ("Rebase from here") and, later, by E4/M3
   * integration — props are the stable surface:
   * `{ repoId, baseSha, onClose, onFinished }`.
   *
   * Phases:
   *   - `planning`: loads `HEAD..baseSha` (exclusive of `baseSha`; `null` =
   *     every commit reachable from HEAD) via `streamLog` with `refs: ["HEAD"]`,
   *     stopping at `baseSha`. Rows render oldest-first (git todo order),
   *     default action `pick`; rows reorder by HTML5 drag-and-drop or the
   *     per-row ↑/↓ buttons, each row's action is a select (pick/squash/
   *     fixup/drop/edit/reword — squash/fixup disabled on the first row) and
   *     `reword` shows an inline message textarea. `validatePlan` gates the
   *     Start button with an inline summary.
   *   - `running`: `rebase_start` handed off to the shared `RebaseMonitor`
   *     (polls `rebase_state` every 500ms until `!active`). Shows "step k/n
   *     (action sha)", rewritten count, and — when the backend pauses with
   *     conflicts — an inline conflict list with Continue/Abort (the editor
   *     itself lives in the status panel; full ConflictEditor integration is
   *     E1/E4's). Cancel = `rebase_abort` after `window.confirm`.
   *   - `done`: completion summary; Close.
   *
   * A11y: rendered through the shadcn ui/dialog primitives — `role="dialog"`
   * + `aria-modal`, focus trap and focus restore come from bits-ui. Esc /
   * outside-click close in the planning phase only (never mid-rebase).
   * Closing the dialog never stops a live rebase — the shared monitor keeps
   * watching and a reopened planner re-attaches to it.
   */
  import { rebaseStart, streamLog, autosquashPlan } from "$lib/ipc/client";
  import type { CommitInfo, RebaseStep } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { rebaseMonitor } from "./rebaseStore.svelte";
  import {
    REBASE_ACTIONS,
    buildPlan,
    cutAtBase,
    moveRow,
    toRebaseSteps,
    validatePlan,
    type PlanRow,
    type RebaseAction,
  } from "./plannerModel";

  let {
    repoId,
    baseSha,
    onClose,
    onFinished,
  }: {
    repoId: string;
    /** Rebase HEAD..baseSha (exclusive); `null` = every commit from root. */
    baseSha: string | null;
    onClose: () => void;
    /** Called once when a watched rebase settles (finished or aborted). */
    onFinished: () => void;
  } = $props();

  type Phase = "planning" | "running" | "done";

  const monitor = rebaseMonitor;

  let rows = $state<PlanRow[]>([]);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let phase = $state<Phase>("planning");
  let starting = $state(false);

  let dragFrom = $state<number | null>(null);
  let dragOver = $state<number | null>(null);

  const validationError = $derived(validatePlan(toRebaseSteps(rows)));
  const canStart = $derived(
    phase === "planning" &&
      !starting &&
      !loading &&
      loadError === null &&
      rows.length > 0 &&
      validationError === null,
  );
  const shortBase = $derived(baseSha === null ? "root" : baseSha.slice(0, 7));

  /** M10: replace the plan with the backend's autosquash ordering. */
  let autosquashing = $state(false);
  async function onAutosquash(): Promise<void> {
    if (autosquashing || baseSha === null || loading) return;
    autosquashing = true;
    try {
      const steps = await autosquashPlan(repoId, baseSha);
      const summaryOf = new Map(rows.map((row) => [row.sha, row.summary]));
      rows = steps.map((step) => ({
        sha: step.sha,
        summary: summaryOf.get(step.sha) ?? step.new_message ?? step.sha.slice(0, 7),
        action: step.action as PlanRow["action"],
        message: step.new_message ?? "",
      }));
      toast(`Autosquash: ${steps.filter((s) => s.action !== "pick").length} fixup/squash step(s) placed`, { kind: "success" });
    } catch (err) {
      toast(
        `Autosquash failed: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      autosquashing = false;
    }
  }

  /** "step k/n (action sha)" for the run view. */
  const progress = $derived.by(() => {
    const state = monitor.state;
    if (state === null || state.plan.length === 0) return null;
    const step = state.plan[state.current] ?? null;
    return {
      k: Math.min(state.current + 1, state.plan.length),
      n: state.plan.length,
      label: step === null ? "…" : `${step.action} ${step.sha.slice(0, 7)}`,
      rewritten: state.rewritten.length,
    };
  });

  // -- load HEAD..baseSha -----------------------------------------------------

  let loadToken = 0;

  $effect(() => {
    void repoId;
    void baseSha;
    const token = ++loadToken;
    loading = true;
    loadError = null;
    rows = [];
    let acc: CommitInfo[] = [];
    let settled = false;
    const finish = (): void => {
      if (token !== loadToken || settled) return;
      settled = true;
      rows = buildPlan(acc);
      loading = false;
    };
    streamLog(
      repoId,
      {
        text: null,
        regex: false,
        author: null,
        path: null,
        after_unix: null,
        before_unix: null,
        refs: ["HEAD"],
        follow: false,
      },
      (page) => {
        if (token !== loadToken || settled) return;
        acc = acc.concat(page.commits);
        if (baseSha !== null) {
          const cut = cutAtBase(acc, baseSha);
          if (cut !== null) {
            acc = cut;
            finish();
          }
        }
      },
    )
      .then(finish)
      .catch((err: unknown) => {
        if (token !== loadToken || settled) return;
        settled = true;
        loading = false;
        loadError = err instanceof Error ? err.message : String(err);
      });
    return () => {
      loadToken++;
    };
  });

  // -- monitor attach + phase transitions ---------------------------------------

  // Attach to (or confirm) monitoring; the monitor self-stops when idle, so
  // this is cheap while planning and re-engages instantly on Start.
  $effect(() => {
    monitor.start(repoId);
  });

  // A rebase that started before this dialog opened (or was resumed) flips
  // straight into the run view.
  $effect(() => {
    if (phase === "planning" && monitor.active) phase = "running";
  });

  // The monitor stopped polling while we were in the run view → it settled.
  $effect(() => {
    if (phase === "running" && !monitor.polling && !monitor.active) {
      phase = "done";
      onFinished();
    }
  });

  // Teardown: a live rebase keeps its monitor (singleton survives us);
  // an idle/finished monitor is reset so the next mount starts clean.
  $effect(() => {
    return () => {
      if (!monitor.active) monitor.stop();
    };
  });

  // -- plan editing --------------------------------------------------------------

  function moveBy(index: number, delta: number): void {
    rows = moveRow(rows, index, index + delta);
  }

  function onDragStart(event: DragEvent, index: number): void {
    dragFrom = index;
    event.dataTransfer?.setData("text/plain", String(index));
    if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
  }

  function onDragOver(event: DragEvent, index: number): void {
    if (dragFrom === null) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
    dragOver = index;
  }

  function onDrop(event: DragEvent, index: number): void {
    event.preventDefault();
    const raw = event.dataTransfer?.getData("text/plain");
    const from =
      dragFrom ?? (raw !== undefined && raw !== "" ? Number(raw) : Number.NaN);
    if (!Number.isNaN(from)) rows = moveRow(rows, from, index);
    dragFrom = null;
    dragOver = null;
  }

  function onDragEnd(): void {
    dragFrom = null;
    dragOver = null;
  }

  // -- start / continue / cancel ---------------------------------------------------

  async function startRebase(): Promise<void> {
    if (!canStart) return;
    const steps: RebaseStep[] = toRebaseSteps(rows);
    const invalid = validatePlan(steps);
    if (invalid !== null) {
      toast(invalid, { kind: "error" });
      return;
    }
    starting = true;
    try {
      await rebaseStart(repoId, steps, baseSha ?? undefined);
      monitor.start(repoId); // (re)engage polling for the new run
      phase = "running";
      toast(
        `Rebase started (${steps.length} step${steps.length === 1 ? "" : "s"})`,
        { kind: "info" },
      );
    } catch (err) {
      toast(
        `Rebase failed to start: ${err instanceof Error ? err.message : String(err)}`,
        { kind: "error" },
      );
    } finally {
      starting = false;
    }
  }

  async function continueRebase(): Promise<void> {
    await monitor.continueRebase();
  }

  /** Abort confirmation (ConfirmDialog state, M9 F9). */
  let abortOpen = $state(false);

  function cancel(): void {
    if (phase === "running") abortOpen = true;
    else onClose();
  }

  async function abortRunning(): Promise<void> {
    phase = "done"; // the settle effect must not double-report
    const aborted = await monitor.abort();
    if (aborted) {
      onFinished();
      onClose();
    } else {
      phase = "running"; // still live — let the user retry
    }
  }
</script>

<Dialog.Root
  open={true}
  onOpenChange={(next) => {
    if (!next) onClose();
  }}
>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <Dialog.Overlay class="scrim bg-black/30" role="presentation" />
  <Dialog.Content
    class="dialog flex max-h-[80vh] w-[min(40rem,calc(100vw-2rem))] max-w-none flex-col gap-0 rounded-3xl bg-card px-6 py-5 text-foreground shadow-lg sm:max-w-none"
    role="dialog"
    aria-modal="true"
    aria-labelledby="rebase-title"
    showCloseButton={false}
    escapeKeydownBehavior={phase === "planning" ? "close" : "ignore"}
    interactOutsideBehavior={phase === "planning" ? "close" : "ignore"}
  >
    <Dialog.Title id="rebase-title" class="mx-1 mb-3 text-xl font-normal text-foreground">
      {#if phase === "planning"}
        Rebase {loading ? "…" : rows.length}
        commit{rows.length === 1 ? "" : "s"}{baseSha === null ? "" : ` onto ${shortBase}`}
      {:else if phase === "running"}
        Rebase in progress
      {:else}
        Rebase finished
      {/if}
    </Dialog.Title>

    {#if phase === "planning"}
      {#if loading}
        <p class="state mx-1 mb-3 mt-1 text-[0.8125rem] text-muted-foreground" role="status">Loading commits…</p>
      {:else if loadError !== null}
        <p class="state error mx-1 mb-3 mt-1 text-[0.8125rem] text-destructive" role="alert">Failed to load commits: {loadError}</p>
      {:else if rows.length === 0}
        <p class="state mx-1 mb-3 mt-1 text-[0.8125rem] text-muted-foreground">No commits in range (HEAD..{shortBase}).</p>
      {:else}
        <ol class="plan m-0 mb-2 min-h-0 flex-1 list-none overflow-y-auto rounded-xl border border-border bg-background p-1" aria-label="Rebase plan (oldest first)">
          {#each rows as row, i (row.sha)}
            <li
              class={"prow cursor-grab rounded-lg bg-background" +
                (i > 0 ? " mt-0.5" : "") +
                (dragFrom === i ? " dragging opacity-45" : "") +
                (dragOver === i && dragFrom !== null && dragFrom !== i
                  ? " droptarget outline-2 outline-dashed outline-primary -outline-offset-2"
                  : "")}
              draggable="true"
              ondragstart={(e) => onDragStart(e, i)}
              ondragover={(e) => onDragOver(e, i)}
              ondrop={(e) => onDrop(e, i)}
              ondragend={onDragEnd}
            >
              <div class="pline flex items-center gap-1.5 px-[0.35rem] py-0.5 text-[0.78rem]">
                <span class="grip flex-none cursor-grab select-none text-muted-foreground" aria-hidden="true">⠿</span>
                <select
                  class="action w-[5.5rem] flex-none rounded-md border border-border bg-muted px-1 py-0.5 text-xs text-foreground focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring"
                  bind:value={row.action}
                  aria-label={`Action for ${row.summary}`}
                >
                  {#each REBASE_ACTIONS as action (action)}
                    <option
                      value={action}
                      disabled={(action === "squash" || action === "fixup") && i === 0}
                    >
                      {action}
                    </option>
                  {/each}
                </select>
                <span class="psha flex-none font-mono text-primary">{row.sha.slice(0, 7)}</span>
                <span class="psummary min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-foreground" title={row.summary}>{row.summary}</span>
                <span class="moves flex flex-none gap-0.5">
                  <Button
                    variant="outline"
                    size="icon-xs"
                    class="text-[0.7rem] leading-none text-muted-foreground disabled:opacity-35"
                    type="button"
                    onclick={() => moveBy(i, -1)}
                    disabled={i === 0}
                    aria-label={`Move ${row.summary} up`}
                  >↑</Button>
                  <Button
                    variant="outline"
                    size="icon-xs"
                    class="text-[0.7rem] leading-none text-muted-foreground disabled:opacity-35"
                    type="button"
                    onclick={() => moveBy(i, 1)}
                    disabled={i === rows.length - 1}
                    aria-label={`Move ${row.summary} down`}
                  >↓</Button>
                </span>
              </div>
              {#if row.action === "reword"}
                <Textarea
                  class="reword mb-1 ml-[2.1rem] mt-0.5 h-auto min-h-0 w-[calc(100%-2.2rem)] resize-y rounded-md bg-background px-2 py-1.5 font-mono text-xs focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring md:text-xs"
                  rows={3}
                  bind:value={row.message}
                  aria-label={`New message for ${row.sha.slice(0, 7)}`}
                  placeholder="New commit message"
                  spellcheck="false"
                ></Textarea>
              {/if}
            </li>
          {/each}
        </ol>
        {#if validationError !== null}
          <p class="validate mx-1 mb-2 text-[0.78rem] text-destructive" role="alert">{validationError}</p>
        {/if}
      {/if}
      <div class="actions flex flex-none justify-end gap-2 pt-1">
        {#if baseSha !== null}
          <Button
            variant="outline"
            class="text-primary"
            type="button"
            onclick={() => void onAutosquash()}
            disabled={autosquashing || loading}
            title="Move fixup!/squash! commits after their targets"
          >
            {autosquashing ? "Autosquashing…" : "Autosquash"}
          </Button>
        {/if}
        <span style:flex="1"></span>
        <Button variant="outline" class="text-primary" type="button" onclick={cancel} disabled={starting}>
          Cancel
        </Button>
        <Button variant="default" type="button" onclick={startRebase} disabled={!canStart}>
          {starting ? "Starting…" : "Start rebase"}
        </Button>
      </div>
    {:else}
      {#if monitor.inConflict}
        <div class="conflict mx-1 mb-2 rounded-xl border border-destructive/55 bg-destructive/5 px-3 py-2.5 text-[0.8125rem]" role="alert">
          <p class="ctitle mb-1 font-medium text-destructive">
            Conflicts — {monitor.conflictCount}
            file{monitor.conflictCount === 1 ? "" : "s"}
          </p>
          <ul class="clist m-0 mb-1 max-h-28 list-none overflow-y-auto p-0">
            {#each monitor.conflicts as file (file.path)}
              <li class="cpath overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs text-foreground" title={file.path}>{file.path}</li>
            {/each}
          </ul>
          <p class="chint mb-2 text-[0.78rem] text-muted-foreground">Resolve in the status panel, then Continue.</p>
          <Button
            variant="outline"
            class="text-primary"
            type="button"
            onclick={() =>
              toast("Resolve conflicts in the status panel, then press Continue", {
                kind: "info",
              })}
          >
            Open in editor
          </Button>
        </div>
      {:else if monitor.paused && phase === "running"}
        <p class="state mx-1 mb-3 mt-1 text-[0.8125rem] text-muted-foreground" role="status">
          Paused for edit — inspect the commit, then Continue.
        </p>
      {/if}

      {#if phase === "running"}
        <p class="runstate mx-1 my-2 text-[0.8125rem] tabular-nums text-foreground" role="status">
          {#if progress !== null}
            step {progress.k}/{progress.n} ({progress.label}) · {progress.rewritten}
            rewritten
          {:else}
            waiting for rebase state…
          {/if}
        </p>
      {:else}
        <p class="state mx-1 mb-3 mt-1 text-[0.8125rem] text-muted-foreground" role="status">
          {progress?.rewritten ?? monitor.rewrittenCount}
          commit{(progress?.rewritten ?? monitor.rewrittenCount) === 1 ? "" : "s"}
          rewritten.
        </p>
      {/if}

      <div class="actions flex flex-none justify-end gap-2 pt-1">
        {#if phase === "running"}
          <Button variant="outline" class="border-destructive/60 text-destructive" type="button" onclick={cancel}>Abort rebase</Button>
          <Button
            variant="default"
            type="button"
            onclick={continueRebase}
            disabled={!monitor.paused}
          >
            Continue
          </Button>
        {:else}
          <Button variant="default" type="button" onclick={cancel}>Close</Button>
        {/if}
      </div>
    {/if}
  </Dialog.Content>
</Dialog.Root>

<ConfirmDialog
  bind:open={abortOpen}
  title="Abort the running rebase?"
  message="The original branch state is restored; completed steps of this run are discarded."
  confirmLabel="Abort rebase"
  danger
  onConfirm={() => void abortRunning()}
/>
