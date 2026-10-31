<script lang="ts">
  /**
   * ExplainHunkPanel (M12, lane C) — the docked popover panel DiffViewer
   * opens from a hunk header's "Explain" button: runs the explain-hunk
   * AI feature for one hunk and shows the answer with loading, error and
   * copy affordances.
   *
   * Docked to the viewer's bottom edge (NOT anchored to the row) so the
   * 60fps virtualization slices — which clip at their bucket bounds —
   * never cut the panel off. Owns an <AiSettings> instance for the
   * "Configure…" action when the backend needs setup.
   */
  import { ai } from "$lib/ai/ai.svelte";
  import { isAiError } from "$lib/ai/types";
  import type { FeatureOutcome } from "$lib/ai/features";
  import AiSettings from "./AiSettings.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { toast } from "$lib/toast";

  let {
    repoId,
    path,
    hunkText,
    /** One-line header for the panel (the hunk's @@ header). */
    label = "",
    onClose,
  }: {
    repoId: string;
    path: string;
    /** Unified-diff text of the hunk (features.formatHunkText). */
    hunkText: string;
    label?: string;
    onClose: () => void;
  } = $props();

  let outcome = $state<FeatureOutcome | null>(null);
  let error = $state<string | null>(null);
  let needsOptIn = $state(false);
  let settingsOpen = $state(false);
  /** Bumped per attempt so re-running the effect re-fires the request. */
  let attempt = $state(0);

  const busy = $derived(ai.peekState(repoId).busy);

  $effect(() => {
    // Fresh panel per hunk: reset and run.
    void hunkText;
    const token = attempt;
    outcome = null;
    error = null;
    needsOptIn = false;
    ai.run("explain-hunk", { repoId, path, hunk: hunkText })
      .then((result) => {
        if (token !== attempt) return;
        outcome = result;
      })
      .catch((err: unknown) => {
        if (token !== attempt) return;
        error = err instanceof Error ? err.message : String(err);
        needsOptIn = isAiError(err) && err.kind === "opt-in";
      });
  });

  function retry(): void {
    attempt++;
  }

  async function copy(): Promise<void> {
    const text = outcome?.result.text;
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      toast("Explanation copied", { kind: "success" });
    } catch {
      toast("Copy failed — the clipboard is unavailable", { kind: "error" });
    }
  }
</script>

<div class="explain-panel absolute bottom-2 left-3 right-3 z-30 flex max-h-[45%] max-w-[34rem] flex-col rounded-xl border border-border bg-popover text-[0.8125rem] text-popover-foreground shadow-[0_8px_24px_rgba(0,0,0,0.35)]" role="complementary" aria-label={`AI explanation of ${path} hunk`}>
  <header class="head flex-none flex items-center gap-2 border-b border-border pb-[0.375rem] pl-3 pr-2 pt-[0.375rem]">
    <span class="file min-w-0 truncate font-mono text-xs font-semibold" title={path}>{path}</span>
    <span class="hunk-label flex-none font-mono text-[0.6875rem] text-muted-foreground">{label}</span>
    <span class="spacer flex-1"></span>
    {#if outcome}
      <Button variant="outline" size="xs" class="mini h-auto whitespace-nowrap rounded-full px-2 py-[0.15rem] text-[0.72rem]" type="button" onclick={() => void copy()}>Copy</Button>
      <Button variant="outline" size="xs" class="mini h-auto whitespace-nowrap rounded-full px-2 py-[0.15rem] text-[0.72rem]" type="button" onclick={retry}>Re-run</Button>
    {/if}
    <Button variant="outline" size="xs" class="mini h-auto whitespace-nowrap rounded-full px-2 py-[0.15rem] text-[0.72rem]" type="button" aria-label="Close explanation" onclick={onClose}>
      ×
    </Button>
  </header>

  <div class="body overflow-y-auto px-3 py-2">
    {#if outcome}
      <p class="text m-0 leading-[1.45] whitespace-pre-wrap [overflow-wrap:anywhere]">{outcome.result.text}</p>
      <p class="meta mb-0 mt-[0.375rem] text-[0.6875rem] text-muted-foreground">via {outcome.result.model} · {outcome.result.backend}</p>
    {:else if error}
      <p class="error m-0 mb-[0.375rem] text-destructive [overflow-wrap:anywhere]" role="alert">{error}</p>
      {#if needsOptIn}
        <div class="opts flex flex-wrap gap-[0.375rem]">
          <Button
            variant="default"
            size="xs"
            class="mini h-auto whitespace-nowrap rounded-full px-2 py-[0.15rem] text-[0.72rem]"
            type="button"
            onclick={() => {
              ai.setOptIn(repoId, true);
              retry();
            }}
          >
            Allow AI for this repo
          </Button>
          <Button
            variant="outline"
            size="xs"
            class="mini h-auto whitespace-nowrap rounded-full px-2 py-[0.15rem] text-[0.72rem]"
            type="button"
            onclick={() => {
              settingsOpen = true;
            }}
          >
            Configure…
          </Button>
        </div>
      {:else}
        <Button variant="outline" size="xs" class="mini h-auto whitespace-nowrap rounded-full px-2 py-[0.15rem] text-[0.72rem]" type="button" onclick={retry}>Retry</Button>
      {/if}
    {:else}
      <p class="loading m-0 flex items-center gap-2 text-muted-foreground" role="status">
        <Spinner class="size-3 text-primary" aria-hidden="true" />
        {busy ? "Thinking…" : "Asking the AI…"}
      </p>
    {/if}
  </div>
</div>

<AiSettings bind:open={settingsOpen} />
