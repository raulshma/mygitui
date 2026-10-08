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
   * A11y: `role="dialog"` + `aria-modal`, basic focus trap (Tab cycles inside
   * the dialog), Esc closes in the planning phase only (never mid-rebase).
   * Closing the dialog never stops a live rebase — the shared monitor keeps
   * watching and a reopened planner re-attaches to it.
   */
  import { rebaseStart, streamLog } from "$lib/ipc/client";
  import type { CommitInfo, RebaseStep } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
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
  let dialogEl: HTMLDivElement | undefined = $state();

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

  // Initial focus (the dialog container is tabindex="-1").
  $effect(() => {
    dialogEl?.focus();
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

  // -- dialog a11y (Esc = planning only; basic focus trap) --------------------------

  function onDialogKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      if (phase === "planning") {
        event.preventDefault();
        onClose();
      }
      return;
    }
    if (event.key !== "Tab") return;
    const root = dialogEl;
    if (root === undefined) return;
    const focusables = Array.from(
      root.querySelectorAll<HTMLElement>(
        'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ),
    );
    if (focusables.length === 0) return;
    const first = focusables[0] as HTMLElement;
    const last = focusables[focusables.length - 1] as HTMLElement;
    const current = document.activeElement;
    if (event.shiftKey && (current === first || !root.contains(current))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && current === last) {
      event.preventDefault();
      first.focus();
    }
  }
</script>

<div
  class="scrim"
  role="presentation"
  onclick={(e) => {
    if (e.target === e.currentTarget && phase === "planning") onClose();
  }}
>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="rebase-title"
    bind:this={dialogEl}
    tabindex="-1"
    onkeydown={onDialogKeydown}
  >
    <h2 id="rebase-title" class="title">
      {#if phase === "planning"}
        Rebase {loading ? "…" : rows.length}
        commit{rows.length === 1 ? "" : "s"}{baseSha === null ? "" : ` onto ${shortBase}`}
      {:else if phase === "running"}
        Rebase in progress
      {:else}
        Rebase finished
      {/if}
    </h2>

    {#if phase === "planning"}
      {#if loading}
        <p class="state" role="status">Loading commits…</p>
      {:else if loadError !== null}
        <p class="state error" role="alert">Failed to load commits: {loadError}</p>
      {:else if rows.length === 0}
        <p class="state">No commits in range (HEAD..{shortBase}).</p>
      {:else}
        <ol class="plan" aria-label="Rebase plan (oldest first)">
          {#each rows as row, i (row.sha)}
            <li
              class="prow"
              class:dragging={dragFrom === i}
              class:droptarget={dragOver === i && dragFrom !== null && dragFrom !== i}
              draggable="true"
              ondragstart={(e) => onDragStart(e, i)}
              ondragover={(e) => onDragOver(e, i)}
              ondrop={(e) => onDrop(e, i)}
              ondragend={onDragEnd}
            >
              <div class="pline">
                <span class="grip" aria-hidden="true">⠿</span>
                <select
                  class="action"
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
                <span class="psha">{row.sha.slice(0, 7)}</span>
                <span class="psummary" title={row.summary}>{row.summary}</span>
                <span class="moves">
                  <button
                    class="mv"
                    type="button"
                    onclick={() => moveBy(i, -1)}
                    disabled={i === 0}
                    aria-label={`Move ${row.summary} up`}
                  >↑</button>
                  <button
                    class="mv"
                    type="button"
                    onclick={() => moveBy(i, 1)}
                    disabled={i === rows.length - 1}
                    aria-label={`Move ${row.summary} down`}
                  >↓</button>
                </span>
              </div>
              {#if row.action === "reword"}
                <textarea
                  class="reword"
                  rows="3"
                  bind:value={row.message}
                  aria-label={`New message for ${row.sha.slice(0, 7)}`}
                  placeholder="New commit message"
                  spellcheck="false"
                ></textarea>
              {/if}
            </li>
          {/each}
        </ol>
        {#if validationError !== null}
          <p class="validate" role="alert">{validationError}</p>
        {/if}
      {/if}
      <div class="actions">
        <button class="ghost" type="button" onclick={cancel} disabled={starting}>
          Cancel
        </button>
        <button class="primary" type="button" onclick={startRebase} disabled={!canStart}>
          {starting ? "Starting…" : "Start rebase"}
        </button>
      </div>
    {:else}
      {#if monitor.inConflict}
        <div class="conflict" role="alert">
          <p class="ctitle">
            Conflicts — {monitor.conflictCount}
            file{monitor.conflictCount === 1 ? "" : "s"}
          </p>
          <ul class="clist">
            {#each monitor.conflicts as file (file.path)}
              <li class="cpath" title={file.path}>{file.path}</li>
            {/each}
          </ul>
          <p class="chint">Resolve in the status panel, then Continue.</p>
          <button
            class="ghost"
            type="button"
            onclick={() =>
              toast("Resolve conflicts in the status panel, then press Continue", {
                kind: "info",
              })}
          >
            Open in editor
          </button>
        </div>
      {:else if monitor.paused && phase === "running"}
        <p class="state" role="status">
          Paused for edit — inspect the commit, then Continue.
        </p>
      {/if}

      {#if phase === "running"}
        <p class="runstate" role="status">
          {#if progress !== null}
            step {progress.k}/{progress.n} ({progress.label}) · {progress.rewritten}
            rewritten
          {:else}
            waiting for rebase state…
          {/if}
        </p>
      {:else}
        <p class="state" role="status">
          {progress?.rewritten ?? monitor.rewrittenCount}
          commit{(progress?.rewritten ?? monitor.rewrittenCount) === 1 ? "" : "s"}
          rewritten.
        </p>
      {/if}

      <div class="actions">
        {#if phase === "running"}
          <button class="ghost danger" type="button" onclick={cancel}>Abort rebase</button>
          <button
            class="primary"
            type="button"
            onclick={continueRebase}
            disabled={!monitor.paused}
          >
            Continue
          </button>
        {:else}
          <button class="primary" type="button" onclick={cancel}>Close</button>
        {/if}
      </div>
    {/if}
  </div>
</div>

<ConfirmDialog
  bind:open={abortOpen}
  title="Abort the running rebase?"
  message="The original branch state is restored; completed steps of this run are discarded."
  confirmLabel="Abort rebase"
  danger
  onConfirm={() => void abortRunning()}
/>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: color-mix(in srgb, var(--m3-scrim, #000) 32%, transparent);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 8vh;
  }

  .dialog {
    width: min(40rem, calc(100vw - 2rem));
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    background: var(--m3-surface-container-high, var(--m3-surface));
    border-radius: var(--m3-shape-extra-large, 28px);
    box-shadow: var(--m3-elevation-3, 0 4px 8px rgba(0, 0, 0, 0.3));
    padding: 1.25rem 1.5rem;
    outline: none;
  }

  .dialog:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .title {
    margin: 0 0.25rem 0.75rem;
    font-size: 1.25rem;
    font-weight: 400;
    color: var(--m3-on-surface);
  }

  .state {
    margin: 0.25rem 0.25rem 0.75rem;
    color: var(--m3-on-surface-variant);
    font-size: 0.8125rem;
  }

  .state.error {
    color: var(--m3-error);
  }

  .runstate {
    margin: 0.5rem 0.25rem;
    color: var(--m3-on-surface);
    font-size: 0.8125rem;
    font-variant-numeric: tabular-nums;
  }

  /* -- plan rows ------------------------------------------------------------------ */

  .plan {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin: 0 0 0.5rem;
    padding: 0.25rem;
    list-style: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-medium, 12px);
    background: var(--m3-surface);
  }

  .prow {
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface);
  }

  .prow + .prow {
    margin-top: 0.2rem;
  }

  .pline {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.2rem 0.35rem;
    font-size: 0.78rem;
  }

  .prow[draggable="true"] {
    cursor: grab;
  }

  .prow.dragging {
    opacity: 0.45;
  }

  .prow.droptarget {
    outline: 2px dashed var(--m3-primary);
    outline-offset: -2px;
  }

  .grip {
    flex: none;
    color: var(--m3-on-surface-variant);
    cursor: grab;
    user-select: none;
  }

  select.action {
    flex: none;
    width: 5.5rem;
    padding: 0.15rem 0.25rem;
    font: inherit;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
  }

  select.action:focus-visible,
  .reword:focus-visible,
  .mv:focus-visible,
  button:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .psha {
    flex: none;
    font-family: ui-monospace, Consolas, monospace;
    color: var(--m3-primary);
  }

  .psummary {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface);
  }

  .moves {
    flex: none;
    display: flex;
    gap: 0.15rem;
  }

  .mv {
    width: 1.4rem;
    height: 1.4rem;
    padding: 0;
    font-size: 0.7rem;
    line-height: 1;
    color: var(--m3-on-surface-variant);
    background: none;
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    cursor: pointer;
  }

  .mv:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }

  .reword {
    width: calc(100% - 2.2rem);
    margin: 0.1rem 0 0.3rem 2.1rem;
    padding: 0.35rem 0.5rem;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-low, var(--m3-surface));
    border: 1px solid var(--m3-outline-variant);
    border-radius: var(--m3-shape-extra-small, 4px);
    resize: vertical;
  }

  .validate {
    margin: 0 0.25rem 0.5rem;
    color: var(--m3-error);
    font-size: 0.78rem;
  }

  /* -- conflict panel --------------------------------------------------------------- */

  .conflict {
    margin: 0 0.25rem 0.5rem;
    padding: 0.6rem 0.75rem;
    border: 1px solid color-mix(in srgb, var(--m3-error) 55%, transparent);
    border-radius: var(--m3-shape-medium, 12px);
    background: color-mix(in srgb, var(--m3-error) 8%, var(--m3-surface));
    font-size: 0.8125rem;
  }

  .ctitle {
    margin: 0 0 0.3rem;
    font-weight: 500;
    color: var(--m3-error);
  }

  .clist {
    max-height: 7rem;
    margin: 0 0 0.3rem;
    padding: 0;
    list-style: none;
    overflow-y: auto;
  }

  .cpath {
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    color: var(--m3-on-surface);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chint {
    margin: 0 0 0.5rem;
    color: var(--m3-on-surface-variant);
    font-size: 0.78rem;
  }

  /* -- action buttons ------------------------------------------------------------------ */

  .actions {
    flex: none;
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    padding-top: 0.25rem;
  }

  .primary,
  .ghost {
    font: inherit;
    font-size: 0.875rem;
    padding: 0.4rem 1.1rem;
    border-radius: var(--m3-shape-full, 9999px);
    cursor: pointer;
  }

  .primary {
    border: none;
    background: var(--m3-primary);
    color: var(--m3-on-primary);
  }

  .primary:disabled {
    background: color-mix(in srgb, var(--m3-on-surface) 12%, transparent);
    color: color-mix(in srgb, var(--m3-on-surface) 38%, transparent);
    cursor: not-allowed;
  }

  .ghost {
    border: 1px solid var(--m3-outline, var(--m3-primary));
    background: none;
    color: var(--m3-primary);
  }

  .ghost.danger {
    color: var(--m3-error);
    border-color: color-mix(in srgb, var(--m3-error) 60%, transparent);
  }

  .ghost:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
