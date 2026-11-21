<script lang="ts">
  /**
   * AppBarMenu — the merged top-row's trailing overflow (Workstream A §5.2):
   * one OverflowMenu ("⋯") holding the window-level utilities that used to
   * squat next to the repo tabs — the theme switcher and Settings. The AI
   * health chip + agent toggle stay visible beside it as the row's status
   * cluster (the update chip self-hides while idle and renders next to the
   * cluster when there is update state — it is an attention badge, not a
   * menu command).
   *
   * Theme items mirror `ThemeToggle` (the row's former theme control):
   * the preference lives in `$lib/theme` (persisted under `mygitui.theme`),
   * storage is read authoritatively at mount (initTheme may not have run
   * yet), and `THEME_PREFERENCE_EVENT` keeps the checked item in sync when
   * another surface changes the theme. Unlike the cycling toggle, the menu
   * items go straight to the chosen preference.
   */
  import { onMount } from "svelte";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import OverflowMenu from "$lib/components/layout/OverflowMenu.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
  import iconSettings from "@ktibow/iconset-material-symbols/settings";
  import {
    getThemePreference,
    setThemePreference,
    THEME_PREFERENCE_EVENT,
    THEME_STORAGE_KEY,
    type ThemePreference,
  } from "$lib/theme";

  let { onOpenSettings }: { onOpenSettings: () => void } = $props();

  const ORDER: readonly ThemePreference[] = ["system", "light", "dark"];

  const LABELS: Record<ThemePreference, string> = {
    system: "System theme",
    light: "Light theme",
    dark: "Dark theme",
  };

  const ICONS = $derived({
    system: appIcon("brightness-auto")!,
    light: appIcon("sunny")!,
    dark: appIcon("bedtime")!,
  });

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

  onMount(() => {
    const sync = () => (preference = readPreference());
    window.addEventListener(THEME_PREFERENCE_EVENT, sync);
    // initTheme() loads the persisted value asynchronously at startup.
    queueMicrotask(sync);
    return () => window.removeEventListener(THEME_PREFERENCE_EVENT, sync);
  });
</script>

<OverflowMenu label="More app options">
  {#snippet content()}
    <DropdownMenu.RadioGroup
      value={preference}
      onValueChange={(value) => {
        setThemePreference(value as ThemePreference);
        preference = value as ThemePreference;
      }}
    >
      {#each ORDER as option (option)}
        <DropdownMenu.RadioItem
          class="cursor-pointer gap-ui-3"
          value={option}
          closeOnSelect={false}
        >
          <span class="pointer-events-none inline-flex flex-none items-center justify-center">
            <Icon icon={ICONS[option]} size={16} />
          </span>
          {LABELS[option]}
        </DropdownMenu.RadioItem>
      {/each}
    </DropdownMenu.RadioGroup>
    <DropdownMenu.Separator />
    <DropdownMenu.Item class="cursor-pointer gap-ui-3" onclick={onOpenSettings}>
      <span class="pointer-events-none inline-flex flex-none items-center justify-center">
        <Icon icon={iconSettings} size={16} />
      </span>
      Settings
    </DropdownMenu.Item>
  {/snippet}
</OverflowMenu>
