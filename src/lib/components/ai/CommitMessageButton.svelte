<script lang="ts">
  /**
   * AI commit-message button (M6, lane H1) — the first wired AI feature.
   *
   * A sparkle icon button for the commit bar. First use in a repository
   * asks an inline opt-in ("Send staged diff to <backend>?") with a
   * Configure action that opens the AI settings dialog; after that a click
   * runs the commit-message feature (staged diff → AI → subject/body).
   *
   * EVENT CONTRACT (CommitBar integration — orchestrator lane):
   *   On success the component dispatches a DOM CustomEvent
   *   "ai-commit-message" (bubbles: true) on its root element with
   *   `detail: { subject: string; body: string; backend: AiBackend; model: string }`.
   *   The owner of the commit textarea listens on the component tag:
   *   ```svelte
   *   <CommitMessageButton {repoId}
   *     onai-commit-message={(e) => {
   *       const d = e.detail;
   *       message = d.body ? `${d.subject}\n\n${d.body}` : d.subject;
   *     }} />
   *   ```
   *   (Svelte 5 forwards the unknown `on…` attribute to the root element as
   *   an event listener.) Failures toast + show an inline Retry.
   */
  import { ai } from "$lib/ai/ai.svelte";
  import { isAiError } from "$lib/ai/types";
  import AiSettings from "./AiSettings.svelte";
  import { toast } from "$lib/toast";

  let {
    repoId,
    disabled = false,
    onResult = undefined,
  }: {
    repoId: string;
    /** Disables the button (e.g. while a commit op runs). */
    disabled?: boolean;
    /** Typed alternative to the root-element `ai-commit-message` DOM event. */
    onResult?: (r: {
      subject: string;
      body: string;
      backend: string;
      model: string;
    }) => void;
  } = $props();

  let root: HTMLSpanElement | undefined = $state();
  let confirmOpen = $state(false);
  let settingsOpen = $state(false);
  let error = $state<string | null>(null);

  const runState = $derived(ai.peekState(repoId));
  const busy = $derived(runState.busy);
  /** Backend name for the opt-in copy. */
  const backendLabel = $derived(ai.config.backend === "openrouter" ? "OpenRouter" : "your opencode server");

  function openConfirm(): void {
    if (disabled || busy) return;
    error = null;
    if (ai.isOptedIn(repoId)) {
      void run();
    } else {
      confirmOpen = !confirmOpen;
    }
  }

  async function allowAndRun(always: boolean): Promise<void> {
    confirmOpen = false;
    if (always) ai.setOptIn(repoId, true);
    await run();
  }

  async function run(): Promise<void> {
    error = null;
    try {
      const outcome = await ai.generateCommitMessage(repoId);
      const message = outcome.message ?? { subject: outcome.result.text, body: "" };
      onResult?.({
        subject: message.subject,
        body: message.body,
        backend: outcome.result.backend,
        model: outcome.result.model,
      });
      root?.dispatchEvent(
        new CustomEvent("ai-commit-message", {
          detail: {
            subject: message.subject,
            body: message.body,
            backend: outcome.result.backend,
            model: outcome.result.model,
          },
          bubbles: true,
        }),
      );
      toast("Commit message generated — review before committing", { kind: "success" });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      error = message;
      toast(`AI commit message failed: ${message}`, { kind: "error" });
      // Not opted in anymore (revoked elsewhere): re-arm the first-use flow.
      if (isAiError(err) && err.kind === "opt-in") confirmOpen = true;
    }
  }
</script>

<span class="ai-commit" bind:this={root}>
  <button
    class="sparkle"
    type="button"
    {disabled}
    aria-label="Generate commit message with AI"
    title="Generate a commit message from the staged diff"
    onclick={openConfirm}
  >
    {#if busy}
      <span class="spinner" aria-hidden="true"></span>
    {:else}
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
    {/if}
  </button>

  {#if confirmOpen}
    <div class="confirm" role="dialog" aria-label="Allow AI for this repository">
      <p class="confirm-text">
        Send the staged diff to {backendLabel} to draft a commit message?
      </p>
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
      <button class="mini" type="button" onclick={() => void run()}>Retry</button>
      <button class="mini" type="button" onclick={() => (error = null)}>Dismiss</button>
    </div>
  {/if}
</span>

<AiSettings bind:open={settingsOpen} />

<style>
  .ai-commit {
    position: relative;
    display: inline-flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 0.25rem;
  }

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

  .icon {
    display: block;
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

  .confirm,
  .fail {
    position: absolute;
    top: calc(100% + 0.25rem);
    right: 0;
    z-index: 30;
    width: max-content;
    max-width: 18rem;
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-medium, 8px);
    padding: 0.5rem;
    box-shadow: 0 4px 16px color-mix(in srgb, black 30%, transparent);
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
    text-align: left;
  }

  .confirm-text {
    margin: 0;
    font-size: 0.75rem;
  }

  .confirm-actions,
  .fail {
    display: flex;
  }

  .confirm-actions {
    flex-direction: row;
    flex-wrap: wrap;
    gap: 0.25rem;
  }

  .fail {
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
