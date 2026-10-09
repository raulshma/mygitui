<script lang="ts">
  /**
   * ReviewStagedButton (M12, lane C) — "Review staged…" for the commit bar.
   *
   * Visible only when the repo is opted in (same gating contract as the
   * explain-hunk button). Runs the review-staged runner (it gathers the
   * staged diff itself — features.ts) and presents the findings in a modal
   * (DialogShell chrome, like the safety dialogs). An empty staging area
   * surfaces as the runner's "nothing is staged to review" error via the
   * shared AiFeatureButton's inline fail UI. Findings are advisory ONLY —
   * nothing is auto-applied.
   */
  import { ai } from "$lib/ai/ai.svelte";
  import type { AiRunOptions } from "$lib/ai/ai.svelte";
  import type { FeatureOutcome } from "$lib/ai/features";
  import DialogShell from "$lib/components/safety/DialogShell.svelte";
  import { toast } from "$lib/toast";
  import AiFeatureButton from "./AiFeatureButton.svelte";

  let { repoId }: { repoId: string } = $props();

  let findings = $state<string | null>(null);
  let open = $state(false);

  function run(opts?: AiRunOptions): Promise<FeatureOutcome> {
    return ai.run("review-staged", { repoId }, undefined, opts);
  }

  function onResult(outcome: FeatureOutcome): void {
    findings = outcome.result.text;
    open = true;
    toast("Staged-change review ready", { kind: "success" });
  }

  async function copy(): Promise<void> {
    if (!findings) return;
    try {
      await navigator.clipboard.writeText(findings);
      toast("Review copied", { kind: "success" });
    } catch {
      toast("Copy failed — the clipboard is unavailable", { kind: "error" });
    }
  }
</script>

<AiFeatureButton
  {repoId}
  label="Review staged…"
  title="Review the staged diff with AI"
  confirmText={"Send the staged diff to {backend} for review?"}
  errorPrefix="AI review failed"
  {run}
  {onResult}
/>

<DialogShell
  bind:open
  labelledBy="review-staged-title"
  width="min(34rem, calc(100vw - 3rem))"
>
  {#snippet children()}
    <h2 id="review-staged-title" class="rs-title">Staged changes review</h2>
    {#if findings !== null}
      <div class="rs-findings">
        <p class="rs-text">{findings}</p>
      </div>
      <p class="rs-note">
        Advisory only — nothing was changed. Verify findings before acting.
      </p>
    {/if}
  {/snippet}
  {#snippet actions()}
    <button class="rs-secondary" type="button" onclick={() => void copy()}>
      Copy
    </button>
    <button class="rs-primary" type="button" data-autofocus onclick={() => (open = false)}>
      Close
    </button>
  {/snippet}
</DialogShell>

<style>
  .rs-title {
    margin: 0 0 0.5rem;
    font-size: 1.125rem;
    font-weight: 500;
  }

  .rs-findings {
    max-height: 55vh;
    overflow-y: auto;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-medium, 12px);
    background: var(--m3-surface-container-low, var(--m3-surface));
    padding: 0.625rem 0.75rem;
    margin-bottom: 0.75rem;
  }

  .rs-findings:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -2px;
  }

  .rs-text {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    line-height: 1.45;
    font-size: 0.8125rem;
  }

  .rs-note {
    margin: 0 0 0.75rem;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .rs-primary {
    border: none;
    border-radius: var(--m3-shape-full, 9999px);
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    font: inherit;
    font-weight: 500;
    padding: 0.4rem 1.1rem;
    cursor: pointer;
  }

  .rs-secondary {
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    padding: 0.4rem 0.9rem;
    cursor: pointer;
  }

  .rs-primary:focus-visible,
  .rs-secondary:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }
</style>
