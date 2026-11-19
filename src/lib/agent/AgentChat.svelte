<script lang="ts">
  /**
   * Agent-mode chat — a stripped-down AssistantPanel: single-purpose git
   * Q&A with fixed context (staged diff + recent commits, no chips, no
   * paste box). All gathering/streaming/persistence stays in `ai.chat`;
   * this component is presentation + the same gates AssistantPanel has
   * (master switch → AI settings, per-repo opt-in → one-click enable).
   */
  import { Button } from "$lib/components/ui/button/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import EmptyState from "$lib/components/layout/EmptyState.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { toast } from "$lib/toast";
  import { describeError } from "$lib/errors";
  import { ai } from "$lib/ai/ai.svelte";
  import { aiElapsedLabel, aiPhaseLabel } from "$lib/ai/registry";
  import { aiResultSeconds, aiUsageLabel } from "$lib/ai/usage";
  import { isAiError } from "$lib/ai/types";
  import type { ChatContextSelection, ChatMessage } from "$lib/ai/chat.svelte";
  import { emitUiEvent } from "$lib/palette/events";
  import AiMarkdown from "$lib/components/ai/AiMarkdown.svelte";
  import AgenticRunTranscript from "$lib/components/ai/AgenticRunTranscript.svelte";

  let { repoId, root, repoName }: { repoId: string; root: string; repoName: string } = $props();

  const iconSend = $derived(appIcon("arrow-upward")!);
  const iconClear = $derived(appIcon("delete-sweep")!);
  const iconRetry = $derived(appIcon("restart-alt")!);

  // --- gates (same order as AssistantPanel) -----------------------------------

  const enabled = $derived(ai.isEnabled());
  const optedIn = $derived(ai.isOptedIn(repoId));

  // --- chat state -------------------------------------------------------------

  const conversation = $derived(ai.chat.conversationFor(root));
  const busy = $derived(ai.chat.busy[root] === true);
  /** The streaming turn's job (same one the status bar shows). */
  const job = $derived(ai.jobs.activeJob(repoId, "chat"));

  /** Composer draft. */
  let draft = $state("");
  /**
   * Agentic chip (per-run override, M13 chunk 3): default derives from the
   * per-feature chat switch; toggling affects the next message only.
   */
  let agentic = $state(ai.isFeatureEnabled("chat"));

  /** Context is fixed: the agent chat always reads staged diff + commits. */
  const CHAT_CONTEXT: ChatContextSelection = {
    staged: true,
    commits: true,
    path: null,
    pasted: null,
  };

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

  function send(question: string): void {
    const text = question.trim();
    if (text.length === 0 || busy) return;
    draft = "";
    pinned = true;
    void ai
      .sendChat({ repoId, root, question: text, context: CHAT_CONTEXT, agentic })
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

  /** One muted footer line: model · duration · usage. */
  function metaLabel(message: ChatMessage): string | null {
    if (message.error !== undefined && message.error !== null) return null;
    const parts: string[] = [];
    if (message.mode) parts.push(message.mode);
    if (message.model) parts.push(`via ${message.model}`);
    if (message.backend) parts.push(message.backend);
    if (typeof message.elapsedMs === "number") parts.push(aiResultSeconds(message.elapsedMs));
    const usage = aiUsageLabel(message.usage ?? null);
    if (usage !== null) parts.push(usage);
    return parts.length > 0 ? parts.join(" · ") : null;
  }
</script>

<section
  class="relative flex min-h-0 flex-1 flex-col overflow-hidden bg-background text-xs text-foreground"
  aria-label="Agent chat"
>
  {#if !enabled}
    <!-- Master switch off: the path back in stays reachable. -->
    <EmptyState
      title="AI assist is off."
      hint="Enable it again in AI settings to use the agent chat."
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
  {:else if !optedIn}
    <!-- Per-repo opt-in: the first-use consent, one click. -->
    <EmptyState
      title="AI is not enabled for {repoName}."
      hint="The agent reads the staged diff and recent commits to answer questions. It never modifies anything."
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
    <header class="flex flex-none items-center gap-2 border-b border-border bg-popover px-2.5 py-1">
      <span class="min-w-0 truncate font-medium">{repoName}</span>
      <span class="ml-auto flex-none text-ui-micro text-muted-foreground">advisory — never mutates</span>
      <Hint label="Clear this conversation">
        {#snippet child({ props })}
          <Button
            {...props}
            class="h-auto whitespace-nowrap rounded-sm px-1.5 py-px font-normal [&_svg:not([class*='size-'])]:size-3"
            variant="ghost"
            type="button"
            aria-label="Clear this conversation"
            disabled={busy || conversation.length === 0}
            onclick={clearConversation}
          >
            <Icon icon={iconClear} size={12} />
          </Button>
        {/snippet}
      </Hint>
    </header>

    <!-- Transcript -->
    <div
      bind:this={listEl}
      onscroll={onListScroll}
      class="min-h-0 flex-1 overflow-y-auto px-2.5 py-2"
      role="log"
      aria-label="Agent conversation"
      aria-live="polite"
    >
      {#if conversation.length === 0 && !busy}
        <p class="m-0 pt-2 text-ui-meta text-muted-foreground">
          Ask anything about <span class="font-medium text-foreground">{repoName}</span> — the staged
          diff and recent commits are read automatically.
        </p>
      {/if}
      {#each conversation as message (message.id)}
        <div class="mb-2 flex flex-col {message.role === 'user' ? 'items-end' : 'items-start'}">
          <div
            class="max-w-full rounded-lg px-2.5 py-1.5 {message.role === 'user'
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
          <!-- Compact footer: serving meta / cancelled / failure + retry. -->
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
        <div class="mb-2 flex flex-col items-start">
          <div class="max-w-full rounded-lg rounded-bl-sm border border-border bg-card px-2.5 py-1.5">
            {#if job.streamedText !== null && job.streamedText.length > 0}
              <AiMarkdown text={job.streamedText} streaming />
            {:else}
              <p class="m-0 flex items-center gap-1.5 text-muted-foreground">
                <span class="inline-block size-1.5 animate-pulse rounded-full bg-primary" aria-hidden="true"></span>
                {job.message ?? aiPhaseLabel(job.phase) ?? "working"}…
              </p>
            {/if}
            {#if job.agentic !== null}
              <!-- Agentic run: live tool transcript + approval cards. -->
              <AgenticRunTranscript {job} {now} />
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

    <!-- Composer -->
    <div class="flex flex-none items-end gap-1.5 border-t border-border bg-popover px-2.5 py-2">
      <button
        class="flex-none rounded-full border px-2 py-px text-ui-tiny transition-colors {agentic
          ? 'border-primary/60 bg-primary/10 text-foreground'
          : 'border-border text-muted-foreground hover:bg-accent'} focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        type="button"
        aria-pressed={agentic}
        aria-label="Let the assistant use repo tools for this message"
        onclick={() => (agentic = !agentic)}
      >
        Tools
      </button>
      <textarea
        class="max-h-32 w-full flex-1 resize-y rounded-md border border-border bg-background px-[9px] py-1.5 text-ui-base leading-[1.4] text-foreground placeholder:text-muted-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
        placeholder={busy ? "Waiting for the assistant…" : `Ask about ${repoName}… (Enter to send, Shift+Enter for a new line)`}
        aria-label="Ask the agent"
        rows="2"
        bind:value={draft}
        onkeydown={onComposerKeydown}
      ></textarea>
      <Hint label="Send (Enter)">
        {#snippet child({ props })}
          <Button
            {...props}
            class="h-auto shrink-0 whitespace-nowrap rounded-sm px-2 py-1.5 font-normal [&_svg:not([class*='size-'])]:size-3.5"
            variant="ghost"
            type="button"
            aria-label="Send question"
            disabled={busy || draft.trim().length === 0}
            onclick={() => send(draft)}
          >
            <Icon icon={iconSend} size={14} />
          </Button>
        {/snippet}
      </Hint>
    </div>
  {/if}
</section>
