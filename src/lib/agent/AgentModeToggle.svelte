<script lang="ts">
  /**
   * Header toggle for Agent mode: the super-minimal git-focused view.
   * Mirrors ThemeToggle's pattern (ghost icon button + Hint) and persists
   * through the `agentMode` store (`mygitui.agent`).
   */
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Tooltip from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { agentMode } from "./agentMode.svelte";

  const icon = $derived(appIcon("agent") ?? appIcon("bolt")!);

  const label = $derived(
    agentMode.enabled
      ? "Exit Agent mode — back to the full workspace"
      : "Agent mode — minimal git focus",
  );
</script>

<Tooltip.Provider>
  <Tooltip.Root>
    <Tooltip.Trigger aria-label={label}>
      {#snippet child({ props })}
        <Button
          {...props}
          class="agent-mode-toggle{agentMode.enabled ? ' text-primary-ink' : ''}"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-pressed={agentMode.enabled}
          onclick={() => agentMode.toggle()}
        >
          <Icon {icon} size={16} />
        </Button>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Content>{label}</Tooltip.Content>
  </Tooltip.Root>
</Tooltip.Provider>
