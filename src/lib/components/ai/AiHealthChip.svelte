<script lang="ts">
  /**
   * AiHealthChip (M12, lane C) — one dot + label summarizing the AI
   * backends' connection health (supervisor status map → aiHealth()).
   *
   *   green  "connected"           preferred backend ok
   *   amber  "degraded/checking"   probing, needs credentials, or the
   *                               fallback is carrying the traffic
   *   red    "down"                preferred down, no usable fallback
   *   gray   "not configured"      nothing probed yet
   *
   * Hover shows per-backend status + latency; click dispatches the typed
   * "open-ai-settings" UI event (App / palette wiring happens at
   * integration — the chip itself stays dependency-light).
   */
  import { ai } from "$lib/ai/ai.svelte";
  import { aiHealth } from "$lib/ai/health";
  import { emitUiEvent } from "$lib/palette/events";

  // The supervisor is a plain Svelte-store-contract observable, not a
  // runes store — mirror it into $state like AiSettings does.
  let statuses = $state(ai.supervisor.statuses);
  $effect(() => ai.supervisor.subscribe((next) => (statuses = next)));

  const view = $derived(
    aiHealth(statuses, ai.config.backend, ai.config.allowFallback !== false),
  );
</script>

<button
  class="ai-health {view.tone}"
  type="button"
  title={`AI: ${view.detail}`}
  aria-label={`AI connection: ${view.label} — open AI settings`}
  onclick={() => emitUiEvent("open-ai-settings")}
>
  <span class="dot" aria-hidden="true"></span>
  <span class="label">AI {view.label}</span>
</button>

<style>
  .ai-health {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    border: 1px solid var(--m3-outline-variant, var(--m3-primary));
    border-radius: var(--m3-shape-full, 9999px);
    background: none;
    color: var(--m3-on-surface-variant, var(--m3-on-surface));
    font: inherit;
    font-size: 0.6875rem;
    padding: 0.1rem 0.55rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .ai-health:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
  }

  .ai-health:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: 1px;
  }

  .dot {
    flex: none;
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    background: var(--m3-outline-variant, #9e9e9e); /* off / unknown */
  }

  .ai-health.ok .dot {
    background: #2e7d32; /* green */
  }

  .ai-health.degraded .dot {
    background: #f9a825; /* amber */
  }

  .ai-health.down .dot {
    background: var(--m3-error, #ba1a1a); /* red */
  }
</style>
