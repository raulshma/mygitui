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
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";

  const iconClose = $derived(appIcon("close")!);
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
  <header class="run-head flex-none flex flex-col gap-ui-35 border-b border-border bg-card px-[0.7rem] pb-[0.55rem] pt-ui-4 @max-[38rem]:px-2 @max-[38rem]:pb-ui-45 @max-[38rem]:pt-ui-35">
    <div class="head-top flex min-w-0 items-center gap-ui-4">
      <span class="crumbs flex min-w-0 items-baseline gap-ui-35 text-ui-meta text-muted-foreground">
        <span class="crumb-muted flex-none opacity-80">Actions</span>
        <span class="crumb-sep flex-none opacity-80" aria-hidden="true">/</span>
        <Hint label={run?.name ?? "Action run"}>
          {#snippet child({ props })}
            <span class="crumb-current min-w-0 truncate font-medium text-foreground" {...props}>
              {run?.name ?? "Action run"}
            </span>
          {/snippet}
        </Hint>
        {#if run}<span class="run-id flex-none font-mono text-ui-micro text-muted-foreground opacity-85">{run.run_id}</span>{/if}
      </span>
      <span class="spacer ml-auto"></span>
      {#if onPopout}
        <Hint label="Open this run log in its own window">
          {#snippet child({ props })}
            <Button
              variant="outline"
              size="xs"
              class="tb flex-none rounded-full px-2.5 text-ui-micro leading-[1.4] text-muted-foreground @max-[38rem]:px-2 @max-[38rem]:text-ui-tiny"
              {...props}
              type="button"
              onclick={onPopout}
            >
              Pop out
            </Button>
          {/snippet}
        </Hint>
      {/if}
      {#if onClose}
        <Hint label="Close run log">
          {#snippet child({ props })}
            <Button
              variant="outline"
              size="xs"
              class="tb close flex-none rounded-full px-1.5 text-muted-foreground [&_svg:not([class*='size-'])]:size-3"
              {...props}
              type="button"
              aria-label="Close run log"
              onclick={onClose}
            >
              <Icon icon={iconClose} size={12} />
            </Button>
          {/snippet}
        </Hint>
      {/if}
    </div>

    {#if run}
      <div class="title-row flex min-w-0 flex-wrap items-center gap-ui-45">
        <Badge
          variant="outline"
          class={`status-chip chip-${run.status} h-auto flex-none items-center whitespace-nowrap rounded-full border-transparent px-2 py-px text-ui-micro font-semibold ${run.status === "running" ? "bg-secondary text-secondary-foreground" : run.status === "success" ? "bg-primary/15 text-primary" : run.status === "cancelled" ? "bg-muted text-muted-foreground" : "bg-destructive/10 text-destructive"}`}
        >
          {runStatusLabel(run.status, run.exit_code)}
        </Badge>
        {#if live}
          <span class="live-badge flex-none inline-flex items-center gap-ui-3 text-ui-micro font-semibold text-primary">
            <span class="live-dot size-[0.42rem] rounded-full bg-current animate-pulse" aria-hidden="true"></span>
            Running
          </span>
        {/if}
        <h2 class="run-title m-0 min-w-0 text-[0.95rem] font-semibold leading-[1.25] text-foreground [overflow-wrap:anywhere]">{run.name}</h2>
      </div>
      {#if run.command}
        <Hint label={run.command}>
          {#snippet child({ props })}
            <code class="command max-w-full self-start truncate rounded border border-border bg-muted px-2 py-px font-mono text-ui-micro text-foreground" {...props}>{run.command}</code>
          {/snippet}
        </Hint>
      {/if}
      <div class="meta flex flex-wrap items-center gap-x-3 gap-y-[0.35rem] text-ui-micro text-muted-foreground">
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
      <div class="title-row flex min-w-0 flex-wrap items-center gap-ui-45">
        <h2 class="run-title m-0 min-w-0 text-[0.95rem] font-semibold leading-[1.25] text-foreground [overflow-wrap:anywhere]">Run {runId}</h2>
      </div>
    {/if}
  </header>

  {#if run}
    <section class="log-pane m-2 flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-border bg-card" aria-label="Action output">
      <div class="log-head flex-none flex flex-wrap items-center gap-ui-4 border-b border-border bg-muted/50 px-2 py-ui-35">
        <h3 class="pane-title m-0 text-xs font-semibold text-foreground">Output</h3>
        <Badge
          variant="outline"
          class={`log-state ${live ? "live" : ""} h-auto flex-none rounded-full border-transparent px-2 py-px text-ui-tiny ${live ? "bg-secondary text-secondary-foreground" : "bg-muted text-muted-foreground"}`}
        >
          {live ? "Live" : "Finished"}
        </Badge>
        <span class="spacer ml-auto"></span>
        <Hint label={follow
          ? "Pause auto-scroll (scroll up to pause too)"
          : "Follow new output"}>
          {#snippet child({ props })}
            <Toggle
              variant="outline"
              size="sm"
              class="tb flex-none whitespace-nowrap rounded-full border-border px-2.5 text-ui-micro leading-[1.4] text-muted-foreground aria-pressed:border-transparent aria-pressed:bg-primary/15 aria-pressed:text-primary @max-[38rem]:px-2 @max-[38rem]:text-ui-tiny"
              {...props}
              bind:pressed={follow}
            >
              {follow ? "Following" : "Follow"}
            </Toggle>
          {/snippet}
        </Hint>
        <Button variant="outline" size="xs" class="tb flex-none rounded-full px-2.5 text-ui-micro leading-[1.4] text-muted-foreground @max-[38rem]:px-2 @max-[38rem]:text-ui-tiny" type="button" onclick={() => void copyLog()}>
          Copy log
        </Button>
        {#if live}
          <Hint label="Cancel this action">
            {#snippet child({ props })}
              <Button
                variant="destructive"
                size="xs"
                class="tb danger flex-none rounded-full px-2.5 text-ui-micro leading-[1.4] @max-[38rem]:px-2 @max-[38rem]:text-ui-tiny"
                {...props}
                type="button"
                onclick={cancelRun}
              >
                Cancel
              </Button>
            {/snippet}
          </Hint>
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
        <Button variant="outline" size="xs" class="tb flex-none rounded-full px-2.5 text-ui-micro leading-[1.4] text-muted-foreground" type="button" onclick={onClose}>Close</Button>
      {/if}
    </div>
  {/if}
</div>

