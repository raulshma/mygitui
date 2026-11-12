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
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import { onMount } from "svelte";

  // The supervisor is a plain Svelte-store-contract observable, not a
  // runes store — mirror it into $state like AiSettings does.
  let statuses = $state(ai.supervisor.statuses);
  $effect(() => ai.supervisor.subscribe((next) => (statuses = next)));
  onMount(() => {
    // The store may have been evaluated before the Tauri bridge was ready.
    // Probe after mount so the health chip cannot remain unknown forever —
    // but AFTER the boot window: a fresh launch fires repo_open/repo_status
    // for every restored tab, and the probe (PATH scan + `opencode
    // --version` spawn in attach mode) would compete with them. Idle-chained
    // so repos always win the first seconds.
    if (!isTauri() || ai.supervisor.status("opencode").status !== "unknown") {
      return;
    }
    let handle = 0;
    const idle = window.requestIdleCallback as
      | ((cb: () => void, opts?: { timeout?: number }) => number)
      | undefined;
    if (idle) {
      handle = idle(() => void ai.supervisor.check("opencode"), { timeout: 5000 });
    } else {
      handle = window.setTimeout(() => void ai.supervisor.check("opencode"), 2500);
    }
    return () => {
      window.cancelIdleCallback?.(handle);
      clearTimeout(handle);
    };
  });

  const view = $derived(
    aiHealth(statuses, ai.config.backend, ai.config.allowFallback !== false),
  );

  let { compact = false }: { compact?: boolean } = $props();
</script>

<Hint label={`AI: ${view.detail}`}>
  {#snippet child({ props })}
    <Button
      variant="outline"
      size="sm"
      class="ai-health {view.tone} gap-ui-3 rounded-full text-muted-foreground {compact
        ? 'compact'
        : ''}"
      type="button"
      {...props}
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
  {/snippet}
</Hint>
