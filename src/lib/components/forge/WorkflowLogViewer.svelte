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
  import { runDurationLabel } from "./forgeModel";
  import {
    countLogMatches,
    filterParsedLog,
    firstErrorLine,
    formatLogTime,
    highlightText,
    parseWorkflowLog,
    type LogJob,
    type LogLine,
    type LogSegment,
    type LogStep,
  } from "./runLogModel";

  let {
    text,
    loading = false,
    error = null,
    emptyText = "No log output for this selection.",
    live = false,
    windowSize = 2_000,
  }: {
    /** Raw `gh run view --log` text (`null` before the first fetch). */
    text: string | null;
    /** A fetch is in flight (stale text stays visible). */
    loading?: boolean;
    /** Fetch error (`text` may still hold the previous payload). */
    error?: string | null;
    /** Shown when a successful fetch returned no output. */
    emptyText?: string;
    /** Enable the live-tail affordance (active runs). */
    live?: boolean;
    /** Lines rendered at once; the "Show more" footer adds another window. */
    windowSize?: number;
  } = $props();

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

  const parsed = $derived(text !== null ? parseWorkflowLog(text) : null);
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
    class="log-line"
    class:level-error={line.level === "error"}
    class:level-warning={line.level === "warning"}
    class:level-meta={line.level === "command" || line.level === "debug"}
    data-first-error={isFirstError ? "true" : undefined}
  >
    <span class="ln" aria-hidden="true">{number}</span>
    {#if hasTimes}
      <span class="lt" aria-hidden="true">{formatLogTime(line.time)}</span>
    {/if}
    <span class="lm"
      >{#each highlightText(line.text, query) as part, index (index)}{#if part.match}<mark
            >{part.text}</mark
          >{:else}{part.text}{/if}{/each}</span
    >
  </div>
{/snippet}

<div class="log-viewer" class:wrap>
  <div class="log-toolbar" role="toolbar" aria-label="Log controls">
    <div class="log-search">
      <span class="search-icon" aria-hidden="true">⌕</span>
      <input
        class="log-search-input"
        type="search"
        aria-label="Search logs"
        placeholder="Find in logs…"
        spellcheck="false"
        bind:value={query}
        onkeydown={onSearchKeydown}
      />
      {#if query.trim()}
        <button
          class="clear-search"
          type="button"
          aria-label="Clear log search"
          title="Clear search"
          onclick={clearSearch}
        >
          ×
        </button>
      {/if}
    </div>
    <span class="log-count" aria-live="polite">
      {#if query.trim()}
        {matches} hit{matches === 1 ? "" : "s"} · {lineLabel(total)}
      {:else}
        {lineLabel(total)}
      {/if}
      {#if loading && text !== null}
        <span class="refreshing"> · refreshing…</span>
      {/if}
    </span>
    {#if firstError}
      <button
        class="tb"
        type="button"
        title="Jump to the first error line"
        onclick={jumpToFirstError}
      >
        First error
      </button>
    {/if}
    <span class="spacer"></span>
    <button
      class="tb"
      class:active={wrap}
      type="button"
      aria-pressed={wrap}
      title="Wrap long log lines"
      onclick={() => (wrap = !wrap)}
    >
      Wrap
    </button>
    <button
      class="tb"
      class:active={follow}
      type="button"
      aria-pressed={follow}
      title={follow ? "Stop following new output" : "Follow new output"}
      onclick={toggleFollow}
    >
      {#if live}<span class="live-dot" aria-hidden="true"></span>{/if}
      {follow ? "Following" : "Tail"}
    </button>
    <button class="tb" type="button" title="Expand all sections" onclick={expandAll}>
      Expand
    </button>
    <button
      class="tb"
      type="button"
      title="Collapse all sections"
      onclick={collapseAll}
    >
      Collapse
    </button>
    <button
      class="tb"
      type="button"
      title="Copy the raw log to the clipboard"
      disabled={text === null}
      onclick={() => void copyLog()}
    >
      {copied ? "Copied" : "Copy"}
    </button>
  </div>

  {#if error}
    <div class="log-error" role="alert">
      <span class="err-mark" aria-hidden="true">!</span>
      <span class="err-text">{error}</span>
      {#if text !== null}
        <span class="err-hint">Showing the last available output.</span>
      {/if}
    </div>
  {/if}

  {#if text === null}
    <div class="log-state" aria-live="polite">
      {#if loading}
        <span class="state-spinner" aria-hidden="true"></span>
        <p>Loading logs…</p>
        <p class="state-hint">
          GitHub may take a moment to make logs available while jobs are queued.
        </p>
      {:else if error}
        <p>Logs are not available yet.</p>
        <p class="state-hint">
          Retry in a moment, or open the run on GitHub for the live console.
        </p>
      {:else}
        <p>{emptyText}</p>
      {/if}
    </div>
  {:else if filtered && filtered.jobs.length === 0}
    {#if query.trim()}
      <div class="log-state">
        <p>No log lines match “{query.trim()}”.</p>
        <button class="tb" type="button" onclick={clearSearch}>Clear search</button>
      </div>
    {:else}
      <div class="log-state">
        <p>{emptyText}</p>
      </div>
    {/if}
  {:else}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (scrollable log region must be keyboard focusable) -->
    <div
      class="log-scroll"
      bind:this={scrollEl}
      onscroll={onScroll}
      tabindex="0"
      role="region"
      aria-label="Workflow logs"
    >
      {#each renderJobs as jobEntry (jobEntry.job.key)}
        {#if overlaps(jobEntry.offset, jobEntry.job.lineCount)}
          <section class="log-job" aria-label={jobEntry.job.name}>
            <button
              class="log-job-head"
              type="button"
              aria-expanded={!collapsed[jobEntry.job.key]}
              onclick={() => toggle(jobEntry.job.key)}
            >
              <span class="caret" aria-hidden="true">
                {collapsed[jobEntry.job.key] ? "▸" : "▾"}
              </span>
              <span class="job-name">{jobEntry.job.name}</span>
              <span class="muted">{lineLabel(jobEntry.job.lineCount)}</span>
              {#if jobEntry.job.errorCount > 0}
                <span class="mini-chip chip-error">
                  {jobEntry.job.errorCount} error{jobEntry.job.errorCount === 1 ? "" : "s"}
                </span>
              {/if}
              {#if jobEntry.job.warningCount > 0}
                <span class="mini-chip chip-warning">
                  {jobEntry.job.warningCount} warning{jobEntry.job.warningCount === 1
                    ? ""
                    : "s"}
                </span>
              {/if}
            </button>

            {#if !collapsed[jobEntry.job.key]}
              {#each jobEntry.steps as stepEntry (stepEntry.step.key)}
                {#if overlaps(stepEntry.offset, stepEntry.step.lineCount)}
                  {@const duration = runDurationLabel(
                    stepEntry.step.firstTime,
                    stepEntry.step.lastTime,
                  )}
                  <section class="log-step" aria-label={stepEntry.step.name}>
                    <button
                      class="log-step-head"
                      type="button"
                      aria-expanded={!collapsed[stepEntry.step.key]}
                      onclick={() => toggle(stepEntry.step.key)}
                    >
                      <span class="caret" aria-hidden="true">
                        {collapsed[stepEntry.step.key] ? "▸" : "▾"}
                      </span>
                      <span class="step-name">{stepEntry.step.name}</span>
                      <span class="muted">{lineLabel(stepEntry.step.lineCount)}</span>
                      {#if stepEntry.step.errorCount > 0}
                        <span class="mini-chip chip-error">
                          {stepEntry.step.errorCount} error{stepEntry.step.errorCount === 1
                            ? ""
                            : "s"}
                        </span>
                      {/if}
                      {#if duration}
                        <span class="muted step-time">{duration}</span>
                      {/if}
                    </button>

                    {#if !collapsed[stepEntry.step.key]}
                      {#each stepEntry.segments as segmentEntry (segmentEntry.segment.key)}
                        {#if overlaps(
                          segmentEntry.offset,
                          segmentEntry.segment.lines.length,
                        )}
                          {#if segmentEntry.segment.kind === "group"}
                            <div class="log-group">
                              <button
                                class="log-group-head"
                                type="button"
                                aria-expanded={!collapsed[segmentEntry.segment.key]}
                                onclick={() => toggle(segmentEntry.segment.key)}
                              >
                                <span class="caret" aria-hidden="true">
                                  {collapsed[segmentEntry.segment.key] ? "▸" : "▾"}
                                </span>
                                <span class="group-title">
                                  {segmentEntry.segment.title}
                                </span>
                                <span class="muted">
                                  {lineLabel(segmentEntry.segment.lines.length)}
                                </span>
                              </button>
                              {#if !collapsed[segmentEntry.segment.key]}
                                <div class="group-body">
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
                  </section>
                {/if}
              {/each}
            {/if}
          </section>
        {/if}
      {/each}
    </div>

    {#if total > shown}
      <div class="log-more">
        <span>
          Showing lines {renderFrom + 1}–{renderEnd} of {total}
        </span>
        <button
          class="tb"
          type="button"
          onclick={() => (limit += pageSize)}
        >
          Show {Math.min(pageSize, total - shown)} more
        </button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .log-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    min-width: 0;
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-small, 8px);
    overflow: hidden;
  }

  .log-toolbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.3rem;
    flex-wrap: wrap;
    padding: 0.35rem 0.5rem;
    border-bottom: 1px solid var(--app-border, var(--m3-outline-variant));
    background: var(--m3-surface-container, var(--m3-surface));
  }

  .log-search {
    position: relative;
    display: flex;
    align-items: center;
    flex: 1 1 11rem;
    max-width: 22rem;
    min-width: 8rem;
  }

  .search-icon {
    position: absolute;
    left: 0.45rem;
    color: var(--m3-on-surface-variant);
    font-size: 0.9rem;
    pointer-events: none;
  }

  .log-search-input {
    width: 100%;
    border: 1px solid var(--app-border, var(--m3-outline-variant));
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
    color: var(--m3-on-surface);
    font: inherit;
    font-size: 0.72rem;
    padding: 0.16rem 1.5rem 0.16rem 1.65rem;
  }

  .log-search-input:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .log-search-input::-webkit-search-cancel-button {
    display: none;
  }

  .clear-search {
    position: absolute;
    right: 0.3rem;
    border: none;
    background: none;
    color: var(--m3-on-surface-variant);
    font-size: 0.9rem;
    line-height: 1;
    cursor: pointer;
    padding: 0 0.2rem;
  }

  .clear-search:hover {
    color: var(--m3-on-surface);
  }

  .log-count {
    flex: none;
    font-size: 0.68rem;
    color: var(--m3-on-surface-variant);
    white-space: nowrap;
  }

  .refreshing {
    font-style: italic;
  }

  .spacer {
    margin-left: auto;
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
    padding: 0.08rem 0.55rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .tb:hover:not(:disabled) {
    background: var(--app-hover, var(--m3-surface-container-high));
    color: var(--m3-on-surface);
  }

  .tb:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .tb:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .tb.active {
    background: var(--m3-primary-container);
    border-color: transparent;
    color: var(--m3-on-primary-container);
  }

  .live-dot {
    width: 0.42rem;
    height: 0.42rem;
    border-radius: 50%;
    background: var(--m3-primary);
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

  .log-error {
    flex: none;
    display: flex;
    align-items: baseline;
    gap: 0.4rem;
    flex-wrap: wrap;
    padding: 0.35rem 0.6rem;
    background: var(--m3-error-container);
    color: var(--m3-on-error-container);
    font-size: 0.72rem;
    border-bottom: 1px solid color-mix(in srgb, currentColor 25%, transparent);
  }

  .err-mark {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1rem;
    height: 1rem;
    border: 1px solid currentColor;
    border-radius: 50%;
    font-size: 0.625rem;
    font-weight: 700;
    flex: none;
  }

  .err-text {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .err-hint {
    opacity: 0.75;
    font-size: 0.66rem;
  }

  .log-state {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.45rem;
    padding: 1.5rem;
    text-align: center;
    color: var(--m3-on-surface-variant);
    font-size: 0.78rem;
  }

  .log-state p {
    margin: 0;
  }

  .state-hint {
    max-width: 26rem;
    font-size: 0.7rem;
    line-height: 1.45;
    opacity: 0.85;
  }

  .state-spinner {
    width: 1.1rem;
    height: 1.1rem;
    border: 2px solid var(--m3-outline-variant);
    border-top-color: var(--m3-primary);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .log-scroll {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0.35rem 0 0.6rem;
    font-family: var(--app-font-mono, ui-monospace, Consolas, monospace);
    font-size: 0.725rem;
    line-height: 1.55;
    color: var(--m3-on-surface);
    background: var(--m3-surface-container-lowest, var(--m3-surface));
  }

  .log-scroll:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .log-job {
    border-bottom: 1px solid var(--m3-outline-variant);
  }

  .log-job:last-child {
    border-bottom: none;
  }

  .log-job-head,
  .log-step-head,
  .log-group-head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    width: 100%;
    border: none;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
    min-width: 0;
  }

  .log-job-head {
    position: sticky;
    top: 0;
    z-index: 2;
    padding: 0.3rem 0.6rem;
    background: var(--m3-surface-container);
    font-size: 0.76rem;
    font-weight: 600;
  }

  .log-job-head:hover,
  .log-step-head:hover,
  .log-group-head:hover {
    background: var(--m3-surface-container-high, var(--m3-surface-container));
  }

  .log-job-head:focus-visible,
  .log-step-head:focus-visible,
  .log-group-head:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .log-step-head {
    padding: 0.22rem 0.6rem 0.22rem 1.35rem;
    background: var(--m3-surface-container-low, var(--m3-surface));
    border-top: 1px solid color-mix(in srgb, var(--m3-outline-variant) 55%, transparent);
    font-size: 0.72rem;
  }

  .log-group-head {
    padding: 0.18rem 0.6rem 0.18rem 2.1rem;
    color: var(--m3-on-surface-variant);
    font-size: 0.7rem;
  }

  .caret {
    flex: none;
    width: 0.8rem;
    color: var(--m3-on-surface-variant);
    font-size: 0.66rem;
  }

  .job-name,
  .step-name,
  .group-title {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .job-name {
    font-weight: 600;
  }

  .group-title {
    font-style: italic;
  }

  .step-time {
    margin-left: auto;
  }

  .muted {
    color: var(--m3-on-surface-variant);
    font-size: 0.66rem;
    font-weight: 400;
  }

  .mini-chip {
    flex: none;
    border-radius: var(--m3-shape-full, 9999px);
    font-size: 0.62rem;
    line-height: 1.5;
    padding: 0 0.4rem;
    font-weight: 500;
  }

  .chip-error {
    background: var(--m3-error-container);
    color: var(--m3-on-error-container);
  }

  .chip-warning {
    background: var(--m3-tertiary-container);
    color: var(--m3-on-tertiary-container);
  }

  .log-group {
    margin: 0;
  }

  .group-body {
    border-left: 2px solid var(--m3-outline-variant);
    margin-left: 2.4rem;
  }

  .log-line {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    width: max-content;
    min-width: 100%;
    padding: 0 0.7rem 0 0;
    border-left: 2px solid transparent;
  }

  .wrap .log-line {
    width: 100%;
  }

  .log-line:hover {
    background: color-mix(in srgb, var(--m3-on-surface) 5%, transparent);
  }

  .ln {
    flex: none;
    width: 3rem;
    padding-right: 0.15rem;
    text-align: right;
    color: var(--m3-on-surface-variant);
    opacity: 0.55;
    user-select: none;
    font-size: 0.66rem;
    line-height: inherit;
  }

  .lt {
    flex: none;
    color: var(--m3-on-surface-variant);
    opacity: 0.7;
    font-size: 0.66rem;
    user-select: none;
  }

  .lm {
    flex: 1;
    min-width: 0;
    white-space: pre;
  }

  .wrap .lm {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .level-error {
    background: color-mix(in srgb, var(--m3-error-container) 55%, transparent);
    border-left-color: var(--m3-error);
  }

  .level-error .lm {
    color: var(--m3-on-error-container);
  }

  .level-warning {
    background: color-mix(in srgb, var(--m3-tertiary-container) 55%, transparent);
    border-left-color: var(--m3-tertiary);
  }

  .level-meta .lm {
    color: var(--m3-on-surface-variant);
    opacity: 0.85;
  }

  mark {
    background: var(--m3-primary-container);
    color: var(--m3-on-primary-container);
    border-radius: 2px;
  }

  .log-more {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.6rem;
    flex-wrap: wrap;
    padding: 0.3rem 0.6rem;
    border-top: 1px solid var(--app-border, var(--m3-outline-variant));
    background: var(--m3-surface-container-low, var(--m3-surface));
    color: var(--m3-on-surface-variant);
    font-size: 0.68rem;
  }

  @media (max-width: 44rem) {
    .log-count {
      order: 2;
    }
    .spacer {
      display: none;
    }
    .tb {
      padding: 0.08rem 0.4rem;
    }
  }
</style>
