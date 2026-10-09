<script lang="ts">
  /**
   * StashMessageButton (M12, lane C) — "Suggest message" for the stash
   * push form. Visible only when the repo is opted in; runs the
   * stash-message runner with a summary of the FULL work-in-progress diff
   * (HEAD → worktree, so index + unstaged changes both inform the message —
   * the runner's own default gathers only the staged side, which would
   * misdescribe most stashes). The runner caps the input (2k chars). A
   * clean worktree is refused locally (no request burned). The parsed
   * one-line message is handed to the caller via `onMessage` (StashPanel
   * fills its input).
   */
  import { ai } from "$lib/ai/ai.svelte";
  import { stagedDiffText, type FeatureOutcome } from "$lib/ai/features";
  import { repoDiff } from "$lib/ipc/client";
  import type { FileDiff } from "$lib/ipc/types";
  import { toast } from "$lib/toast";
  import AiFeatureButton from "./AiFeatureButton.svelte";

  let {
    repoId,
    onMessage,
  }: {
    repoId: string;
    /** Called with the suggested one-line message. */
    onMessage: (message: string) => void;
  } = $props();

  async function run(): Promise<FeatureOutcome> {
    const files = await repoDiff(repoId, "head", "worktree");
    const summary = wipSummary(files);
    if (summary.trim().length === 0) {
      throw new Error("nothing to stash — the worktree is clean");
    }
    return ai.run("stash-message", { repoId, diff: summary });
  }

  /** Per-file stats + (capped downstream) patch text for the whole WIP. */
  function wipSummary(files: readonly FileDiff[]): string {
    const stats = files
      .map((f) => `- ${f.path} (+${f.additions}/-${f.deletions})`)
      .join("\n");
    return `${stats}\n\n${stagedDiffText([...files])}`;
  }

  function onResult(outcome: FeatureOutcome): void {
    const message = outcome.message?.subject ?? outcome.result.text.trim();
    if (message) onMessage(message);
  }
</script>

<AiFeatureButton
  {repoId}
  label="Suggest message"
  title="Draft a stash message with AI"
  confirmText={"Send a summary of your changes to {backend} to draft a stash message?"}
  errorPrefix="AI stash message failed"
  {run}
  {onResult}
/>
