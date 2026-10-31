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
  import { isTauri } from "$lib/entry/dragdrop";
  import { emitUiEvent } from "$lib/palette/events";
  import { Button } from "$lib/components/ui/button/index.js";
  import { onMount } from "svelte";

  // The supervisor is a plain Svelte-store-contract observable, not a
  // runes store — mirror it into $state like AiSettings does.
  let statuses = $state(ai.supervisor.statuses);
  $effect(() => ai.supervisor.subscribe((next) => (statuses = next)));
  onMount(() => {
    // The store may have been evaluated before the Tauri bridge was ready.
    // Probe after mount so the health chip cannot remain unknown forever.
    if (isTauri() && ai.supervisor.status("opencode").status === "unknown") {
      void ai.supervisor.check("opencode");
    }
  });

  const view = $derived(
    aiHealth(statuses, ai.config.backend, ai.config.allowFallback !== false),
  );

  let { compact = false }: { compact?: boolean } = $props();
</script>

<Button
  variant="outline"
  size="xs"
  class="ai-health {view.tone} h-auto min-h-7 gap-[0.3rem] rounded-full px-[0.55rem] py-[0.1rem] text-[0.6875rem] text-muted-foreground {compact
    ? 'compact size-7 justify-center gap-0 p-0'
    : ''}"
  type="button"
  title={`AI: ${view.detail}`}
  aria-label={`AI connection: ${view.label} — open AI settings`}
  onclick={() => emitUiEvent("open-ai-settings")}
>
  <span
    class="dot flex-none size-2 rounded-full {view.tone === 'ok'
      ? 'bg-[var(--app-added)]'
      : view.tone === 'degraded'
        ? 'bg-[var(--app-modified)]'
        : view.tone === 'down'
          ? 'bg-destructive'
          : 'bg-muted-foreground'}"
    aria-hidden="true"></span>
  <span class="label {compact ? 'sr-only' : ''}">AI {view.label}</span>
</Button>
