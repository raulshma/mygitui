<script lang="ts">
  /**
   * One pending tool-approval ask (agentic runs, M13 chunk 2): renders the
   * ask's action + resources readably, the decision buttons, and the 120 s
   * auto-reject countdown from {@link AGENTIC_ASK_TIMEOUT_MS}.
   *
   * Buttons by action (see `offersAlways`):
   *   - shell / external_directory / subagent / skill / execute / MCP
   *     asks: Allow once · Always in this repo · Deny;
   *   - read / edit asks: Allow once · Deny ONLY, with a one-line hint —
   *     server-side those asks persist a project-wide allow rule, so an
   *     "always" here would auto-allow every future sensitive ask in the
   *     project (`safeDecision` enforces this defensively too);
   *   - `question` asks: a one-line answer + Skip (responds "once" with
   *     the answer; empty answer proceeds with a null message).
   *
   * A decision resolves through the bound controller's `respond`; a stale
   * id (resolved elsewhere, e.g. by the timeout racing the click) lands
   * in a quiet "Already resolved." state — no error, no toast. The parent
   * drops the card once the ask leaves `controller.pending`.
   */
  import { Button } from "$lib/components/ui/button/index.js";
  import {
    AGENTIC_ASK_TIMEOUT_MS,
    offersAlways,
    safeDecision,
  } from "$lib/ai/agenticRun.svelte";
  import type {
    AgenticApproval,
    AgenticDecision,
    AgenticRunController,
  } from "$lib/ai/agenticRun.svelte";
  import { aiResultSeconds } from "$lib/ai/usage";

  let {
    approval,
    controller,
    now,
  }: {
    approval: AgenticApproval;
    controller: AgenticRunController;
    /** Parent ticker (epoch ms) — drives the auto-reject countdown. */
    now: number;
  } = $props();

  /** Friendly label per ask action (the raw action is the fallback). */
  const ACTION_LABELS: Record<string, string> = {
    shell: "Run command",
    read: "Read file",
    edit: "Edit file",
    external_directory: "Access a directory outside the repo",
    subagent: "Spawn a subagent",
    skill: "Run a skill",
    execute: "Execute",
    question: "Question",
  };

  const title = $derived(approval.message ?? ACTION_LABELS[approval.action] ?? approval.action);
  const remainingMs = $derived(Math.max(0, approval.askedAt + AGENTIC_ASK_TIMEOUT_MS - now));
  const expired = $derived(remainingMs <= 0);
  const always = $derived(offersAlways(approval.action));
  const isQuestion = $derived(approval.action === "question");
  const isFileAsk = $derived(approval.action === "read" || approval.action === "edit");

  /** Long resources render truncated with an expand toggle. */
  const LONG_RESOURCE = 96;
  let expanded = $state(false);
  const long = $derived(approval.resources.some((resource) => resource.length > LONG_RESOURCE));

  /** True while a decision's reply is on its way (buttons frozen). */
  let answering = $state(false);
  /** The last reply POST failed — the ask is still open, offer a retry. */
  let replyFailed = $state(false);
  /** Question variant's draft answer. */
  let answer = $state("");

  async function decide(decision: AgenticDecision): Promise<void> {
    if (answering || expired) return;
    answering = true;
    replyFailed = false;
    const message =
      isQuestion && answer.trim().length > 0 ? answer.trim() : null;
    const outcome = await controller
      .respond(approval.requestID, safeDecision(approval.action, decision), message)
      .catch(() => "reply-failed" as const);
    if (outcome === "handled") return; // the ask left `pending` — the parent drops the card
    // "reply-failed": the ask is STILL pending (the controller re-armed
    // its timer) — unfreeze the buttons behind a retry affordance.
    // ("stale" cannot render: a resolved ask's card is unmounted.)
    answering = false;
    if (outcome === "reply-failed") replyFailed = true;
  }
</script>

<section
  class="agentic-ask flex flex-col gap-1 rounded-md border border-border bg-muted/40 px-2 py-1.5"
  role="group"
  aria-label="Approval needed: {title}"
>
  <p class="m-0 flex items-baseline gap-2 text-ui-tiny font-medium text-foreground">
    <span class="min-w-0 flex-1 [overflow-wrap:anywhere]">{title}</span>
    <span
      class="flex-none text-ui-micro font-normal tabular-nums text-muted-foreground"
      title="Unanswered asks are auto-rejected after 120 seconds"
    >
      {#if expired}timed out{:else}auto-rejects in {aiResultSeconds(remainingMs)}{/if}
    </span>
  </p>
  {#if approval.resources.length > 0}
    <div class="resources flex flex-col gap-px">
      {#each approval.resources as resource (resource)}
        {#if !long}
          <code class="break-all font-mono text-ui-micro text-muted-foreground">{resource}</code>
        {:else if expanded}
          <button
            class="break-all text-left font-mono text-ui-micro text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            aria-expanded="true"
            onclick={() => (expanded = false)}
          >
            {resource}
          </button>
        {:else}
          <button
            class="truncate text-left font-mono text-ui-micro text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
            type="button"
            aria-expanded="false"
            title={resource}
            onclick={() => (expanded = true)}
          >
            {resource}
          </button>
        {/if}
      {/each}
    </div>
  {/if}
  {#if replyFailed}
    <p class="m-0 text-ui-micro text-destructive" role="alert">
      Reply failed — the ask is still open; try again.
    </p>
  {/if}
  {#if isQuestion}
    <form
      class="flex items-center gap-1"
      onsubmit={(event) => {
        event.preventDefault();
        void decide("once");
      }}
    >
      <input
        class="min-w-0 flex-1 rounded-md border border-border bg-background px-2 py-1 text-ui-tiny text-foreground placeholder:text-muted-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring disabled:opacity-50"
        placeholder="Answer (optional)…"
        aria-label="Answer the agent's question"
        bind:value={answer}
        disabled={answering || expired}
      />
      <Button
        class="h-auto shrink-0 whitespace-nowrap rounded-sm px-1.5 py-px font-normal"
        variant="secondary"
        type="submit"
        disabled={answering || expired}
      >
        Skip
      </Button>
    </form>
  {:else}
    {#if isFileAsk}
      <p class="m-0 text-ui-micro text-muted-foreground">
        Always-allow isn't offered for file reads/edits — each one needs your approval.
      </p>
    {/if}
    <div class="flex flex-wrap items-center gap-1.5">
      <Button
        class="h-auto whitespace-nowrap rounded-sm px-1.5 py-px font-normal"
        variant="secondary"
        type="button"
        disabled={answering || expired}
        onclick={() => decide("once")}
      >
        Allow once
      </Button>
      {#if always}
        <Button
          class="h-auto whitespace-nowrap rounded-sm px-1.5 py-px font-normal"
          variant="secondary"
          type="button"
          disabled={answering || expired}
          onclick={() => decide("always")}
        >
          Always in this repo
        </Button>
      {/if}
      <Button
        class="h-auto whitespace-nowrap rounded-sm px-1.5 py-px font-normal"
        variant="ghost"
        type="button"
        disabled={answering || expired}
        onclick={() => decide("reject")}
      >
        Deny
      </Button>
    </div>
  {/if}
</section>
