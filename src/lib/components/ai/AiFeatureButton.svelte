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
   *     Configure… / Cancel. `popUp` prefers the upward side for bars
   *     docked at the window's bottom edge. Overlays are portaled to <body>
   *     and clamped to the viewport, so a clipping ancestor (SplitPane) can
   *     never cut them off.
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
  import { floating } from "$lib/components/overlay/floating";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
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
    /** Open the confirm/fail overlays UPWARD (for docked-bottom bars). */
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
  <span class="ai-feature relative inline-flex {rootClass ?? ''}" bind:this={root}>
    {#if icon}
      <Button
        variant="outline"
        size="icon"
        class="sparkle size-7 rounded-full text-primary"
        type="button"
        {disabled}
        aria-label={ariaLabel ?? title ?? label}
        aria-busy={busy}
        title={title ?? label}
        onclick={openAction}
      >
        <span class="icon-box relative block" class:busy>
          <svg
            class="icon block {busy ? 'opacity-25' : ''}"
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
            <Spinner class="icon-spinner absolute inset-0 m-auto size-[0.9rem] text-primary" aria-hidden="true" />
          {/if}
        </span>
      </Button>
    {:else}
      <Button
        variant="outline"
        size="xs"
        class="feature-btn h-auto gap-[0.3rem] rounded-full px-[0.6rem] py-[0.15rem] text-[0.72rem] text-muted-foreground {compact
          ? 'compact px-2 py-[0.05rem] text-[0.6875rem]'
          : ''}"
        type="button"
        {disabled}
        aria-label={ariaLabel ?? title ?? label}
        title={title ?? label}
        onclick={openAction}
      >
        {#if busy}
          <Spinner class="size-[0.65rem] text-primary" aria-hidden="true" />
        {/if}
        {label}
      </Button>
    {/if}

    {#if confirmOpen}
      <div
        class="confirm invisible fixed left-0 top-0 z-40 max-h-[calc(100vh_-_1rem)] w-max max-w-[min(18rem,calc(100vw_-_1rem))] overflow-y-auto rounded-lg border border-border bg-popover p-2 text-left text-popover-foreground shadow-[0_4px_16px_rgba(0,0,0,0.3)]"
        role="dialog"
        aria-label="Allow AI for this repository"
        use:floating={{ anchor: root, above: popUp, alignEnd: icon }}
      >
        <p class="confirm-text m-0 mb-[0.375rem] text-xs">{confirmText.replace("{backend}", backendLabel)}</p>
        <div class="confirm-actions flex flex-row flex-wrap gap-1">
          <Button variant="default" size="xs" class="mini h-auto rounded-full px-2 py-[0.15rem] text-[0.72rem]" type="button" onclick={() => void allowAndRun(true)}>
            Always allow
          </Button>
          <Button variant="outline" size="xs" class="mini h-auto rounded-full px-2 py-[0.15rem] text-[0.72rem]" type="button" onclick={() => void allowAndRun(false)}>
            Just once
          </Button>
          <Button
            variant="outline"
            size="xs"
            class="mini h-auto rounded-full px-2 py-[0.15rem] text-[0.72rem]"
            type="button"
            onclick={() => {
              confirmOpen = false;
              settingsOpen = true;
            }}
          >
            Configure…
          </Button>
          <Button variant="outline" size="xs" class="mini h-auto rounded-full px-2 py-[0.15rem] text-[0.72rem]" type="button" onclick={() => (confirmOpen = false)}>
            Cancel
          </Button>
        </div>
      </div>
    {/if}

    {#if error && !confirmOpen}
      <div
        class="fail invisible fixed left-0 top-0 z-40 flex max-h-[calc(100vh_-_1rem)] w-max max-w-[min(18rem,calc(100vw_-_1rem))] flex-row flex-wrap items-center gap-[0.375rem] overflow-y-auto rounded-lg border border-border bg-popover p-2 text-left text-popover-foreground shadow-[0_4px_16px_rgba(0,0,0,0.3)]"
        role="alert"
        use:floating={{ anchor: root, above: popUp, alignEnd: icon }}
      >
        <span class="fail-text text-[0.72rem] text-destructive [overflow-wrap:anywhere]">{error}</span>
        <Button variant="outline" size="xs" class="mini h-auto rounded-full px-2 py-[0.15rem] text-[0.72rem]" type="button" onclick={() => void doRun()}>Retry</Button>
        <Button variant="outline" size="xs" class="mini h-auto rounded-full px-2 py-[0.15rem] text-[0.72rem]" type="button" onclick={() => (error = null)}>Dismiss</Button>
      </div>
    {/if}
  </span>
{/if}

<AiSettings bind:open={settingsOpen} />
