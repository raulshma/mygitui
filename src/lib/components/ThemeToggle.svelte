<script lang="ts">
  /**
   * Compact theme control: cycles system → light → dark.
   *
   * The preference lives in `$lib/theme` (persisted under `mygitui.theme`);
   * the palette's theme commands use the same API. `THEME_PREFERENCE_EVENT`
   * keeps every mounted toggle in sync when another surface changes it.
   */
  import { onMount } from "svelte";
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

<button
  class="theme-toggle"
  type="button"
  title={`${LABELS[preference]} — click for ${LABELS[next].toLowerCase()}`}
  aria-label={`${LABELS[preference]}. Switch to ${LABELS[next].toLowerCase()}`}
  onclick={cycle}
>
  <Icon icon={ICONS[preference]} size={16} />
</button>

<style>
  .theme-toggle {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.65rem;
    height: 1.65rem;
    padding: 0;
    border: 1px solid transparent;
    border-radius: var(--m3-shape-extra-small, 4px);
    background: none;
    color: var(--m3-on-surface-variant);
    cursor: pointer;
    transition:
      background-color 0.12s ease,
      color 0.12s ease;
  }

  .theme-toggle:hover {
    background: var(--m3-surface-container-high, var(--m3-surface));
    color: var(--m3-on-surface);
  }

  .theme-toggle:focus-visible {
    outline: 2px solid var(--m3-primary);
    outline-offset: -1px;
  }
</style>
