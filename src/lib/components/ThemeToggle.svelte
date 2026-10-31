<script lang="ts">
  /**
   * Compact theme control: cycles system → light → dark.
   *
   * The preference lives in `$lib/theme` (persisted under `mygitui.theme`);
   * the palette's theme commands use the same API. `THEME_PREFERENCE_EVENT`
   * keeps every mounted toggle in sync when another surface changes it.
   */
  import { onMount } from "svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Tooltip from "$lib/components/ui/tooltip/index.js";
  import Icon from "$lib/components/icons/Icon.svelte";
  import {
    getThemePreference,
    setThemePreference,
    THEME_PREFERENCE_EVENT,
    THEME_STORAGE_KEY,
    type ThemePreference,
  } from "$lib/theme";
  import iconSystem from "@ktibow/iconset-material-symbols/brightness-auto";
  import iconLight from "@ktibow/iconset-material-symbols/sunny";
  import iconDark from "@ktibow/iconset-material-symbols/bedtime";

  const ORDER: readonly ThemePreference[] = ["system", "light", "dark"];

  const ICONS = {
    system: iconSystem,
    light: iconLight,
    dark: iconDark,
  } as const;

  const LABELS: Record<ThemePreference, string> = {
    system: "System theme",
    light: "Light theme",
    dark: "Dark theme",
  };

  /** Storage is authoritative; initTheme may not have run yet at mount. */
  function readPreference(): ThemePreference {
    try {
      const raw = localStorage.getItem(THEME_STORAGE_KEY);
      if (raw === "light" || raw === "dark") return raw;
    } catch {
      // Fall through to the in-memory value.
    }
    return getThemePreference();
  }

  let preference = $state<ThemePreference>(readPreference());

  const next = $derived.by<ThemePreference>(() => {
    const index = ORDER.indexOf(preference);
    return ORDER[(index + 1) % ORDER.length] ?? "system";
  });

  onMount(() => {
    const sync = () => (preference = readPreference());
    window.addEventListener(THEME_PREFERENCE_EVENT, sync);
    // initTheme() loads the persisted value asynchronously at startup.
    queueMicrotask(sync);
    return () => window.removeEventListener(THEME_PREFERENCE_EVENT, sync);
  });

  function cycle(): void {
    const target = next;
    setThemePreference(target);
    preference = target;
  }
</script>

<Tooltip.Provider>
  <Tooltip.Root>
    <Tooltip.Trigger aria-label={`${LABELS[preference]}. Switch to ${LABELS[next].toLowerCase()}`}>
      {#snippet child({ props })}
        <Button
          {...props}
          class="theme-toggle"
          variant="ghost"
          size="icon-sm"
          type="button"
          onclick={cycle}
        >
          <Icon icon={ICONS[preference]} size={16} />
        </Button>
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Content>
      {LABELS[preference]} — click for {LABELS[next].toLowerCase()}
    </Tooltip.Content>
  </Tooltip.Root>
</Tooltip.Provider>
