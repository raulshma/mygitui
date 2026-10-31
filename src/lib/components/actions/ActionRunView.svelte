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
  import { Button } from "$lib/components/ui/button/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Toggle } from "$lib/components/ui/toggle/index.js";
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
  import { actionLogSegments } from "./actionLog";
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

  // Re-enable follow via the Follow toggle (bits-ui flips `follow` through
  // bind:pressed): jump back to the tail.
  $effect(() => {
    if (follow) scrollToBottom();
  });

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

<div class="action-run-view @container flex h-full min-h-0 min-w-0 flex-col bg-background">
  <header class="run-head flex-none flex flex-col gap-[0.35rem] border-b border-border bg-card px-[0.7rem] pb-[0.55rem] pt-[0.4rem] @max-[38rem]:px-2 @max-[38rem]:pb-[0.45rem] @max-[38rem]:pt-[0.35rem]">
    <div class="head-top flex min-w-0 items-center gap-[0.4rem]">
      <span class="crumbs flex min-w-0 items-baseline gap-[0.35rem] text-[0.72rem] text-muted-foreground">
        <span class="crumb-muted flex-none opacity-80">Actions</span>
        <span class="crumb-sep flex-none opacity-80" aria-hidden="true">/</span>
        <span class="crumb-current min-w-0 truncate font-medium text-foreground" title={run?.name ?? "Action run"}>
          {run?.name ?? "Action run"}
        </span>
        {#if run}<span class="run-id flex-none font-mono text-[0.66rem] text-muted-foreground opacity-85">{run.run_id}</span>{/if}
      </span>
      <span class="spacer ml-auto"></span>
      {#if onPopout}
        <Button
          variant="outline"
          size="xs"
          class="tb flex-none rounded-full px-[0.6rem] text-[0.68rem] leading-[1.4] text-muted-foreground @max-[38rem]:px-[0.45rem] @max-[38rem]:text-[0.64rem]"
          type="button"
          title="Open this run log in its own window"
          onclick={onPopout}
        >
          Pop out
        </Button>
      {/if}
      {#if onClose}
        <Button
          variant="outline"
          size="xs"
          class="tb close flex-none rounded-full px-[0.4rem] text-[0.9rem] leading-none text-muted-foreground"
          type="button"
          aria-label="Close run log"
          title="Close run log"
          onclick={onClose}
        >
          ×
        </Button>
      {/if}
    </div>

    {#if run}
      <div class="title-row flex min-w-0 flex-wrap items-center gap-[0.45rem]">
        <Badge
          variant="outline"
          class={`status-chip chip-${run.status} h-auto flex-none items-center whitespace-nowrap rounded-full border-transparent px-2 py-[0.05rem] text-[0.66rem] font-semibold ${run.status === "running" ? "bg-secondary text-secondary-foreground" : run.status === "success" ? "bg-primary/15 text-primary" : run.status === "failed" ? "bg-destructive/10 text-destructive" : "bg-muted text-muted-foreground"}`}
        >
          {runStatusLabel(run.status, run.exit_code)}
        </Badge>
        {#if live}
          <span class="live-badge flex-none inline-flex items-center gap-[0.3rem] text-[0.66rem] font-semibold text-primary">
            <span class="live-dot size-[0.42rem] rounded-full bg-current animate-pulse" aria-hidden="true"></span>
            Running
          </span>
        {/if}
        <h2 class="run-title m-0 min-w-0 text-[0.95rem] font-semibold leading-[1.25] text-foreground [overflow-wrap:anywhere]">{run.name}</h2>
      </div>
      {#if run.command}
        <code class="command max-w-full self-start truncate rounded border border-border bg-muted px-[0.45rem] py-[0.12rem] font-mono text-[0.7rem] text-foreground" title={run.command}>{run.command}</code>
      {/if}
      <div class="meta flex flex-wrap items-center gap-x-3 gap-y-[0.35rem] text-[0.68rem] text-muted-foreground">
        <span class="meta-item inline-flex items-baseline gap-1 whitespace-nowrap">
          <span class="meta-label opacity-75">Output</span>
          {run.lines.length}
          {run.lines.length === 1 ? "line" : "lines"}
          {#if run.dropped > 0}({run.dropped} earlier dropped){/if}
        </span>
        {#if run.done && run.exit_code !== null}
          <span class="meta-item inline-flex items-baseline gap-1 whitespace-nowrap">
            <span class="meta-label opacity-75">Exit code</span>
            {run.exit_code}
          </span>
        {/if}
      </div>
    {:else}
      <div class="title-row flex min-w-0 flex-wrap items-center gap-[0.45rem]">
        <h2 class="run-title m-0 min-w-0 text-[0.95rem] font-semibold leading-[1.25] text-foreground [overflow-wrap:anywhere]">Run {runId}</h2>
      </div>
    {/if}
  </header>

  {#if run}
    <section class="log-pane m-2 flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-border bg-card" aria-label="Action output">
      <div class="log-head flex-none flex flex-wrap items-center gap-[0.4rem] border-b border-border bg-muted/50 px-2 py-[0.35rem]">
        <h3 class="pane-title m-0 text-xs font-semibold text-foreground">Output</h3>
        <Badge
          variant="outline"
          class={`log-state ${live ? "live" : ""} h-auto flex-none rounded-full border-transparent px-[0.45rem] py-px text-[0.62rem] ${live ? "bg-secondary text-secondary-foreground" : "bg-muted text-muted-foreground"}`}
        >
          {live ? "Live" : "Finished"}
        </Badge>
        <span class="spacer ml-auto"></span>
        <Toggle
          variant="outline"
          size="sm"
          class="tb flex-none whitespace-nowrap rounded-full border-border px-[0.6rem] text-[0.68rem] leading-[1.4] text-muted-foreground aria-pressed:border-transparent aria-pressed:bg-primary/15 aria-pressed:text-primary @max-[38rem]:px-[0.45rem] @max-[38rem]:text-[0.64rem]"
          bind:pressed={follow}
          title={follow
            ? "Pause auto-scroll (scroll up to pause too)"
            : "Follow new output"}
        >
          {follow ? "Following" : "Follow"}
        </Toggle>
        <Button variant="outline" size="xs" class="tb flex-none rounded-full px-[0.6rem] text-[0.68rem] leading-[1.4] text-muted-foreground @max-[38rem]:px-[0.45rem] @max-[38rem]:text-[0.64rem]" type="button" onclick={() => void copyLog()}>
          Copy log
        </Button>
        {#if live}
          <Button
            variant="destructive"
            size="xs"
            class="tb danger flex-none rounded-full px-[0.6rem] text-[0.68rem] leading-[1.4] @max-[38rem]:px-[0.45rem] @max-[38rem]:text-[0.64rem]"
            type="button"
            title="Cancel this action"
            onclick={cancelRun}
          >
            Cancel
          </Button>
        {/if}
      </div>

      <div
        class="log-body min-h-0 min-w-0 flex-1 overflow-auto bg-background px-[0.65rem] py-2 font-mono text-[0.74rem] leading-[1.5] text-foreground select-text"
        bind:this={logEl}
        onscroll={onScroll}
        role="log"
        aria-live="polite"
        aria-label={`Output of ${run.name}`}
      >
        {#if shown.length === 0}
          <p class="log-empty m-0 font-[inherit] text-muted-foreground">
            {live ? "Waiting for output…" : "This run produced no output."}
          </p>
        {:else}
          {#each shown as line, i (i)}
            {@const segments = actionLogSegments(line)}
            <div class="log-line w-max min-h-[1.1em] min-w-full whitespace-pre {i === 0 && run.dropped > 0 ? 'notice italic text-muted-foreground' : ''}" class:notice={i === 0 && run.dropped > 0}>
              {#each segments as segment, segmentIndex (segmentIndex)}
                <span
                  style:color={segment.color}
                  style:background-color={segment.background}
                  style:font-weight={segment.bold ? "700" : undefined}
                  style:opacity={segment.dim ? "0.7" : undefined}
                  style:font-style={segment.italic ? "italic" : undefined}
                  style:text-decoration-line={segment.underline && segment.strike
                    ? "underline line-through"
                    : segment.underline
                      ? "underline"
                      : segment.strike
                        ? "line-through"
                        : undefined}
                >{segment.text}</span>
              {/each}
            </div>
          {/each}
          {#if live}
            <div class="log-line log-live w-max min-h-[1.1em] min-w-full whitespace-pre animate-[blink_1s_step-end_infinite] text-primary" aria-hidden="true">▊</div>
          {/if}
        {/if}
      </div>
    </section>
  {:else}
    <div class="run-state flex min-h-0 flex-1 flex-col items-center justify-center gap-2 p-8 text-center" role="status">
      <p class="state-title m-0 text-[0.9rem] font-semibold text-foreground">This run is no longer available.</p>
      <p class="state-hint m-0 max-w-96 text-xs leading-[1.45] text-muted-foreground">
        Its output was not kept, or the run ended before this window loaded.
        Run the action again to see fresh output.
      </p>
      {#if onClose}
        <Button variant="outline" size="xs" class="tb flex-none rounded-full px-[0.6rem] text-[0.68rem] leading-[1.4] text-muted-foreground" type="button" onclick={onClose}>Close</Button>
      {/if}
    </div>
  {/if}
</div>

