<script lang="ts">
  /**
   * GitHub Actions log viewer — the main-editor side of run details.
   *
   * Renders the parsed job → step → group tree produced by `runLogModel`:
   * collapsible sections, level tints for errors/warnings, optional line
   * wrapping, live tailing, search filtering + safe highlighting, copy and
   * incremental rendering for very large logs (the backend caps at ~1.5 MB
   * but that can still be tens of thousands of lines).
   *
   * The component is presentational: the caller owns fetching and passes
   * `text`/`loading`/`error`. `text` may stay stale while a refresh is in
   * flight; the toolbar shows "Refreshing…" without clearing the view.
   */
  import { tick, untrack } from "svelte";
  import { toast } from "$lib/toast";
  import {
    activityDurationLabel,
    runDurationLabel,
    runStateIcon,
    runStateKind,
    stepStateClass,
  } from "./forgeModel";
  import {
    countLogMatches,
    filterParsedLog,
    firstErrorLine,
    formatLogTime,
    highlightText,
    parseWorkflowLog,
    type KnownJobMeta,
    type LogJob,
    type LogLine,
    type LogSegment,
    type LogStep,
  } from "./runLogModel";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Toggle } from "$lib/components/ui/toggle/index.js";

  let {
    text,
    jobs = [],
    loading = false,
    error = null,
    emptyText = "No log output for this selection.",
    live = false,
    liveConsole = false,
    onOpenLive = undefined,
    windowSize = 2_000,
    targetStepKey = null,
  }: {
    /** Raw `gh run view --log` text (`null` before the first fetch). */
    text: string | null;
    /** Known jobs with their steps (from workflow run details) for step correlation. */
    jobs?: KnownJobMeta[];
    /** A fetch is in flight (stale text stays visible). */
    loading?: boolean;
    /** Fetch error (`text` may still hold the previous payload). */
    error?: string | null;
    /** Shown when a successful fetch returned no output. */
    emptyText?: string;
    /** Enable the live-tail affordance (active runs). */
    live?: boolean;
    /** The run is still active, so a live console link is useful. */
    liveConsole?: boolean;
    /** Opens the run on GitHub (its live console while jobs are running). */
    onOpenLive?: () => void;
    /** Lines rendered at once; the "Show more" footer adds another window. */
    windowSize?: number;
    /** Target step key (`job\0step`) to expand and scroll into view. */
    targetStepKey?: string | null;
  } = $props();

  // Tailwind replacements for the removed scoped styles: one tone per
  // run-state class, plus the shared toolbar-button utilities.
  const runTone: Record<string, string> = {
    "run-success": "text-[var(--app-added)]",
    "run-failure": "text-[var(--app-deleted)]",
    "run-running": "text-primary",
    "run-queued": "text-muted-foreground",
    "run-cancelled": "text-muted-foreground",
    "run-skipped": "text-muted-foreground opacity-60",
    "run-unknown": "text-muted-foreground",
  };
  // Pill geometry/type only — border, background, hover and disabled chrome
  // come from the ui Button/Toggle primitives this is passed to as `class`.
  const tb =
    "flex-none gap-1 whitespace-nowrap rounded-full px-[0.55rem] py-[0.08rem] text-[0.68rem] leading-[1.4] font-normal text-muted-foreground max-[44rem]:px-[0.4rem]";

  // `windowSize` is overridable so tests (and compact embeds) can keep the
  // DOM small while exercising the same windowing logic.
  const pageSize = $derived(Math.max(1, Math.floor(windowSize)));

  let query = $state("");
  let wrap = $state(false);
  let follow = $state(true);
  let collapsed = $state<Record<string, boolean>>({});
  let limit = $state(untrack(() => pageSize));
  /** First rendered line when not following the tail. */
  let windowStart = $state(0);
  let copied = $state(false);
  let scrollEl = $state<HTMLDivElement | undefined>();
  let highlightedStepKey = $state<string | null>(null);

  const parsed = $derived(text !== null ? parseWorkflowLog(text, jobs) : null);
  const filtered = $derived(parsed ? filterParsedLog(parsed, query) : null);
  const matches = $derived(parsed ? countLogMatches(parsed, query) : 0);
  const total = $derived(filtered?.lineCount ?? 0);
  const firstError = $derived(filtered ? firstErrorLine(filtered) : null);
  const hasTimes = $derived.by(() => {
    if (!parsed) return false;
    for (const job of parsed.jobs) {
      for (const step of job.steps) {
        for (const segment of step.segments) {
          if (segment.lines.some((line) => line.time !== "")) return true;
        }
      }
    }
    return false;
  });

  interface RenderSegment {
    segment: LogSegment;
    offset: number;
  }
  interface RenderStep {
    step: LogStep;
    offset: number;
    segments: RenderSegment[];
  }
  interface RenderJob {
    job: LogJob;
    offset: number;
    steps: RenderStep[];
  }

  /**
   * Flattens the filtered tree with global line offsets so the render cap
   * can cut anywhere and step-relative line numbers stay correct.
   */
  const renderJobs = $derived.by<RenderJob[]>(() => {
    const value = filtered;
    if (!value) return [];
    let offset = 0;
    return value.jobs.map((job) => {
      const jobOffset = offset;
      const steps = job.steps.map((step) => {
        const stepOffset = offset;
        const segments = step.segments.map((segment) => {
          const entry = { segment, offset };
          offset += segment.lines.length;
          return entry;
        });
        return { step, offset: stepOffset, segments };
      });
      return { job, offset: jobOffset, steps };
    });
  });

  // Follow mode anchors the window to the tail; otherwise the window stays
  // where the user left it. Clamp so filtering or a shrinking payload cannot
  // leave the window past the end of the log.
  const renderFrom = $derived(
    follow
      ? Math.max(0, total - limit)
      : Math.max(0, Math.min(windowStart, Math.max(0, total - limit))),
  );
  const renderEnd = $derived(Math.min(total, renderFrom + limit));
  const shown = $derived(Math.max(0, renderEnd - renderFrom));

  /** True when a block of `count` lines overlaps the rendered window. */
  function overlaps(offset: number, count: number): boolean {
    return count > 0 && offset + count > renderFrom && offset < renderEnd;
  }

  function lineLabel(count: number): string {
    return `${count} line${count === 1 ? "" : "s"}`;
  }

  function toggle(key: string): void {
    collapsed = { ...collapsed, [key]: !collapsed[key] };
  }

  function expandAll(): void {
    collapsed = {};
  }

  function collapseAll(): void {
    const value = filtered;
    if (!value) return;
    const next: Record<string, boolean> = {};
    for (const job of value.jobs) {
      next[job.key] = true;
      for (const step of job.steps) {
        next[step.key] = true;
        for (const segment of step.segments) {
          if (segment.kind === "group") next[segment.key] = true;
        }
      }
    }
    collapsed = next;
  }

  function stepIcon(conclusion = "", status = ""): string {
    const kind = runStateKind({
      status: status || (conclusion ? "completed" : "in_progress"),
      conclusion,
    });
    return runStateIcon(kind);
  }

  function jobIcon(job: LogJob): string {
    return runStateIcon(
      runStateKind({
        status: job.status ?? (job.conclusion ? "completed" : "in_progress"),
        conclusion: job.conclusion ?? "",
      }),
    );
  }

  function jobStateClass(job: LogJob): string {
    return stepStateClass(job.conclusion ?? "", job.status ?? "");
  }

  /** "3/13 steps" for the job header (`null` when metadata has no steps). */
  function jobStepProgress(job: LogJob): string | null {
    if (job.steps.length === 0) return null;
    const completed = job.steps.filter((step) => step.status === "completed").length;
    return `${completed}/${job.steps.length} steps`;
  }

  $effect(() => {
    const target = targetStepKey;
    if (!target) return;
    // Read the collapse map untracked: this effect must run only when the
    // requested step changes, not every time it writes the expanded state.
    collapsed = untrack(() => {
      const next = { ...collapsed };
      delete next[target];
      const parts = target.split("\u0000");
      if (parts[0]) delete next[parts[0]];
      return next;
    });

    highlightedStepKey = target;
    void tick().then(() => {
      const el = scrollEl?.querySelector<HTMLElement>(
        `[data-step-key="${CSS.escape(target)}"]`,
      );
      if (el) {
        el.scrollIntoView({ behavior: "smooth", block: "nearest" });
      }
    });
    const timer = setTimeout(() => {
      if (highlightedStepKey === target) highlightedStepKey = null;
    }, 2_500);
    return () => clearTimeout(timer);
  });

  /** Ignore the scroll events our own scrolling produces. */
  let ignoreScroll = false;

  function releaseScrollGuard(): void {
    const release = () => {
      ignoreScroll = false;
    };
    if (typeof requestAnimationFrame === "function") {
      requestAnimationFrame(release);
    } else {
      setTimeout(release, 0);
    }
  }

  function scrollToBottom(): void {
    ignoreScroll = true;
    void tick().then(() => {
      if (scrollEl) scrollEl.scrollTop = scrollEl.scrollHeight;
      releaseScrollGuard();
    });
  }

  function toggleFollow(): void {
    if (follow) {
      // Freeze the current tail window where it is before leaving follow mode.
      windowStart = renderFrom;
      follow = false;
    } else {
      follow = true;
      scrollToBottom();
    }
  }

  /**
   * Scrolls to the first error, expanding its job/step/group first and
   * moving the render window when the line sits outside it.
   */
  function jumpToFirstError(): void {
    const target = firstError;
    const value = filtered;
    if (!target || !value) return;
    let offset = 0;
    for (const job of value.jobs) {
      for (const step of job.steps) {
        for (const segment of step.segments) {
          const index = segment.lines.indexOf(target);
          if (index === -1) {
            offset += segment.lines.length;
            continue;
          }
          const lineOffset = offset + index;
          const visible = lineOffset >= renderFrom && lineOffset < renderEnd;
          const next = { ...collapsed };
          delete next[job.key];
          delete next[step.key];
          if (segment.kind === "group") delete next[segment.key];
          collapsed = next;
          follow = false;
          windowStart = visible
            ? renderFrom
            : Math.max(0, lineOffset - Math.floor(limit / 2));
          ignoreScroll = true;
          void tick().then(() => {
            scrollEl
              ?.querySelector<HTMLElement>("[data-first-error]")
              ?.scrollIntoView({ block: "center" });
            releaseScrollGuard();
          });
          return;
        }
      }
    }
  }

  function onScroll(): void {
    if (ignoreScroll) return;
    const el = scrollEl;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 32;
    if (follow && !atBottom) {
      // Keep the rendered window stable; only stop anchoring it to the tail.
      windowStart = renderFrom;
      follow = false;
    } else if (!follow && atBottom && renderEnd >= total) {
      follow = true;
    }
  }

  function clearSearch(): void {
    query = "";
  }

  function onSearchKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      clearSearch();
    }
  }

  async function copyLog(): Promise<void> {
    if (text === null) return;
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
      toast("Logs copied to the clipboard", { kind: "success" });
      setTimeout(() => (copied = false), 1_500);
    } catch {
      toast("Clipboard is not available", { kind: "error" });
    }
  }

  // A new search starts from the first result, not the tail of the old view.
  // `$effect.pre` applies this before the filtered DOM renders.
  let previousQuery = "";
  $effect.pre(() => {
    if (query === previousQuery) return;
    previousQuery = query;
    limit = pageSize;
    windowStart = 0;
    follow = false;
  });

  // A fresh payload (new scope or the first fetch) starts tailing with a
  // full-size window. Collapse/search state survives live refreshes so a
  // streaming run does not re-open sections the user closed.
  let previousTextNull = true;
  $effect(() => {
    const isNull = text === null;
    if (previousTextNull && !isNull) {
      limit = pageSize;
      windowStart = 0;
      follow = true;
    }
    previousTextNull = isNull;
  });

  // Follow the tail while new output arrives (the user can scroll away).
  let previousLength = -1;
  $effect(() => {
    const length = text?.length ?? 0;
    if (length === previousLength) return;
    previousLength = length;
    if (follow) scrollToBottom();
  });

  // Completed logs open on the first failure instead of the noisy tail; a
  // live run keeps tailing unless the user already scrolled away.
  let errorScrollKey = "";
  $effect(() => {
    const value = filtered;
    if (!value) return;
    const first = firstErrorLine(value);
    if (!first) {
      errorScrollKey = "";
      return;
    }
    // While a run is live and tailing, stay on the newest output. Once it
    // completes, fall back to the first failure so it is not buried.
    const key = `${live ? "live" : "done"}\u0000${text?.length ?? 0}\u0000${query}`;
    if (key === errorScrollKey) return;
    errorScrollKey = key;
    if (live && follow) return;
    follow = false;
    jumpToFirstError();
  });
</script>

{#snippet logLine(
  line: LogLine,
  number: number,
  isFirstError: boolean,
)}
  <div
    class="log-line flex min-w-full items-start gap-2 border-l-2 border-transparent pr-[0.7rem] hover:bg-foreground/5 {wrap
      ? "w-full"
      : "w-max"} {line.level === "error" ? "border-l-destructive bg-destructive/10" : ""} {line.level ===
    "warning"
      ? "border-l-muted-foreground bg-muted/50"
      : ""}"
    class:level-error={line.level === "error"}
    class:level-warning={line.level === "warning"}
    class:level-meta={line.level === "command" || line.level === "debug"}
    data-first-error={isFirstError ? "true" : undefined}
  >
    <span class="ln w-12 flex-none select-none pr-[0.15rem] text-right text-[0.66rem] leading-[inherit] text-muted-foreground opacity-55" aria-hidden="true">{number}</span>
    {#if hasTimes}
      <span class="lt flex-none select-none text-[0.66rem] text-muted-foreground opacity-70" aria-hidden="true">{formatLogTime(line.time)}</span>
    {/if}
    <span
      class="lm min-w-0 flex-1 {wrap
        ? "whitespace-pre-wrap [overflow-wrap:anywhere]"
        : "whitespace-pre"} {line.level === "error"
        ? "text-destructive"
        : ""} {line.level === "command" || line.level === "debug"
        ? "text-muted-foreground opacity-85"
        : ""}"
      >{#each highlightText(line.text, query) as part, index (index)}{#if part.match}<mark
            class="rounded-[2px] bg-accent text-accent-foreground"
            >{part.text}</mark
          >{:else}{part.text}{/if}{/each}</span
    >
  </div>
{/snippet}

<div
  class="log-viewer flex h-full min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-sm border border-border bg-background"
  class:wrap
>
  <div class="log-toolbar flex-none flex flex-wrap items-center gap-[0.3rem] border-b border-border bg-popover px-2 py-[0.35rem]" role="toolbar" aria-label="Log controls">
    <div class="log-search relative flex min-w-32 flex-[1_1_11rem] max-w-[22rem] items-center">
      <span class="search-icon pointer-events-none absolute left-[0.45rem] text-[0.9rem] text-muted-foreground" aria-hidden="true">⌕</span>
      <Input
        class="log-search-input h-auto w-full rounded-full py-[0.16rem] pl-[1.65rem] pr-[1.5rem] text-[0.72rem] [&::-webkit-search-cancel-button]:hidden"
        type="search"
        aria-label="Search logs"
        placeholder="Find in logs…"
        spellcheck="false"
        bind:value={query}
        onkeydown={onSearchKeydown}
      />
      {#if query.trim()}
        <Button
          class="clear-search absolute right-[0.3rem] h-auto px-[0.2rem] py-0 text-[0.9rem] leading-none font-normal text-muted-foreground hover:text-foreground dark:hover:bg-transparent"
          variant="ghost"
          type="button"
          aria-label="Clear log search"
          title="Clear search"
          onclick={clearSearch}
        >
          ×
        </Button>
      {/if}
    </div>
    <span class="log-count flex-none whitespace-nowrap text-[0.68rem] text-muted-foreground max-[44rem]:order-2" aria-live="polite">
      {#if query.trim()}
        {matches} hit{matches === 1 ? "" : "s"} · {lineLabel(total)}
      {:else}
        {lineLabel(total)}
      {/if}
      {#if loading && text !== null}
        <span class="refreshing italic"> · refreshing…</span>
      {/if}
    </span>
    {#if live}
      <span class="live-badge flex-none inline-flex items-center gap-1 whitespace-nowrap text-[0.66rem] font-semibold text-primary" title="Following this run while GitHub publishes output">
        <span class="live-dot size-[0.42rem] animate-pulse rounded-full bg-primary" aria-hidden="true"></span>
        Live
      </span>
    {/if}
    {#if firstError}
      <Button
        class={tb}
        variant="outline"
        type="button"
        title="Jump to the first error line"
        onclick={jumpToFirstError}
      >
        First error
      </Button>
    {/if}
    <span class="spacer ml-auto max-[44rem]:hidden"></span>
    {#if liveConsole && onOpenLive}
      <Button
        class="live-console {tb} border-current/40 text-primary"
        variant="outline"
        type="button"
        title="Open GitHub's live console for this run"
        onclick={onOpenLive}
      >
        <span class="live-dot size-[0.42rem] animate-pulse rounded-full bg-primary" aria-hidden="true"></span>
        Live console
      </Button>
    {/if}
    <Toggle
      class="{tb} {wrap ? "border-transparent bg-accent text-accent-foreground" : ""} focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary focus-visible:ring-0"
      pressed={wrap}
      onPressedChange={() => (wrap = !wrap)}
      title="Wrap long log lines"
    >
      Wrap
    </Toggle>
    <Toggle
      class="{tb} {follow ? "border-transparent bg-accent text-accent-foreground" : ""} focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary focus-visible:ring-0"
      pressed={follow}
      onPressedChange={() => toggleFollow()}
      title={follow ? "Stop following new output" : "Follow new output"}
    >
      {#if live}<span class="live-dot size-[0.42rem] animate-pulse rounded-full bg-primary" aria-hidden="true"></span>{/if}
      {follow ? "Following" : "Tail"}
    </Toggle>
    <Button class={tb} variant="outline" type="button" title="Expand all sections" onclick={expandAll}>
      Expand
    </Button>
    <Button
      class={tb}
      variant="outline"
      type="button"
      title="Collapse all sections"
      onclick={collapseAll}
    >
      Collapse
    </Button>
    <Button
      class={tb}
      variant="outline"
      type="button"
      title="Copy the raw log to the clipboard"
      disabled={text === null}
      onclick={() => void copyLog()}
    >
      {copied ? "Copied" : "Copy"}
    </Button>
  </div>

  {#if error}
    <div class="log-error flex-none flex flex-wrap items-baseline gap-[0.4rem] border-b border-current/25 bg-destructive/15 px-[0.6rem] py-[0.35rem] text-xs text-destructive" role="alert">
      <span class="err-mark flex-none inline-flex size-4 items-center justify-center rounded-full border border-current text-[0.625rem] font-bold" aria-hidden="true">!</span>
      <span class="err-text min-w-0 [overflow-wrap:anywhere]">{error}</span>
      {#if text !== null}
        <span class="err-hint text-[0.66rem] opacity-75">Showing the last available output.</span>
      {/if}
    </div>
  {/if}

  {#if text === null}
    <div class="log-state flex min-h-0 flex-1 flex-col items-center justify-center gap-[0.45rem] p-6 text-center text-[0.78rem] text-muted-foreground" aria-live="polite">
      {#if loading}
        <span class="state-spinner flex-none size-[1.1rem] animate-spin rounded-full border-2 border-border border-t-primary" aria-hidden="true"></span>
        <p class="m-0">Loading logs…</p>
        <p class="state-hint m-0 max-w-[26rem] text-[0.7rem] leading-[1.45] opacity-85">
          GitHub may take a moment to make logs available while jobs are queued.
        </p>
      {:else if error}
        <p class="m-0">Logs are not available yet.</p>
        <p class="state-hint m-0 max-w-[26rem] text-[0.7rem] leading-[1.45] opacity-85">
          Retry in a moment, or open the run on GitHub for the live console.
        </p>
      {:else}
        <p class="m-0">{emptyText}</p>
      {/if}
    </div>
  {:else if filtered && filtered.jobs.length === 0}
    {#if query.trim()}
      <div class="log-state flex min-h-0 flex-1 flex-col items-center justify-center gap-[0.45rem] p-6 text-center text-[0.78rem] text-muted-foreground">
        <p class="m-0">No log lines match “{query.trim()}”.</p>
        <Button class={tb} variant="outline" type="button" onclick={clearSearch}>Clear search</Button>
      </div>
    {:else}
      <div class="log-state flex min-h-0 flex-1 flex-col items-center justify-center gap-[0.45rem] p-6 text-center text-[0.78rem] text-muted-foreground">
        <p class="m-0">{emptyText}</p>
      </div>
    {/if}
  {:else}
    {#if live && total === 0 && query.trim() === ""}
      <div class="live-note flex-none flex flex-wrap items-center gap-[0.45rem] border-b border-border bg-muted/35 px-[0.6rem] py-[0.4rem] text-xs leading-[1.4] text-foreground" role="status">
        <span class="live-dot size-[0.42rem] animate-pulse rounded-full bg-primary" aria-hidden="true"></span>
        <span class="live-note-text min-w-0 flex-[1_1_16rem]">
          GitHub publishes each job's log when it finishes. For output while a
          job is still running, use the live console on GitHub.
        </span>
        {#if onOpenLive}
          <Button class={tb} variant="outline" type="button" onclick={onOpenLive}>
            Open live console
          </Button>
        {/if}
      </div>
    {/if}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (scrollable log region must be keyboard focusable) -->
    <div
      class="log-scroll min-h-0 flex-1 overflow-auto bg-background pb-[0.6rem] pt-[0.35rem] font-mono text-[0.725rem] leading-[1.55] text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
      bind:this={scrollEl}
      onscroll={onScroll}
      tabindex="0"
      role="region"
      aria-label="Workflow logs"
    >
      {#each renderJobs as jobEntry (jobEntry.job.key)}
        {#if jobEntry.job.lineCount === 0 || overlaps(jobEntry.offset, jobEntry.job.lineCount)}
          {@const jobDuration = activityDurationLabel(
            jobEntry.job.started_at ?? null,
            jobEntry.job.completed_at ?? null,
            jobEntry.job.status ?? (jobEntry.job.conclusion ? "completed" : ""),
          )}
          <section class="log-job border-b border-border last:border-none" aria-label={jobEntry.job.name}>
            <button
              class="log-job-head sticky top-0 z-[2] flex w-full min-w-0 cursor-pointer items-center gap-[0.4rem] border-none bg-popover px-[0.6rem] py-[0.3rem] text-left text-[0.76rem] font-semibold text-inherit hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
              type="button"
              aria-expanded={!collapsed[jobEntry.job.key]}
              onclick={() => toggle(jobEntry.job.key)}
            >
              <span class="caret w-[0.8rem] flex-none text-[0.66rem] text-muted-foreground" aria-hidden="true">
                {collapsed[jobEntry.job.key] ? "▸" : "▾"}
              </span>
              <span
                class="step-status-icon flex-none inline-flex w-4 items-center justify-center text-[0.72rem] leading-none font-bold {jobStateClass(jobEntry.job)} {runTone[jobStateClass(jobEntry.job)] ?? "text-muted-foreground"}"
                aria-hidden="true"
              >
                {jobIcon(jobEntry.job)}
              </span>
              <span class="job-name min-w-0 truncate font-semibold">{jobEntry.job.name}</span>
              <span class="spacer ml-auto"></span>
              {#if jobStepProgress(jobEntry.job)}
                <span class="muted text-[0.66rem] font-normal text-muted-foreground">{jobStepProgress(jobEntry.job)}</span>
              {/if}
              <span class="muted text-[0.66rem] font-normal text-muted-foreground">{lineLabel(jobEntry.job.lineCount)}</span>
              {#if jobEntry.job.errorCount > 0}
                <span class="mini-chip flex-none rounded-full bg-destructive/15 px-[0.4rem] text-[0.62rem] leading-[1.5] font-medium text-destructive">
                  {jobEntry.job.errorCount} error{jobEntry.job.errorCount === 1 ? "" : "s"}
                </span>
              {/if}
              {#if jobEntry.job.warningCount > 0}
                <span class="mini-chip flex-none rounded-full bg-muted px-[0.4rem] text-[0.62rem] leading-[1.5] font-medium text-muted-foreground">
                  {jobEntry.job.warningCount} warning{jobEntry.job.warningCount === 1
                    ? ""
                    : "s"}
                </span>
              {/if}
              {#if jobDuration}
                <span class="muted job-duration mr-[0.35rem] text-[0.66rem] font-normal text-muted-foreground">{jobDuration}</span>
              {/if}
            </button>

            {#if !collapsed[jobEntry.job.key]}
              {#each jobEntry.steps as stepEntry (stepEntry.step.key)}
                {@const step = stepEntry.step}
                {@const isStepCollapsed = !!collapsed[step.key]}
                {@const duration =
                  activityDurationLabel(
                    step.started_at ?? null,
                    step.completed_at ?? null,
                    step.status ?? (step.conclusion ? "completed" : ""),
                  ) ||
                  runDurationLabel(step.firstTime, step.lastTime)}
                <section
                  class="log-step border-t border-border/40 transition-colors duration-200 {highlightedStepKey === step.key
                    ? "bg-primary/[0.14] outline-1 outline-primary"
                    : ""}"
                  class:step-failed={step.conclusion === "failure" || step.errorCount > 0}
                  class:step-running={step.status === "in_progress"}
                  class:highlighted={highlightedStepKey === step.key}
                  data-step-key={step.key}
                  aria-label={step.name}
                >
                  <button
                    class="log-step-head flex w-full min-w-0 cursor-pointer items-center gap-[0.4rem] border-none px-[0.6rem] py-[0.24rem] pl-[1.1rem] text-left text-[0.72rem] text-inherit hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary {step.conclusion === "failure" ||
                    step.errorCount > 0
                      ? "border-l-[3px] border-l-destructive bg-destructive/15"
                      : step.status === "in_progress"
                        ? "border-l-[3px] border-l-primary bg-card"
                        : "bg-card"}"
                    type="button"
                    aria-expanded={!isStepCollapsed}
                    onclick={() => toggle(step.key)}
                  >
                    <span class="caret w-[0.8rem] flex-none text-[0.66rem] text-muted-foreground" aria-hidden="true">
                      {isStepCollapsed ? "▸" : "▾"}
                    </span>
                    <span
                      class="step-status-icon flex-none inline-flex w-4 items-center justify-center text-[0.72rem] leading-none font-bold {stepStateClass(step.conclusion ?? "", step.status ?? "")} {runTone[stepStateClass(step.conclusion ?? "", step.status ?? "")] ?? "text-muted-foreground"}"
                      aria-hidden="true"
                    >
                      {stepIcon(step.conclusion, step.status)}
                    </span>
                    <span class="step-name min-w-0 truncate">{step.name}</span>
                    <span class="spacer ml-auto"></span>
                    {#if step.lineCount > 0}
                      <span class="muted step-lines mr-[0.35rem] text-[0.66rem] font-normal text-muted-foreground">{lineLabel(step.lineCount)}</span>
                    {/if}
                    {#if step.errorCount > 0}
                      <span class="mini-chip flex-none rounded-full bg-destructive/15 px-[0.4rem] text-[0.62rem] leading-[1.5] font-medium text-destructive">
                        {step.errorCount} error{step.errorCount === 1 ? "" : "s"}
                      </span>
                    {/if}
                    {#if step.warningCount > 0}
                      <span class="mini-chip flex-none rounded-full bg-muted px-[0.4rem] text-[0.62rem] leading-[1.5] font-medium text-muted-foreground">
                        {step.warningCount} warning{step.warningCount === 1 ? "" : "s"}
                      </span>
                    {/if}
                    {#if step.conclusion === "skipped"}
                      <span class="muted step-state-badge mr-[0.35rem] text-[0.66rem] font-normal text-muted-foreground">Skipped</span>
                    {/if}
                    {#if duration}
                      <span class="muted step-time ml-auto text-[0.66rem] font-normal text-muted-foreground">{duration}</span>
                    {/if}
                  </button>

                  {#if !isStepCollapsed}
                    {#if step.lineCount === 0}
                      <div class="empty-step-notice flex items-center gap-[0.4rem] py-[0.45rem] pr-4 pl-[2.6rem] text-[0.68rem] italic text-muted-foreground opacity-80">
                        {#if step.conclusion === "skipped"}
                          This step was skipped.
                        {:else if step.status === "queued" || step.status === "in_progress"}
                          <span class="state-spinner small flex-none size-[0.7rem] animate-spin rounded-full border-[1.5px] border-border border-t-primary" aria-hidden="true"></span>
                          <span>
                            {step.status === "in_progress"
                              ? "Step is running — its log appears when the job finishes."
                              : "Step is queued…"}
                          </span>
                        {:else if jobEntry.job.status !== "completed"}
                          <span>
                            {jobEntry.job.status === "queued"
                              ? "Job is queued — logs appear once it starts."
                              : "Logs for this step appear when the job finishes."}
                          </span>
                        {:else}
                          No log output recorded for this step.
                        {/if}
                      </div>
                    {:else}
                      {#each stepEntry.segments as segmentEntry (segmentEntry.segment.key)}
                        {#if overlaps(
                          segmentEntry.offset,
                          segmentEntry.segment.lines.length,
                        )}
                          {#if segmentEntry.segment.kind === "group"}
                            <div class="log-group m-0">
                              <button
                                class="log-group-head flex w-full min-w-0 cursor-pointer items-center gap-[0.4rem] border-none bg-transparent px-[0.6rem] py-[0.18rem] pl-[2.1rem] text-left text-[0.7rem] text-muted-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                                type="button"
                                aria-expanded={!collapsed[segmentEntry.segment.key]}
                                onclick={() => toggle(segmentEntry.segment.key)}
                              >
                                <span class="caret w-[0.8rem] flex-none text-[0.66rem] text-muted-foreground" aria-hidden="true">
                                  {collapsed[segmentEntry.segment.key] ? "▸" : "▾"}
                                </span>
                                <span class="group-title min-w-0 truncate italic">
                                  {segmentEntry.segment.title}
                                </span>
                                <span class="muted text-[0.66rem] font-normal text-muted-foreground">
                                  {lineLabel(segmentEntry.segment.lines.length)}
                                </span>
                              </button>
                              {#if !collapsed[segmentEntry.segment.key]}
                                <div class="group-body ml-[2.4rem] border-l-2 border-border">
                                  {#each segmentEntry.segment.lines as line, index (index)}
                                    {@const offset = segmentEntry.offset + index}
                                    {#if offset >= renderFrom && offset < renderEnd}
                                      {@render logLine(
                                        line,
                                        offset - stepEntry.offset + 1,
                                        line === firstError,
                                      )}
                                    {/if}
                                  {/each}
                                </div>
                              {/if}
                            </div>
                          {:else}
                            {#each segmentEntry.segment.lines as line, index (index)}
                              {@const offset = segmentEntry.offset + index}
                              {#if offset >= renderFrom && offset < renderEnd}
                                {@render logLine(
                                  line,
                                  offset - stepEntry.offset + 1,
                                  line === firstError,
                                )}
                              {/if}
                            {/each}
                          {/if}
                        {/if}
                      {/each}
                    {/if}
                  {/if}
                </section>
              {/each}
            {/if}
          </section>
        {/if}
      {/each}
    </div>

    {#if total > shown}
      <div class="log-more flex-none flex flex-wrap items-center justify-center gap-[0.6rem] border-t border-border bg-card px-[0.6rem] py-[0.3rem] text-[0.68rem] text-muted-foreground">
        <span>
          Showing lines {renderFrom + 1}–{renderEnd} of {total}
        </span>
        <Button
          class={tb}
          variant="outline"
          type="button"
          onclick={() => (limit += pageSize)}
        >
          Show {Math.min(pageSize, total - shown)} more
        </Button>
      </div>
    {/if}
  {/if}
</div>
