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

<div class="explain-panel" role="complementary" aria-label={`AI explanation of ${path} hunk`}>
  <header class="head">
    <span class="file" title={path}>{path}</span>
    <span class="hunk-label">{label}</span>
    <span class="spacer"></span>
    {#if outcome}
      <button class="mini" type="button" onclick={() => void copy()}>Copy</button>
      <button class="mini" type="button" onclick={retry}>Re-run</button>
    {/if}
    <button class="mini" type="button" aria-label="Close explanation" onclick={onClose}>
      ×
    </button>
  </header>

  <div class="body">
    {#if outcome}
      <p class="text">{outcome.result.text}</p>
      <p class="meta">via {outcome.result.model} · {outcome.result.backend}</p>
    {:else if error}
      <p class="error" role="alert">{error}</p>
      {#if needsOptIn}
        <div class="opts">
          <button
            class="mini primary"
            type="button"
            onclick={() => {
              ai.setOptIn(repoId, true);
              retry();
            }}
          >
            Allow AI for this repo
          </button>
          <button
            class="mini"
            type="button"
            onclick={() => {
              settingsOpen = true;
            }}
          >
            Configure…
          </button>
        </div>
      {:else}
        <button class="mini" type="button" onclick={retry}>Retry</button>
      {/if}
    {:else}
      <p class="loading" role="status">
        <span class="spinner" aria-hidden="true"></span>
        {busy ? "Thinking…" : "Asking the AI…"}
      </p>
    {/if}
  </div>
</div>

<AiSettings bind:open={settingsOpen} />

<style>
  .explain-panel {
    position: absolute;
    left: 0.75rem;
    right: 0.75rem;
    bottom: 0.5rem;
    z-index: 30;
    max-width: 34rem;
    max-height: 45%;
    display: flex;
    flex-direction: column;
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-medium, 12px);
    box-shadow: 0 8px 24px color-mix(in srgb, black 35%, transparent);
    font-size: 0.8125rem;
  }

  .head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.375rem 0.5rem 0.375rem 0.75rem;
    border-bottom: 1px solid var(--m3-outline-variant, var(--m3-primary));
  }

  .file {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.75rem;
    font-weight: 600;
  }

  .hunk-label {
    flex: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font-family: ui-monospace, Consolas, monospace;
    font-size: 0.6875rem;
  }

  .spacer {
    flex: 1;
  }

  .body {
    overflow-y: auto;
    padding: 0.5rem 0.75rem;
  }

  .text {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    line-height: 1.45;
  }

  .meta {
    margin: 0.375rem 0 0;
    font-size: 0.6875rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .error {
    margin: 0 0 0.375rem;
    color: var(--m3-error, inherit);
    overflow-wrap: anywhere;
  }

  .loading {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
  }

  .opts {
    display: flex;
    gap: 0.375rem;
    flex-wrap: wrap;
  }

  .spinner {
    width: 0.75rem;
    height: 0.75rem;
    border: 2px solid color-mix(in srgb, var(--m3-primary) 35%, transparent);
    border-top-color: var(--m3-primary);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .mini {
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.5rem;
    border-radius: var(--m3-shape-full, 9999px);
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    background: none;
    color: var(--m3-on-surface);
    cursor: pointer;
    white-space: nowrap;
  }

  .mini.primary {
    background: var(--m3-primary);
    color: var(--m3-on-primary);
    border-color: transparent;
  }

  .mini:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }
</style>
