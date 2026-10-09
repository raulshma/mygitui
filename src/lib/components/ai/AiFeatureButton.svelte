<script lang="ts">
  /**
   * AI feature button (M12, lane C) — the shared skin for the dormant AI
   * feature runners, extracted from CommitMessageButton's UX contract:
   *
   *   - Optional visibility gating on per-repo opt-in (`gateVisibility`):
   *     set true for buttons that should only exist once AI is allowed for
   *     the repo (review-staged, stash-message); false to always show and
   *     ask on click (the commit-message behaviour).
   *   - First use / revoked-mid-session shows the inline confirm
   *     ("Send … to <backend>?") with Always allow / Just once /
   *     Configure… / Cancel.
   *   - Busy spinner while `run()` is in flight; failures toast + render
   *     an inline Retry/Dismiss; `AiError` kind "opt-in" re-arms the
   *     confirm instead.
   *
   * The runner itself is a caller closure (`run`), so the same component
   * serves every FeatureKind; results flow back through `onResult`.
   */
  import { ai } from "$lib/ai/ai.svelte";
  import { isAiError } from "$lib/ai/types";
  import type { FeatureOutcome } from "$lib/ai/features";
  import AiSettings from "./AiSettings.svelte";
  import { toast } from "$lib/toast";

  let {
    repoId,
    label,
    title = undefined,
    confirmText,
    errorPrefix,
    gateVisibility = true,
    disabled = false,
    compact = false,
    run,
    onResult = undefined,
  }: {
    repoId: string;
    /** Button text. */
    label: string;
    /** Hover tooltip. */
    title?: string | undefined;
    /** First-use copy ("Send the staged diff to <backend> to …?"). */
    confirmText: string;
    /** Toast/error prefix, e.g. "AI review failed". */
    errorPrefix: string;
    /** Render nothing until the repo is opted in (default true). */
    gateVisibility?: boolean;
    disabled?: boolean;
    /** Tighter padding for in-row placement. */
    compact?: boolean;
    /** The feature runner (closes over any context it needs). */
    run: () => Promise<FeatureOutcome>;
    /** Called with the finished outcome (the caller owns the result UI). */
    onResult?: (outcome: FeatureOutcome) => void;
  } = $props();

  let root = $state<HTMLSpanElement | undefined>(undefined);
  let confirmOpen = $state(false);
  let settingsOpen = $state(false);
  let error = $state<string | null>(null);
  let running = $state(false);

  const runState = $derived(ai.peekState(repoId));
  const busy = $derived(running || runState.busy);
  const backendLabel = $derived(
    ai.config.backend === "openrouter" ? "OpenRouter" : "your opencode server",
  );

  function openAction(): void {
    if (disabled || busy) return;
    error = null;
    if (ai.isOptedIn(repoId)) {
      void doRun();
    } else {
      confirmOpen = !confirmOpen;
    }
  }

  async function allowAndRun(always: boolean): Promise<void> {
    confirmOpen = false;
    if (always) ai.setOptIn(repoId, true);
    await doRun();
  }

  async function doRun(): Promise<void> {
    running = true;
    error = null;
    try {
      const outcome = await run();
      onResult?.(outcome);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      toast(`${errorPrefix}: ${message}`, { kind: "error" });
      // Revoked elsewhere (or gateVisibility=false): re-arm the first-use flow.
      if (isAiError(err) && err.kind === "opt-in") confirmOpen = true;
    } finally {
      running = false;
    }
  }
</script>

{#if !gateVisibility || ai.isOptedIn(repoId)}
  <span class="ai-feature" bind:this={root}>
    <button
      class="feature-btn"
      class:compact
      type="button"
      {disabled}
      aria-label={title ?? label}
      title={title ?? label}
      onclick={openAction}
    >
      {#if busy}
        <span class="spinner" aria-hidden="true"></span>
      {/if}
      {label}
    </button>

    {#if confirmOpen}
      <div class="confirm" role="dialog" aria-label="Allow AI for this repository">
        <p class="confirm-text">{confirmText.replace("{backend}", backendLabel)}</p>
        <div class="confirm-actions">
          <button class="mini primary" type="button" onclick={() => void allowAndRun(true)}>
            Always allow
          </button>
          <button class="mini" type="button" onclick={() => void allowAndRun(false)}>
            Just once
          </button>
          <button
            class="mini"
            type="button"
            onclick={() => {
              confirmOpen = false;
              settingsOpen = true;
            }}
          >
            Configure…
          </button>
          <button class="mini" type="button" onclick={() => (confirmOpen = false)}>
            Cancel
          </button>
        </div>
      </div>
    {/if}

    {#if error && !confirmOpen}
      <div class="fail" role="alert">
        <span class="fail-text">{error}</span>
        <button class="mini" type="button" onclick={() => void doRun()}>Retry</button>
        <button class="mini" type="button" onclick={() => (error = null)}>Dismiss</button>
      </div>
    {/if}
  </span>
{/if}

<AiSettings bind:open={settingsOpen} />

<style>
  .ai-feature {
    position: relative;
    display: inline-flex;
  }

  .feature-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.72rem;
    padding: 0.15rem 0.6rem;
    cursor: pointer;
    white-space: nowrap;
  }
  .feature-btn.compact {
    font-size: 0.6875rem;
    padding: 0.05rem 0.5rem;
  }
  .feature-btn:hover:not(:disabled) {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }
  .feature-btn:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }
  .feature-btn:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .spinner {
    width: 0.65rem;
    height: 0.65rem;
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

  .confirm,
  .fail {
    position: absolute;
    top: calc(100% + 0.25rem);
    left: 0;
    z-index: 40;
    width: max-content;
    max-width: 18rem;
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-medium, 8px);
    padding: 0.5rem;
    box-shadow: 0 4px 16px color-mix(in srgb, black 30%, transparent);
    text-align: left;
  }

  .confirm-text {
    margin: 0 0 0.375rem;
    font-size: 0.75rem;
  }

  .confirm-actions {
    display: flex;
    flex-direction: row;
    flex-wrap: wrap;
    gap: 0.25rem;
  }

  .fail {
    display: flex;
    flex-direction: row;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.375rem;
  }

  .fail-text {
    font-size: 0.72rem;
    color: var(--m3-error, inherit);
    overflow-wrap: anywhere;
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
