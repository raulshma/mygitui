<script lang="ts">
  /**
   * Appearance customizer — the shadcn-svelte.com/create column as a dialog.
   *
   * Every control writes straight into the `appearance` store, which applies
   * live (tokens + style sheet + fonts + menu classes) and persists. Preset
   * codes round-trip with the website's share links via the vendored codec.
   * Swatches resolve against the currently applied scheme so previews match
   * what the user sees.
   *
   * Each option is a pill (see ./AppearancePill.svelte): a <label> wrapping
   * the bare RadioGroup.Item circle — the whole pill toggles, `has-data-`
   * styles the checked one.
   */
  import { onMount } from "svelte";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as RadioGroup from "$lib/components/ui/radio-group/index.js";
  import * as Tooltip from "$lib/components/ui/tooltip/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Separator } from "$lib/components/ui/separator/index.js";
  import { ScrollArea } from "$lib/components/ui/scroll-area/index.js";
  import { toast } from "$lib/toast";
  import { currentScheme, THEME_SCHEME_EVENT, type ColorScheme } from "$lib/theme";
  import { appearance } from "$lib/theme/appearance.svelte";
  import { preloadIconSet } from "$lib/components/icons/library.svelte";
  import {
    ALL_THEMES,
    BASE_COLORS,
    FONT_HEADINGS,
    FONTS,
    ICON_LIBRARIES,
    MENU_ACCENTS,
    MENU_COLORS,
    RADII,
    STYLES,
    getTheme,
    getThemeSwatch,
    getThemesForBaseColor,
  } from "$lib/theme/preset/values";
  import { isPresetCode, type AppearanceConfig } from "$lib/theme/preset/codec";
  import AppearancePill from "./AppearancePill.svelte";

  let { open = $bindable(false) }: { open?: boolean } = $props();

  /** Config mirror so radio groups read a plain object reactively. */
  let config = $derived(appearance.config);

  // Theme / chart pickers constrain to the base color's theme set, exactly
  // like /create ("Match base color" first, then the colored themes).
  const BASE_COLOR_NAMES = new Set<string>(BASE_COLORS.map((b) => b.name));
  let availableThemes = $derived(getThemesForBaseColor(config.baseColor));

  // Applied scheme (drives swatch previews); follows OS/override switches.
  let scheme = $state<ColorScheme>(currentScheme());
  onMount(() => {
    const sync = () => (scheme = currentScheme());
    window.addEventListener(THEME_SCHEME_EVENT, sync);
    return () => window.removeEventListener(THEME_SCHEME_EVENT, sync);
  });

  // Preset code round-trip.
  let codeInput = $state("");
  let codeError = $state(false);

  function copyCode(): void {
    void navigator.clipboard.writeText(appearance.presetCode).then(
      () => toast("Appearance preset copied", { kind: "success" }),
      () => toast("Could not access the clipboard", { kind: "error" }),
    );
  }

  function applyCode(): void {
    const code = codeInput.trim().replace(/^--preset[= ]/, "");
    if (isPresetCode(code) && appearance.applyPresetCode(code)) {
      codeError = false;
      codeInput = "";
      toast("Appearance preset applied", { kind: "success" });
    } else {
      codeError = true;
      toast("That preset code could not be decoded", { kind: "error" });
    }
  }

  /** Chart ramp preview strip for a theme name. */
  function chartStrip(name: string): string[] {
    const theme = getTheme(name);
    if (!theme) return [];
    return [1, 2, 3, 4, 5].map((i) => theme.cssVars[scheme][`chart-${i}`] ?? "transparent");
  }

  // Hover preloads: the heavy pickers are code-split chunks (style sheets,
  // fonts, icon sets), so warming one on pointer enter makes the click apply
  // instantly. All no-ops for already-loaded or static choices.
  const warmStyle = (name: string) => appearance.preloadStyle(name);
  const warmFont = (name: string) => appearance.preloadFont(name);
  const warmIcons = (name: string) => preloadIconSet(name);

  /**
   * RadioGroup events only know strings: narrows the raw value against the
   * picker's option list (an out-of-list value is dropped instead of
   * silently widening the config) and writes that one field.
   */
  function pick<K extends keyof AppearanceConfig>(
    key: K,
    values: ReadonlyArray<AppearanceConfig[K]>,
  ): (value: string) => void {
    return (value) => {
      if ((values as ReadonlyArray<string>).includes(value)) {
        appearance.set({ [key]: value } as Pick<AppearanceConfig, K>);
      }
    };
  }

  const setStyle = pick("style", STYLES.map((s) => s.name));
  const setBaseColor = pick("baseColor", BASE_COLORS.map((b) => b.name));
  const setTheme = pick("theme", ALL_THEMES.map((t) => t.name));
  const setChartColor = pick("chartColor", ALL_THEMES.map((t) => t.name));
  const setFontHeading = pick("fontHeading", FONT_HEADINGS.map((h) => h.name));
  const setFont = pick("font", FONTS.map((f) => f.name));
  const setIconLibrary = pick("iconLibrary", ICON_LIBRARIES.map((l) => l.name));
  const setRadius = pick("radius", RADII.map((r) => r.name));
  const setMenuColor = pick("menuColor", MENU_COLORS.map((m) => m.value));
  const setMenuAccent = pick("menuAccent", MENU_ACCENTS.map((a) => a.value));

  /** Shared class for the color swatch dot in the base/theme pickers. */
  const SWATCH = "size-4 shrink-0 rounded-full border border-border";
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>Appearance</Dialog.Title>
      <Dialog.Description>
        Style, colors, fonts, icons, and menus — mirrors shadcn-svelte.com/create. Changes apply
        live.
      </Dialog.Description>
    </Dialog.Header>

    <ScrollArea class="max-h-[60vh]">
      <div class="flex flex-col gap-5 pr-3">
        <!-- Style -->
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">Style</h3>
          <RadioGroup.Root
            class="grid grid-cols-2 gap-2 sm:grid-cols-4"
            value={config.style}
            onValueChange={setStyle}
          >
            {#each STYLES as style (style.name)}
              <AppearancePill
                value={style.name}
                class="flex-col items-start"
                warm={() => warmStyle(style.name)}
              >
                <span>
                  <span class="block text-sm font-medium">{style.title}</span>
                  <span class="block text-xs text-muted-foreground">{style.description}</span>
                </span>
              </AppearancePill>
            {/each}
          </RadioGroup.Root>
        </section>

        <Separator />

        <!-- Base color -->
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">Base color</h3>
          <RadioGroup.Root
            class="flex flex-wrap gap-2"
            value={config.baseColor}
            onValueChange={setBaseColor}
          >
            {#each BASE_COLORS as base (base.name)}
              <AppearancePill value={base.name}>
                <span class={SWATCH} style="background: {getThemeSwatch(base.name, scheme)}"></span>
                {base.title}
              </AppearancePill>
            {/each}
          </RadioGroup.Root>
        </section>

        <!-- Theme -->
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">Theme</h3>
          <RadioGroup.Root
            class="flex flex-wrap gap-2"
            value={config.theme}
            onValueChange={setTheme}
          >
            {#each availableThemes as theme (theme.name)}
              <AppearancePill value={theme.name}>
                <span class={SWATCH} style="background: {getThemeSwatch(theme.name, scheme)}"></span>
                {theme.name === config.baseColor ? "Match base color" : theme.title}
              </AppearancePill>
            {/each}
          </RadioGroup.Root>
        </section>

        <!-- Chart color -->
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">Chart color</h3>
          <RadioGroup.Root
            class="flex flex-wrap gap-2"
            value={config.chartColor}
            onValueChange={setChartColor}
          >
            {#each availableThemes as theme (theme.name)}
              <AppearancePill value={theme.name}>
                <span class="flex shrink-0 overflow-hidden rounded-full border border-border">
                  {#each chartStrip(theme.name) as color, i (i)}
                    <span class="size-2" style="background: {color}"></span>
                  {/each}
                </span>
                {BASE_COLOR_NAMES.has(theme.name) ? "Match base color" : theme.title}
              </AppearancePill>
            {/each}
          </RadioGroup.Root>
        </section>

        <Separator />

        <!-- Heading font -->
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">Heading</h3>
          <RadioGroup.Root
            class="flex flex-wrap gap-2"
            value={config.fontHeading}
            onValueChange={setFontHeading}
          >
            {#each FONT_HEADINGS as heading (heading.name)}
              <AppearancePill value={heading.name} class="px-3" warm={() => warmFont(heading.name)}>
                {#if heading.family}
                  <span class="text-base leading-none" style="font-family: {heading.family}">
                    Aa
                  </span>
                {/if}
                {heading.title}
              </AppearancePill>
            {/each}
          </RadioGroup.Root>
        </section>

        <!-- Body font -->
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">Font</h3>
          <RadioGroup.Root
            class="flex flex-wrap gap-2"
            value={config.font}
            onValueChange={setFont}
          >
            {#each FONTS as font (font.name)}
              <AppearancePill value={font.name} class="px-3" warm={() => warmFont(font.name)}>
                <span class="text-base leading-none" style="font-family: {font.family}">
                  Aa
                </span>
                {font.title}
              </AppearancePill>
            {/each}
          </RadioGroup.Root>
        </section>

        <Separator />

        <!-- Icon library -->
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">Icon library</h3>
          <RadioGroup.Root
            class="flex flex-wrap gap-2"
            value={config.iconLibrary}
            onValueChange={setIconLibrary}
          >
            {#each ICON_LIBRARIES as lib (lib.name)}
              <AppearancePill value={lib.name} class="px-3" warm={() => warmIcons(lib.name)}>
                {lib.title}
              </AppearancePill>
            {/each}
          </RadioGroup.Root>
          <p class="text-xs text-muted-foreground">
            Interface glyphs switch live; file-type icons stay Material Symbols.
          </p>
        </section>

        <!-- Radius -->
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">Radius</h3>
          <RadioGroup.Root
            class="flex flex-wrap gap-2"
            value={config.radius}
            onValueChange={setRadius}
          >
            {#each RADII as radius (radius.name)}
              <AppearancePill value={radius.name} class="px-3">
                <span
                  class="size-4 shrink-0 border-2 border-current"
                  style="border-radius: {radius.value}"
                ></span>
                {radius.label}
              </AppearancePill>
            {/each}
          </RadioGroup.Root>
        </section>

        <Separator />

        <!-- Menu -->
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">Menu</h3>
          <RadioGroup.Root
            class="flex flex-wrap gap-2"
            value={config.menuColor}
            onValueChange={setMenuColor}
          >
            {#each MENU_COLORS as menu (menu.value)}
              <AppearancePill value={menu.value} class="px-3">
                {menu.label}
              </AppearancePill>
            {/each}
          </RadioGroup.Root>
          <p class="text-xs text-muted-foreground">
            Open a dropdown, context, or select menu to preview.
          </p>
        </section>

        <!-- Menu accent -->
        <section class="flex flex-col gap-2">
          <h3 class="text-sm font-medium">Menu accent</h3>
          <RadioGroup.Root
            class="flex flex-wrap gap-2"
            value={config.menuAccent}
            onValueChange={setMenuAccent}
          >
            {#each MENU_ACCENTS as accent (accent.value)}
              <AppearancePill value={accent.value} class="px-3">
                {accent.label}
              </AppearancePill>
            {/each}
          </RadioGroup.Root>
        </section>
      </div>
    </ScrollArea>

    <Dialog.Footer class="flex-col gap-3 sm:flex-col">
      <!-- Preset code round-trip (shadcn-svelte.com/create compatible) -->
      <div class="flex flex-wrap items-center gap-2">
        <Tooltip.Provider>
          <Tooltip.Root>
            <Tooltip.Trigger aria-label="Copy appearance preset code">
              {#snippet child({ props })}
                <Button {...props} variant="outline" size="sm" onclick={copyCode}>
                  Copy preset
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>
              Paste into “shadcn-svelte init --preset …” or the website’s Open Preset
            </Tooltip.Content>
          </Tooltip.Root>
        </Tooltip.Provider>
        <Input
          class="h-8 w-44 font-mono text-xs {codeError ? 'border-destructive' : ''}"
          placeholder="Preset code…"
          bind:value={codeInput}
          aria-invalid={codeError}
          onkeydown={(e) => e.key === "Enter" && applyCode()}
        />
        <Button variant="secondary" size="sm" onclick={applyCode}>Apply</Button>
        <span class="flex-1"></span>
        <Button variant="outline" size="sm" onclick={() => appearance.shuffle()}>Shuffle</Button>
        <Button
          variant="ghost"
          size="sm"
          onclick={() => {
            appearance.reset();
            toast("Appearance reset to defaults", { kind: "info" });
          }}
        >
          Reset
        </Button>
      </div>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
