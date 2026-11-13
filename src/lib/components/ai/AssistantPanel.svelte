<script lang="ts">
  /**
   * AssistantPanel (M13) — the repo-aware Q&A assistant + app-wide AI
   * activity log, in two tabs.
   *
   * Chat: per-root conversation threads from `ai.chat` (`chat.svelte.ts`
   * owns gathering, prompt assembly, streaming and persistence — this
   * component is presentation + the context-chip selection). Answers are
   * advisory by contract: the panel states it in a persistent banner and
   * offers no apply/mutation affordances. Context chips (staged diff,
   * recent commits, the center pane's current file, pasted notes) select
   * what the next turn gathers. While a turn streams, the bubble mirrors
   * the same `AiJob` the status bar renders — one source of truth.
   *
   * Activity: every AI run (features + chat) from `ai.jobs` — usage
   * aggregates (today / 7 days, per feature) above a capped, expandable
   * history. The status bar's AI rows deep-link here via the
   * `ai-open-activity` bus event (RepoView flips the bound `tab`).
   *
   * Gates, in order: master switch off → "AI is turned off" empty state
   * with the settings path back in; repo not opted in → enable prompt.
   * The palette command (`panel.focus-assistant`) focuses the panel like
   * any other.
   */
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import EmptyState from "$lib/components/layout/EmptyState.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { toast } from "$lib/toast";
  import { describeError } from "$lib/errors";
  import { formatRelativeTime } from "$lib/git/format";
  import { ai } from "$lib/ai/ai.svelte";
  import {
    aiElapsedLabel,
    aiFeatureLabel,
    aiPhaseLabel,
  } from "$lib/ai/registry";
  import type { AiFeatureTag } from "$lib/ai/registry";
  import { aiResultSeconds, aiUsageLabel } from "$lib/ai/usage";
  import { isAiError } from "$lib/ai/types";
  import type { AiJobRecord } from "$lib/ai/jobs.svelte";
  import type { ChatMessage } from "$lib/ai/chat.svelte";
  import { emitUiEvent } from "$lib/palette/events";
  import AiMarkdown from "./AiMarkdown.svelte";

  let {
    repoId,
    root,
    repoName,
    tab = $bindable("chat"),
    /** Center pane's current diff path (context chip; `null` hides it). */
    diffPath = null,
  }: {
    repoId: string;
    root: string;
    repoName: string;
    tab?: "chat" | "activity";
    diffPath?: string | null;
  } = $props();

  const iconSend = $derived(appIcon("arrow-upward")!);
  const iconClear = $derived(appIcon("delete-sweep")!);
  const iconInfo = $derived(appIcon("info")!);
  const iconRetry = $derived(appIcon("restart-alt")!);

  // --- gates -------------------------------------------------------------------

  const enabled = $derived(ai.isEnabled());
  const optedIn = $derived(ai.isOptedIn(repoId));

  // --- chat state ----------------------------------------------------------------

  const conversation = $derived(ai.chat.conversationFor(root));
  const busy = $derived(ai.chat.busy[root] === true);
  /** The streaming turn's job (same one the status bar shows). */
  const job = $derived(ai.jobs.activeJob(repoId, "chat"));

  /** Composer draft. */
  let draft = $state("");
  /** Context chip selection (defaults: staged diff + recent commits). */
  let ctxStaged = $state(true);
  let ctxCommits = $state(true);
  let ctxFile = $state(false);
  let ctxPasted = $state("");
  /** Paste box visible? */
  let pasteOpen = $state(false);

  /** 1s ticker while a turn streams (elapsed label), idle-cost zero. */
  let now = $state(Date.now());
  $effect(() => {
    if (!busy) return;
    now = Date.now();
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });

  let listEl = $state<HTMLElement | null>(null);
  /** True while the user keeps the list pinned to the bottom. */
  let pinned = true;

  function onListScroll(): void {
    const el = listEl;
    if (el === null) return;
    pinned = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  }

  // Follow new messages and stream deltas, but only while pinned (the
  // user reading an earlier answer must not be yanked back down).
  $effect(() => {
    void conversation.length;
    void job?.streamedText?.length;
    const el = listEl;
    if (el === null || !pinned) return;
    el.scrollTop = el.scrollHeight;
  });

  function currentContext() {
    return {
      staged: ctxStaged,
      commits: ctxCommits,
      path: ctxFile ? diffPath : null,
      pasted: ctxPasted.trim() ? ctxPasted : null,
    };
  }

  function send(question: string): void {
    const text = question.trim();
    if (text.length === 0 || busy) return;
    draft = "";
    pinned = true;
    void ai
      .sendChat({ repoId, root, question: text, context: currentContext() })
      .catch((err: unknown) => {
        // Cancelled turns already landed in the thread with their partial
        // text — no extra toast; real failures did too, plus one toast.
        if (isAiError(err) && err.kind === "cancelled") return;
        toast(`Assistant failed: ${describeError(err)}`, { kind: "error" });
      });
  }

  /** Re-sends the question that preceded a failed assistant turn. */
  function retryAfter(message: ChatMessage): void {
    const index = conversation.indexOf(message);
    if (index <= 0) return;
    const prior = conversation[index - 1];
    if (prior.role !== "user") return;
    send(prior.text);
  }

  function clearConversation(): void {
    ai.chat.clear(root);
  }

  function onComposerKeydown(event: KeyboardEvent): void {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      send(draft);
    }
  }

  /** A message bubble needs a footer (serving meta / failure note). */
  function metaLabel(message: ChatMessage): string | null {
    if (message.error !== undefined && message.error !== null) return null;
    const parts: string[] = [];
    if (message.model) parts.push(`via ${message.model}`);
    if (message.backend) parts.push(message.backend);
    if (typeof message.elapsedMs === "number") parts.push(aiResultSeconds(message.elapsedMs));
    const usage = aiUsageLabel(message.usage ?? null);
    if (usage !== null) parts.push(usage);
    return parts.length > 0 ? parts.join(" · ") : null;
  }

  // --- activity state ---------------------------------------------------------------

  const history = $derived(ai.jobs.history);
  const DAY_MS = 86_400_000;
  const todaySummary = $derived(ai.jobs.usageSince(startOfDay(Date.now())));
  const weekSummary = $derived(ai.jobs.usageSince(Date.now() - 7 * DAY_MS));

  function startOfDay(ms: number): number {
    const d = new Date(ms);
    d.setHours(0, 0, 0, 0);
    return d.getTime();
  }

  let expanded = $state<Record<string, boolean>>({});

  function toggleRecord(id: string): void {
    expanded[id] = !expanded[id];
  }

  function clearActivity(): void {
    ai.jobs.clearHistory();
    toast("AI activity history cleared", { kind: "success" });
  }

  const statusLabel: Record<AiJobRecord["status"], string> = {
    done: "done",
    cancelled: "cancelled",
    error: "failed",
  };
</script>

<section
  class="assistant relative flex min-h-0 flex-1 flex-col overflow-hidden bg-background text-xs text-foreground"
  aria-label="AI assistant"
>
  {#if !enabled}
    <!-- Master switch off: the path back in stays reachable. -->
    <EmptyState
      title="AI is turned off."
      hint="Enable it again in AI settings to use the assistant and the AI features."
    >
      <Button
        class="h-auto whitespace-nowrap rounded-sm px-2 py-px text-ui-tiny font-normal"
        variant="secondary"
        type="button"
        onclick={() => emitUiEvent("open-ai-settings")}
      >
        Open AI settings
      </Button>
    </EmptyState>
  {:else}
    <!-- Tab strip: Chat / Activity (segmented, panel-local). -->
    <div
      class="tabs flex flex-none items-center gap-1 border-b border-border bg-popover px-2 py-1"
      role="tablist"
      aria-label="Assistant tabs"
    >
      <button
        class="rounded-sm px-2 py-px text-ui-meta font-medium {tab === 'chat'
          ? 'bg-accent text-accent-foreground'
          : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        type="button"
        role="tab"
        aria-selected={tab === "chat"}
        onclick={() => (tab = "chat")}
      >
        Chat
      </button>
      <button
        class="rounded-sm px-2 py-px text-ui-meta font-medium {tab === 'activity'
          ? 'bg-accent text-accent-foreground'
          : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        type="button"
        role="tab"
        aria-selected={tab === "activity"}
        onclick={() => (tab = "activity")}
      >
        Activity
      </button>
      <span class="spacer flex-1"></span>
      {#if tab === "chat"}
        <Hint label="Clear this conversation">
          {#snippet child({ props })}
            <Button
              {...props}
              class="h-auto whitespace-nowrap rounded-sm px-1.5 py-px font-normal [&_svg:not([class*='size-'])]:size-3"
              variant="secondary"
              type="button"
              aria-label="Clear this conversation"
              disabled={busy || conversation.length === 0}
              onclick={clearConversation}
            >
              <Icon icon={iconClear} size={12} />
            </Button>
          {/snippet}
        </Hint>
      {:else}
        <Hint label="Clear the activity history">
          {#snippet child({ props })}
            <Button
              {...props}
              class="h-auto whitespace-nowrap rounded-sm px-1.5 py-px text-ui-tiny font-normal"
              variant="secondary"
              type="button"
              aria-label="Clear the activity history"
              disabled={history.length === 0}
              onclick={clearActivity}
            >
              Clear
            </Button>
          {/snippet}
        </Hint>
      {/if}
    </div>

    {#if tab === "chat"}
      {#if !optedIn}
        <!-- Per-repo opt-in: the panel's first-use consent, one click. -->
        <EmptyState
          title="AI is not enabled for {repoName}."
          hint="The assistant reads repo data you explicitly select (diffs, history, pasted notes) to answer questions. It never modifies anything."
        >
          <Button
            class="h-auto whitespace-nowrap rounded-sm px-2 py-px text-ui-tiny font-normal"
            type="button"
            onclick={() => ai.setOptIn(repoId, true)}
          >
            Enable AI for this repository
          </Button>
        </EmptyState>
      {:else}
        <!-- Advisory contract, always visible above the transcript. -->
        <p
          class="banner flex flex-none items-start gap-1.5 border-b border-border bg-muted/50 px-2.5 py-1 text-ui-tiny text-muted-foreground"
          role="note"
        >
          <span class="mt-[2px] flex-none text-muted-foreground" aria-hidden="true">
            <Icon icon={iconInfo} size={12} />
          </span>
          <span>
            The assistant answers questions about this repository. It never
            modifies anything — suggestions are applied by you.
          </span>
        </p>

        <!-- Transcript -->
        <div
          bind:this={listEl}
          onscroll={onListScroll}
          class="list min-h-0 flex-1 overflow-y-auto px-2.5 py-2"
          role="log"
          aria-label="Assistant conversation"
          aria-live="polite"
        >
          {#if conversation.length === 0 && !busy}
            <div class="intro flex flex-col gap-2 pt-2">
              <p class="m-0 text-ui-meta text-muted-foreground">
                Ask anything about <span class="font-medium text-foreground">{repoName}</span> — the
                enabled context chips below tell the assistant what to read.
              </p>
              <div class="quick flex flex-wrap gap-1.5" aria-label="Quick prompts">
                <Button
                  class="h-auto whitespace-nowrap rounded-sm px-2 py-px text-ui-tiny font-normal"
                  variant="secondary"
                  type="button"
                  onclick={() => send("Explain what the current working copy changes do.")}
                >
                  Explain working copy
                </Button>
                <Button
                  class="h-auto whitespace-nowrap rounded-sm px-2 py-px text-ui-tiny font-normal"
                  variant="secondary"
                  type="button"
                  onclick={() => send("Summarize the recent commit history and what the branch is working on.")}
                >
                  Summarize recent history
                </Button>
              </div>
            </div>
          {/if}
          {#each conversation as message (message.id)}
            <div
              class="msg mb-2.5 flex flex-col {message.role === 'user'
                ? 'items-end'
                : 'items-start'}"
            >
              <div
                class="bubble max-w-full rounded-lg px-2.5 py-1.5 {message.role === 'user'
                  ? 'rounded-br-sm bg-primary/10'
                  : 'rounded-bl-sm border border-border bg-card'} {message.error
                  ? 'border-destructive/40'
                  : ''}"
              >
                {#if message.role === "user"}
                  <p class="m-0 whitespace-pre-wrap leading-[1.45] [overflow-wrap:anywhere]">{message.text}</p>
                {:else if message.error && message.text.length === 0}
                  <p class="m-0 text-destructive" role="alert">{message.error}</p>
                {:else}
                  <AiMarkdown text={message.text} />
                {/if}
              </div>
              <!-- Footer: serving meta, failure note, retry. -->
              {#if metaLabel(message) !== null}
                <span class="mt-px text-ui-micro text-muted-foreground">{metaLabel(message)}</span>
              {:else if message.cancelled}
                <span class="mt-px text-ui-micro text-muted-foreground">cancelled</span>
              {:else if message.error}
                <span class="mt-px flex items-center gap-1.5 text-ui-micro text-destructive">
                  {message.error}
                  <button
                    class="inline-flex items-center gap-0.5 rounded-sm px-1 py-px text-ui-micro text-foreground hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                    type="button"
                    onclick={() => retryAfter(message)}
                  >
                    <Icon icon={iconRetry} size={10} />
                    Retry
                  </button>
                </span>
              {/if}
            </div>
          {/each}
          {#if busy && job !== null}
            <!-- The live turn: mirrors the status bar's job (one source). -->
            <div class="msg streaming mb-2.5 flex flex-col items-start">
              <div class="bubble max-w-full rounded-lg rounded-bl-sm border border-border bg-card px-2.5 py-1.5">
                {#if job.streamedText !== null && job.streamedText.length > 0}
                  <AiMarkdown text={job.streamedText} streaming />
                {:else}
                  <p class="m-0 flex items-center gap-1.5 text-muted-foreground">
                    <span class="inline-block size-1.5 animate-pulse rounded-full bg-primary" aria-hidden="true"></span>
                    {job.message ?? aiPhaseLabel(job.phase) ?? "working"}…
                  </p>
                {/if}
              </div>
              <span class="mt-px text-ui-micro text-muted-foreground">
                {aiElapsedLabel(now - job.startedAt)} ·
                <button
                  class="rounded-sm px-1 py-px text-ui-micro text-foreground underline hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                  type="button"
                  onclick={() => ai.chat.stop(root)}
                >
                  Cancel
                </button>
              </span>
            </div>
          {/if}
        </div>

        <!-- Composer + context chips -->
        <div class="composer flex flex-none flex-col gap-1.5 border-t border-border bg-popover px-2.5 py-2">
          <div class="chips flex flex-wrap items-center gap-1" role="group" aria-label="Context for the next question">
            <button
              class="chip rounded-full border px-2 py-px text-ui-tiny transition-colors {ctxStaged
                ? 'border-primary/60 bg-primary/10 text-foreground'
                : 'border-border text-muted-foreground hover:bg-accent'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
              type="button"
              aria-pressed={ctxStaged}
              onclick={() => (ctxStaged = !ctxStaged)}
            >
              Staged diff
            </button>
            <button
              class="chip rounded-full border px-2 py-px text-ui-tiny transition-colors {ctxCommits
                ? 'border-primary/60 bg-primary/10 text-foreground'
                : 'border-border text-muted-foreground hover:bg-accent'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
              type="button"
              aria-pressed={ctxCommits}
              onclick={() => (ctxCommits = !ctxCommits)}
            >
              Recent commits
            </button>
            {#if diffPath !== null}
              <Hint label={`Attach the working-copy diff of ${diffPath}`}>
                {#snippet child({ props })}
                  <button
                    {...props}
                    class="chip max-w-[14rem] truncate rounded-full border px-2 py-px text-ui-tiny transition-colors {ctxFile
                      ? 'border-primary/60 bg-primary/10 text-foreground'
                      : 'border-border text-muted-foreground hover:bg-accent'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                    type="button"
                    aria-pressed={ctxFile}
                    onclick={() => (ctxFile = !ctxFile)}
                  >
                    {diffPath.split("/").at(-1)}
                  </button>
                {/snippet}
              </Hint>
            {/if}
            <button
              class="chip rounded-full border px-2 py-px text-ui-tiny transition-colors {ctxPasted.trim()
                ? 'border-primary/60 bg-primary/10 text-foreground'
                : 'border-border text-muted-foreground hover:bg-accent'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
              type="button"
              aria-pressed={ctxPasted.trim().length > 0 || pasteOpen}
              onclick={() => (pasteOpen = !pasteOpen)}
            >
              Notes…
            </button>
          </div>
          {#if pasteOpen}
            <textarea
              class="paste h-16 w-full resize-y rounded-sm border border-border bg-background px-[7px] py-1 text-ui-meta text-foreground placeholder:text-muted-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
              placeholder="Paste logs, error messages or notes for the assistant to read…"
              aria-label="Pasted notes"
              bind:value={ctxPasted}
            ></textarea>
          {/if}
          <div class="inputrow flex items-end gap-1.5">
            <textarea
              class="input max-h-32 min-h-[2.1rem] w-full flex-1 resize-y rounded-md border border-border bg-background px-[9px] py-1.5 text-ui-base leading-[1.4] text-foreground placeholder:text-muted-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
              placeholder={busy ? "Waiting for the assistant…" : `Ask about ${repoName}… (Enter to send, Shift+Enter for a new line)`}
              aria-label="Ask the assistant"
              rows="1"
              bind:value={draft}
              onkeydown={onComposerKeydown}
            ></textarea>
            {#if busy}
              <Button
                class="h-auto whitespace-nowrap rounded-sm px-2 py-1.5 text-ui-tiny font-normal"
                variant="secondary"
                type="button"
                onclick={() => ai.chat.stop(root)}
              >
                Cancel
              </Button>
            {:else}
              <Hint label="Send (Enter)">
                {#snippet child({ props })}
                  <Button
                    {...props}
                    class="h-auto whitespace-nowrap rounded-sm px-2 py-1.5 font-normal [&_svg:not([class*='size-'])]:size-3.5"
                    type="button"
                    aria-label="Send question"
                    disabled={draft.trim().length === 0}
                    onclick={() => send(draft)}
                  >
                    <Icon icon={iconSend} size={14} />
                  </Button>
                {/snippet}
              </Hint>
            {/if}
          </div>
        </div>
      {/if}
    {:else}
      <!-- Activity: app-wide job history + usage aggregates. -->
      <div class="activity min-h-0 flex-1 overflow-y-auto px-2.5 py-2">
        {#if history.length === 0}
          <EmptyState
            title="No AI activity yet."
            hint="Feature runs and assistant questions appear here with their model, duration and token usage."
          />
        {:else}
          <div class="cards mb-2 flex flex-none flex-wrap gap-1.5">
            <div class="card flex min-w-[6rem] flex-col gap-[1px] rounded-md border border-border bg-card px-2.5 py-[5px]">
              <span class="card-value text-sm font-semibold tabular-nums">{todaySummary.runs}</span>
              <span class="card-label text-ui-tiny uppercase tracking-[0.03em] text-muted-foreground">runs today</span>
            </div>
            <div class="card flex min-w-[6rem] flex-col gap-[1px] rounded-md border border-border bg-card px-2.5 py-[5px]">
              <span class="card-value text-sm font-semibold tabular-nums">{todaySummary.totalTokens}</span>
              <span class="card-label text-ui-tiny uppercase tracking-[0.03em] text-muted-foreground">tokens today</span>
            </div>
            <div class="card flex min-w-[6rem] flex-col gap-[1px] rounded-md border border-border bg-card px-2.5 py-[5px]">
              <span class="card-value text-sm font-semibold tabular-nums">{weekSummary.runs}</span>
              <span class="card-label text-ui-tiny uppercase tracking-[0.03em] text-muted-foreground">runs 7d</span>
            </div>
            <div class="card flex min-w-[6rem] flex-col gap-[1px] rounded-md border border-border bg-card px-2.5 py-[5px]">
              <span class="card-value text-sm font-semibold tabular-nums">{weekSummary.totalTokens}</span>
              <span class="card-label text-ui-tiny uppercase tracking-[0.03em] text-muted-foreground">tokens 7d</span>
            </div>
          </div>

          {#if Object.keys(weekSummary.byFeature).length > 0}
            <ul class="byfeature mb-2 m-0 flex list-none flex-wrap gap-x-3 gap-y-px p-0 text-ui-tiny text-muted-foreground" aria-label="Usage per feature (7 days)">
              {#each Object.entries(weekSummary.byFeature) as [kind, bucket] (kind)}
                <li>
                  <span class="text-foreground">{aiFeatureLabel(kind as AiFeatureTag)}</span>
                  · {bucket.runs} run{bucket.runs === 1 ? "" : "s"} · {bucket.totalTokens} tokens
                </li>
              {/each}
            </ul>
          {/if}

          <ul class="history m-0 flex list-none flex-col gap-1 p-0" aria-label="AI run history">
            {#each history as record (record.id)}
              <li class="rec rounded-md border border-border bg-card">
                <button
                  class="row flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1 text-left hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
                  type="button"
                  aria-expanded={expanded[record.id] === true}
                  onclick={() => toggleRecord(record.id)}
                >
                  <span
                    class="dot size-1.5 flex-none rounded-full {record.status === 'done'
                      ? 'bg-muted-foreground/50'
                      : record.status === 'error'
                        ? 'bg-destructive'
                        : 'bg-yellow-500'}"
                    aria-hidden="true"
                  ></span>
                  <span class="feature min-w-0 flex-none font-medium {record.status !== 'done' ? 'text-muted-foreground' : ''}">
                    {aiFeatureLabel(record.feature)}
                  </span>
                  <span class="repo min-w-0 flex-none text-muted-foreground">{record.repoName ?? record.repoId}</span>
                  <span class="spacer min-w-0 flex-1 truncate text-ui-tiny text-muted-foreground">
                    {#if record.model}{record.model}{/if}
                  </span>
                  <span class="tokens flex-none tabular-nums text-ui-tiny text-muted-foreground">
                    {aiUsageLabel(record.usage) ?? ""}
                  </span>
                  <span class="elapsed flex-none tabular-nums text-ui-tiny text-muted-foreground">
                    {aiResultSeconds(record.elapsedMs)}
                  </span>
                  <span class="age flex-none text-ui-tiny text-muted-foreground" title={new Date(record.startedAt).toLocaleString()}>
                    {formatRelativeTime(Math.floor(record.startedAt / 1000))}
                  </span>
                  <span class="status w-14 flex-none text-right text-ui-tiny {record.status === 'error' ? 'text-destructive' : 'text-muted-foreground'}">
                    {statusLabel[record.status]}
                  </span>
                </button>
                {#if expanded[record.id] === true}
                  <div class="detail flex flex-col gap-1.5 border-t border-border px-2.5 py-1.5">
                    {#if record.promptPreview}
                      <div>
                        <h4 class="m-0 mb-px text-ui-tiny font-medium text-muted-foreground">Prompt</h4>
                        <pre class="m-0 max-h-40 overflow-y-auto whitespace-pre-wrap rounded bg-muted/60 p-1.5 font-mono text-ui-micro leading-[1.4] [overflow-wrap:anywhere]">{record.promptPreview}</pre>
                      </div>
                    {/if}
                    {#if record.error}
                      <p class="m-0 text-ui-tiny text-destructive" role="alert">{record.error}</p>
                    {/if}
                    {#if record.text}
                      <div>
                        <h4 class="m-0 mb-px text-ui-tiny font-medium text-muted-foreground">Response</h4>
                        <div class="rounded bg-muted/60 p-1.5">
                          <AiMarkdown text={record.text} />
                        </div>
                      </div>
                    {:else if !record.error && record.status === "cancelled"}
                      <p class="m-0 text-ui-tiny text-muted-foreground">Cancelled before a response arrived.</p>
                    {/if}
                  </div>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  {/if}
</section>
