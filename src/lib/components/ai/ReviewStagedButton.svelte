<!--
  ReviewStagedButton (M12, lane C) — "Review staged…" for the commit bar.

  Visible only when the repo is opted in (same gating contract as the
  explain-hunk button). Runs the review-staged runner (it gathers the
  staged diff itself — features.ts) and presents the findings in a modal
  (shadcn ui/dialog chrome, like the safety dialogs). An empty staging area
  surfaces as the runner's "nothing is staged to review" error via the
  shared AiFeatureButton's inline fail UI. Findings are advisory ONLY —
  nothing is auto-applied.
-->
<script lang="ts">
  import { ai } from "$lib/ai/ai.svelte";
  import type { AiRunOptions } from "$lib/ai/ai.svelte";
  import type { FeatureOutcome } from "$lib/ai/features";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { toast } from "$lib/toast";
  import AiFeatureButton from "./AiFeatureButton.svelte";

  let { repoId }: { repoId: string } = $props();

  let findings = $state<string | null>(null);
  let open = $state(false);
  let closeButton = $state<HTMLElement | null>(null);

  function run(opts?: AiRunOptions): Promise<FeatureOutcome> {
    return ai.run("review-staged", { repoId }, undefined, opts);
  }

  function onResult(outcome: FeatureOutcome): void {
    findings = outcome.result.text;
    open = true;
    toast("Staged-change review ready", { kind: "success" });
  }

  // Focus lands on Close (the old data-autofocus convention) on open.
  $effect(() => {
    if (open) {
      requestAnimationFrame(() => closeButton?.focus());
    }
  });

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

<Dialog.Root bind:open>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
  <Dialog.Content
    class="dialog w-[min(34rem,calc(100vw-3rem))] max-w-none gap-0 rounded-lg bg-card px-6 py-5 text-sm text-popover-foreground shadow-xl sm:max-w-none"
    role="dialog"
    aria-modal="true"
    aria-labelledby="review-staged-title"
    showCloseButton={false}
    escapeKeydownBehavior="ignore"
    interactOutsideBehavior="ignore"
    onkeydown={(e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        open = false;
      }
    }}
  >
    <Dialog.Title id="review-staged-title" class="m-0 mb-2 text-[1.125rem] font-medium text-foreground">Staged changes review</Dialog.Title>
    {#if findings !== null}
      <div class="rs-findings mb-3 max-h-[55vh] overflow-y-auto rounded-xl border border-border bg-muted/50 px-3 py-[0.625rem] focus-visible:-outline-offset-2 focus-visible:outline-2 focus-visible:outline-primary">
        <p class="rs-text m-0 text-[0.8125rem] leading-[1.45] whitespace-pre-wrap [overflow-wrap:anywhere]">{findings}</p>
      </div>
      <p class="rs-note m-0 mb-3 text-[0.6875rem] text-muted-foreground">
        Advisory only — nothing was changed. Verify findings before acting.
      </p>
    {/if}
    <div class="mt-4 flex flex-wrap justify-end gap-2">
      <Button variant="outline" class="text-muted-foreground" size="sm" type="button" onclick={() => void copy()}>
        Copy
      </Button>
      <Button bind:ref={closeButton} size="sm" type="button" onclick={() => (open = false)}>
        Close
      </Button>
    </div>
  </Dialog.Content>
</Dialog.Root>
