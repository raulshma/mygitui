<script lang="ts">
  /**
   * Custom action run detail — rendered in the main editor (or a popout).
   *
   * Shows the action name/command, a live status chip and the streamed
   * output (auto-following until the user scrolls up, copyable, cancellable
   * while running). The same component serves the popout window: there the
   * run is hydrated from the snapshot the main window stashed before
   * opening ({@link readStashedActionRun}) and live lines keep arriving over
   * the shared `action-output` subscription.
   */
  import { untrack } from "svelte";
  import { toast } from "$lib/toast";
  import {
    actionsStore,
    startActionsEvents,
  } from "$lib/stores/actions.svelte";
  import {
    outputDisplayLines,
    runStatusLabel,
    truncateLines,
    RUN_LINE_CAP,
  } from "./actionsModel";
  import { readStashedActionRun } from "./runPopout";

  let {
    repoId,
    runId,
    onClose = undefined,
    onPopout = undefined,
  }: {
    repoId: string;
    runId: string;
    /** Closes the run view (main editor only). */
    onClose?: () => void;
    /** Opens the run log in its own window. */
    onPopout?: () => void;
  } = $props();

  // Popout windows start with an empty store: seed the snapshot before the
  // first paint. A no-op in the main window (it observed the run live).
  const initial = untrack(() => ({ repoId, runId }));
  const snapshot = readStashedActionRun(initial.runId);
  if (snapshot !== null && snapshot.repo_id === initial.repoId) {
    actionsStore.hydrateRun(snapshot);
  }

  // Idempotent: the shared event module owns the actual subscription.
  $effect(() => {
    void repoId;
    startActionsEvents();
  });

  const run = $derived(actionsStore.runFor(runId));

  /** Display lines (truncation notice included when the cap dropped lines). */
  const shown = $derived.by(() => {
    if (!run) return [];
    const kept = truncateLines(run.lines, RUN_LINE_CAP);
    return outputDisplayLines(
      kept.lines,
      run.dropped + kept.dropped,
      RUN_LINE_CAP,
    );
  });

  const live = $derived(run !== null && !run.done);

  /** Auto-scroll to the tail while the user stays at the bottom. */
  let follow = $state(true);
  let logEl: HTMLDivElement | undefined = $state();

  function scrollToBottom(): void {
    requestAnimationFrame(() => {
      if (logEl) logEl.scrollTop = logEl.scrollHeight;
    });
  }

  $effect(() => {
    const current = run;
    if (!current) return;
    void current.lines.length;
    void current.done;
    if (follow) scrollToBottom();
  });

  function onScroll(): void {
    if (!logEl) return;
    const distance =
      logEl.scrollHeight - logEl.scrollTop - logEl.clientHeight;
    follow = distance < 24;
  }

  function toggleFollow(): void {
    follow = !follow;
    if (follow) scrollToBottom();
  }

  async function copyLog(): Promise<void> {
    if (!run) return;
    try {
      await navigator.clipboard.writeText(shown.join("\n"));
      toast("Run log copied to the clipboard", { kind: "success" });
    } catch {
      toast("Clipboard is not available", { kind: "error" });
    }
  }

  function cancelRun(): void {
    if (!run) return;
    void actionsStore.cancel(run.run_id);
  }
</script>

<div class="action-run-view">
  <header class="run-head">
    <div class="head-top">
      <span class="crumbs">
        <span class="crumb-muted">Actions</span>
        <span class="crumb-sep" aria-hidden="true">/</span>
        <span class="crumb-current" title={run?.name ?? "Action run"}>
          {run?.name ?? "Action run"}
        </span>
        {#if run}<span class="run-id">{run.run_id}</span>{/if}
      </span>
      <span class="spacer"></span>
      {#if onPopout}
        <button
          class="tb"
          type="button"
          title="Open this run log in its own window"
          onclick={onPopout}
        >
          Pop out
        </button>
      {/if}
      {#if onClose}
        <button
          class="tb close"
          type="button"
          aria-label="Close run log"
          title="Close run log"
          onclick={onClose}
        >
          ×
        </button>
      {/if}
    </div>

    {#if run}
      <div class="title-row">
        <span class="status-chip chip-{run.status}">
          {runStatusLabel(run.status, run.exit_code)}
        </span>
        {#if live}
          <span class="live-badge">
            <span class="live-dot" aria-hidden="true"></span>
            Running
          </span>
        {/if}
        <h2 class="run-title">{run.name}</h2>
      </div>
      {#if run.command}
        <code class="command" title={run.command}>{run.command}</code>
      {/if}
      <div class="meta">
        <span class="meta-item">
          <span class="meta-label">Output</span>
          {run.lines.length}
          {run.lines.length === 1 ? "line" : "lines"}
          {#if run.dropped > 0}({run.dropped} earlier dropped){/if}
        </span>
        {#if run.done && run.exit_code !== null}
          <span class="meta-item">
            <span class="meta-label">Exit code</span>
            {run.exit_code}
          </span>
        {/if}
      </div>
    {:else}
      <div class="title-row">
        <h2 class="run-title">Run {runId}</h2>
      </div>
    {/if}
  </header>

  {#if run}
    <section class="log-pane" aria-label="Action output">
      <div class="log-head">
        <h3 class="pane-title">Output</h3>
        <span class="log-state" class:live>
          {live ? "Live" : "Finished"}
        </span>
        <span class="spacer"></span>
        <button
          class="tb"
          class:active={follow}
          type="button"
          aria-pressed={follow}
          title={follow
            ? "Pause auto-scroll (scroll up to pause too)"
            : "Follow new output"}
          onclick={toggleFollow}
        >
          {follow ? "Following" : "Follow"}
        </button>
        <button class="tb" type="button" onclick={() => void copyLog()}>
          Copy log
        </button>
        {#if live}
          <button
            class="tb danger"
            type="button"
            title="Cancel this action"
            onclick={cancelRun}
          >
            Cancel
          </button>
        {/if}
      </div>

      <div
        class="log-body"
        bind:this={logEl}
        onscroll={onScroll}
        role="log"
        aria-live="polite"
        aria-label={`Output of ${run.name}`}
      >
        {#if shown.length === 0}
          <p class="log-empty">
            {live ? "Waiting for output…" : "This run produced no output."}
          </p>
        {:else}
          {#each shown as line, i (i)}
            <div class="log-line" class:notice={i === 0 && run.dropped > 0}>
              {line}
            </div>
          {/each}
          {#if live}
            <div class="log-line log-live" aria-hidden="true">▊</div>
          {/if}
        {/if}
      </div>
    </section>
  {:else}
    <div class="run-state" role="status">
      <p class="state-title">This run is no longer available.</p>
      <p class="state-hint">
        Its output was not kept, or the run ended before this window loaded.
        Run the action again to see fresh output.
      </p>
      {#if onClose}
        <button class="tb" type="button" onclick={onClose}>Close</button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .action-run-view {
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

  .crumb-muted,
  .crumb-sep {
    flex: none;
    opacity: 0.8;
  }

  .crumb-current {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--m3-on-surface);
    font-weight: 500;
  }

  .run-id {
    flex: none;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.66rem;
    color: var(--m3-on-surface-variant);
    opacity: 0.85;
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
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0.05rem 0.5rem;
    font-size: 0.66rem;
    font-weight: 600;
    white-space: nowrap;
    background: var(--m3-surface-container-highest, var(--m3-surface-container));
    color: var(--m3-on-surface-variant);
  }

  .status-chip.chip-running {
    background: var(--m3-tertiary-container);
    color: var(--m3-on-tertiary-container);
  }

  .status-chip.chip-success {
    background: var(--m3-primary-container);
    color: var(--m3-on-primary-container);
  }

  .status-chip.chip-failed {
    background: var(--m3-error-container);
    color: var(--m3-on-error-container);
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

  .live-dot {
    width: 0.42rem;
    height: 0.42rem;
    border-radius: 50%;
    background: currentColor;
    animation: pulse 1.4s ease-in-out infinite;
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

  .command {
    align-self: flex-start;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 0.12rem 0.45rem;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-extra-small, 4px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    color: var(--m3-on-surface);
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.7rem;
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

  .tb:hover {
    background: var(--app-hover, var(--m3-surface-container-high));
    color: var(--m3-on-surface);
  }

  .tb:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
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

  .log-pane {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    margin: 0.5rem;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-small, 8px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    overflow: hidden;
  }

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
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--m3-on-surface);
  }

  .log-state {
    flex: none;
    border-radius: var(--m3-shape-full, 9999px);
    padding: 0 0.45rem;
    font-size: 0.62rem;
    background: var(--m3-surface-container-highest);
    color: var(--m3-on-surface-variant);
  }

  .log-state.live {
    background: var(--m3-tertiary-container);
    color: var(--m3-on-tertiary-container);
  }

  .log-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0.5rem 0.65rem;
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.74rem;
    line-height: 1.5;
    color: var(--m3-on-surface);
    user-select: text;
  }

  .log-line {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    min-height: 1.1em;
  }

  .log-line.notice {
    color: var(--m3-on-surface-variant);
    font-style: italic;
  }

  .log-live {
    color: var(--m3-primary);
    animation: blink 1s step-end infinite;
  }

  @keyframes blink {
    50% {
      opacity: 0;
    }
  }

  .log-empty {
    margin: 0;
    color: var(--m3-on-surface-variant);
    font-family: inherit;
  }

  .run-state {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    padding: 2rem;
    text-align: center;
  }

  .state-title {
    margin: 0;
    font-size: 0.9rem;
    font-weight: 600;
    color: var(--m3-on-surface);
  }

  .state-hint {
    margin: 0;
    max-width: 24rem;
    font-size: 0.75rem;
    line-height: 1.45;
    color: var(--m3-on-surface-variant);
  }

  @container (max-width: 38rem) {
    .run-head {
      padding: 0.35rem 0.5rem 0.45rem;
    }

    .tb {
      padding: 0.08rem 0.45rem;
      font-size: 0.64rem;
    }
  }
</style>
