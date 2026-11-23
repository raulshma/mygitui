<script lang="ts">
  /**
   * Live agentic-run transcript (M13 chunk 2): the tool-call rolling
   * window, the run's pending approval cards (oldest first, one decision
   * per card — deliberately no approve-all), the budget indicator and an
   * optional run-cancel. Bound to the job's agentic runtime — reads the
   * controller's reactive `steps`/`pending`/budget state directly, so
   * rows and cards appear as the run's events land. Renders nothing for
   * a job without an agentic runtime (plain feature/chat runs).
   *
   * Embedded inside the live-turn bubbles of AssistantPanel and AgentChat;
   * chunk 3/4 only need to start the run (via `ai.runAgentic`) for this
   * to light up.
   */
  import AgenticApprovalCard from "./AgenticApprovalCard.svelte";
  import { agenticStepViews } from "$lib/ai/agenticRun.svelte";
  import type { AiJob } from "$lib/ai/jobs.svelte";
  import { aiElapsedLabel } from "$lib/ai/registry";

  let {
    job,
    now,
    onCancel,
  }: {
    job: AiJob;
    /** Parent ticker (epoch ms) — elapsed, countdowns. */
    now: number;
    /** Run cancel (parent-owned; the panel/chat surface has its own). */
    onCancel?: () => void;
  } = $props();

  const controller = $derived(job.agentic?.controller ?? null);
  const views = $derived(controller === null ? [] : agenticStepViews(controller.steps));
  const budget = $derived(controller?.budgetView(now) ?? null);
  const pending = $derived(controller?.pending ?? []);
  const budgetLabel = $derived.by(() => {
    if (budget === null) return null;
    if (budget.budgetStopped && budget.budgetReason === "tool-calls") return "tool-call cap reached";
    if (budget.budgetStopped && budget.budgetReason === "wall-clock") return "time cap reached";
    return null;
  });
</script>

{#if controller !== null}
  <div
    class="agentic flex flex-col gap-1.5 border-t border-border pt-1.5"
    role="log"
    aria-label="Agent tool transcript"
  >
    {#if budget !== null}
      <p class="m-0 flex items-baseline gap-2 text-ui-micro text-muted-foreground">
        <span class="min-w-0 flex-1">
          {budget.toolCalls}/{budget.maxToolCalls} tool calls ·
          {aiElapsedLabel(budget.elapsedMs)} of {aiElapsedLabel(budget.maxWallClockMs)} budget
        </span>
        {#if budgetLabel !== null}
          <span class="flex-none font-medium text-yellow-600">{budgetLabel}</span>
        {/if}
      </p>
    {/if}
    {#if views.length > 0}
      <ul class="steps m-0 flex list-none flex-col gap-px p-0" aria-label="Tool calls">
        {#each views as view (view.id)}
          <li class="flex items-baseline gap-1.5 text-ui-micro">
            <span class="flex-none font-medium text-foreground">{view.tool}</span>
            <span
              class="min-w-0 flex-1 truncate font-mono text-muted-foreground"
              title={view.args}
              >{view.args}</span
            >
            <span
              class="flex-none tabular-nums {view.status === 'failed'
                ? 'text-destructive'
                : view.status === 'ok'
                  ? 'text-muted-foreground'
                  : 'text-primary-ink'}"
            >
              {view.status === "called" ? "running" : view.status}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
    {#each pending as approval (approval.requestID)}
      <AgenticApprovalCard {approval} {controller} {now} />
    {/each}
    {#if onCancel}
      <button
        class="self-start rounded-sm px-1 py-px text-ui-micro text-foreground underline hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-primary"
        type="button"
        onclick={onCancel}
      >
        Cancel run
      </button>
    {/if}
  </div>
{/if}
