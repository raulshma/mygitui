<script lang="ts">
  /**
   * AI feature button (M12, lane C) — the shared skin for every AI feature
   * runner (absorbs CommitMessageButton's commit-bar variant):
   *
   *   - Optional visibility gating on per-repo opt-in (`gateVisibility`):
   *     set true for buttons that should only exist once AI is allowed for
   *     the repo (review-staged, stash-message); false to always show and
   *     ask on click (the commit-message behaviour).
   *   - First use / revoked-mid-session shows the inline confirm
   *     ("Send … to <backend>?") with Always allow / Just once /
   *     Configure… / Cancel. `popUp` anchors the overlays UPWARD (the
   *     commit bar is docked at the window's bottom edge — downward
   *     popups would render off-screen).
   *   - Busy spinner while `run()` is in flight; failures toast + render
   *     an inline Retry/Dismiss; `AiError` kind "opt-in" re-arms the
   *     confirm instead. `successToast` fires on success (the commit
   *     message's "review before committing" nudge).
   *   - `icon` renders the compact sparkle circle (the commit-bar
   *     variant) instead of the text pill.
   *
   * The runner itself is a caller closure (`run`), so the same component
   * serves every FeatureKind; results flow back through `onResult` and —
   * when `resultEvent` is set — a bubbling DOM CustomEvent on the root
   * element (detail: the FeatureOutcome) for non-Svelte-friendly owners.
   */
  import { ai } from "$lib/ai/ai.svelte";
  import type { AiRunOptions } from "$lib/ai/ai.svelte";
  import { isAiError } from "$lib/ai/types";
  import type { FeatureOutcome } from "$lib/ai/features";
  import AiSettings from "./AiSettings.svelte";
  import { toast } from "$lib/toast";

  let {
    repoId,
    label,
    title = undefined,
    ariaLabel = undefined,
    confirmText,
    errorPrefix,
    gateVisibility = true,
    disabled = false,
    compact = false,
    icon = false,
    popUp = false,
    successToast = undefined,
    resultEvent = undefined,
    run,
    onResult = undefined,
    /** Extra class(es) on the root span (e.g. a legacy hook for tests). */
    rootClass = undefined,
  }: {
    repoId: string;
    /** Button text. */
    label: string;
    /** Hover tooltip. */
    title?: string | undefined;
    /** Accessible name when it should differ from the tooltip (icon mode). */
    ariaLabel?: string | undefined;
    /** First-use copy ("Send the staged diff to <backend> to …?"). */
    confirmText: string;
    /** Toast/error prefix, e.g. "AI review failed". */
    errorPrefix: string;
    /** Render nothing until the repo is opted in (default true). */
    gateVisibility?: boolean;
    disabled?: boolean;
    /** Tighter padding for in-row placement. */
    compact?: boolean;
    /** Sparkle icon circle (commit-bar variant) instead of the text pill. */
    icon?: boolean;
    /** Open the confirm/fail overlays UPWARD (docked-bottom bars). */
    popUp?: boolean;
    /** Success toast copy, when the feature wants one. */
    successToast?: string | undefined;
    /** Dispatch a bubbling DOM CustomEvent with this name on success. */
    resultEvent?: string | undefined;
    /** The feature runner (closes over any context it needs). */
    run: (opts?: AiRunOptions) => Promise<FeatureOutcome>;
    /** Called with the finished outcome (the caller owns the result UI). */
    onResult?: (outcome: FeatureOutcome) => void;
    /** Extra class(es) on the root span (e.g. a legacy hook for tests). */
    rootClass?: string | undefined;
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
      const opts = ai.runOptsFor(repoId);
      const outcome = await run(opts);
      onResult?.(outcome);
      if (successToast) toast(successToast, { kind: "success" });
      if (resultEvent) {
        root?.dispatchEvent(new CustomEvent(resultEvent, { detail: outcome, bubbles: true }));
      }
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
  <span class="ai-feature {rootClass ?? ''}" class:pop-up={popUp} bind:this={root}>
    {#if icon}
      <button
        class="sparkle"
        type="button"
        {disabled}
        aria-label={ariaLabel ?? title ?? label}
        aria-busy={busy}
        title={title ?? label}
        onclick={openAction}
      >
        <span class="icon-box" class:busy>
          <svg
            class="icon"
            aria-hidden="true"
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="currentColor"
          >
            <path
              d="M12 2l2.09 6.26L20 10.35l-5.91 2.09L12 18.7l-2.09-6.26L4 10.35l5.91-2.09L12 2z"
            />
            <path d="M19 15l1.18 3.32L23.5 19.5l-3.32 1.18L19 24l-1.18-3.32L14.5 19.5l3.32-1.18L19 15z" />
          </svg>
          {#if busy}
            <span class="spinner icon-spinner" aria-hidden="true"></span>
          {/if}
        </span>
      </button>
    {:else}
      <button
        class="feature-btn"
        class:compact
        type="button"
        {disabled}
        aria-label={ariaLabel ?? title ?? label}
        title={title ?? label}
        onclick={openAction}
      >
        {#if busy}
          <span class="spinner" aria-hidden="true"></span>
        {/if}
        {label}
      </button>
    {/if}

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

  /* Icon variant (commit-bar sparkle): a compact circular icon button. */
  .sparkle {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.75rem;
    height: 1.75rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-primary);
    cursor: pointer;
  }

  .sparkle:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  .sparkle:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .icon-box {
    position: relative;
    display: block;
  }

  .icon-box.busy .icon {
    opacity: 0.25;
  }

  .icon {
    display: block;
  }

  /* Overlays the sparkle while a generation is in flight. */
  .icon-spinner {
    position: absolute;
    inset: 0;
    margin: auto;
    width: 0.9rem;
    height: 0.9rem;
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

  /* Docked-bottom bars (commit bar): popups must open UPWARD or they
     render off-screen (invisible first-use dialog). Anchored to the right
     edge like the commit-bar sparkle it replaces. */
  .pop-up .confirm,
  .pop-up .fail {
    top: auto;
    bottom: calc(100% + 0.25rem);
    left: auto;
    right: 0;
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
