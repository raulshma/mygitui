<script lang="ts">
  /**
   * Header toggle for the Overview: the standalone multi-repo dashboard
   * that replaced Agent mode (and the old recents home). Mirrors
   * ThemeToggle's pattern (ghost icon button + Hint) and persists through
   * the `overviewStore` (`mygitui.overview`).
   */
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Tooltip from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import { overviewStore } from "./overviewStore.svelte";

  const icon = $derived(appIcon("home-rounded")!);

  const label = $derived(
    overviewStore.enabled
      ? "Exit the Overview — back to the workspace"
      : "Overview — all repositories at a glance",
  );
</script>

<Tooltip.Provider>
  <Tooltip.Root>
    <Tooltip.Trigger aria-label={label}>
      {#snippet child({ props })}
        <Button
          {...props}
          class="overview-toggle{overviewStore.enabled ? ' text-primary-ink' : ''}"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-pressed={overviewStore.enabled}
          onclick={() => overviewStore.toggle()}
        >
          <Icon {icon} size={16} />
        </Button>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Content>{label}</Tooltip.Content>
  </Tooltip.Root>
</Tooltip.Provider>
