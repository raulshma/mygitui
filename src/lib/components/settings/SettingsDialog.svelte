<script lang="ts">
  /**
   * Settings dialog — the single hub for every app-wide preference.
   *
   * Today's knobs live in feature stores (theme, appearance customizer, AI
   * config, updater schedule, autofetch cadence, diff/editor/file-tree prefs,
   * terminal theme, open-with apps, keybind overrides) plus the global
   * `appSettings` switches. This dialog wires them all through their OWNING
   * stores — nothing is duplicated or re-persisted here — with a left nav
   * (General / Appearance / Git / Editor & Diffs / Terminal / Open With /
   * AI / Updates / Advanced) and a scrollable content pane. Every control
   * writes immediately (no Save step).
   *
   * Related dialogs are opened via the typed UI bus (`open-appearance`,
   * `open-ai-settings`): App.svelte owns those overlays, and the command
   * palette exposes the same targets, so the dialog only links.
   *
   * The Advanced section is the escape hatch: a read-only inspector of the
   * `mygitui.*` localStorage keys with copy-diagnostics, a keybind reset,
   * and a confirmed danger reset that clears those keys (the global
   * settings re-default immediately; the other stores re-read on next
   * launch).
   *
   * Stores arrive as props so tests can inject instances (the
   * CustomAppsSection pattern); production uses the singletons.
   */
  import { onMount, untrack } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import * as Collapsible from "$lib/components/ui/collapsible/index.js";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import CustomAppsSection from "$lib/components/openWith/CustomAppsSection.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { toast } from "$lib/toast";
  import {
    appSettings,
    APP_SETTINGS_DEFAULTS,
    AppSettingsStore,
    AUTOFETCH_MINUTES_MAX,
    TERMINAL_FONT_MAX,
    TERMINAL_FONT_MIN,
  } from "$lib/state/appSettings.svelte";
  import {
    getThemePreference,
    setThemePreference,
    THEME_PREFERENCE_EVENT,
    type ThemePreference,
  } from "$lib/theme";
  import { isTauri } from "$lib/entry/dragdrop";
  import { updater } from "$lib/stores/updater.svelte";
  import { autofetch } from "$lib/stores/autofetch.svelte";
  import { customApps, type CustomAppsStore } from "$lib/openwith/customApps.svelte";
  import {
    terminalThemePref,
    type TerminalThemePreference,
  } from "$lib/terminal/terminalThemes.svelte";
  import { readDiffModePref, writeDiffModePref } from "$lib/diff/modePref";
  import { readWrapPref, writeWrapPref } from "$lib/diff/wrapPref";
  import { editorPrefs, type MdViewMode } from "$lib/components/files/editorPrefs.svelte";
  import { fileTreePrefs } from "$lib/components/panels/fileTreePrefs.svelte";
  import { fileIconPreferences } from "$lib/components/icons/fileIconPreferences.svelte";
  import { resetBindings } from "$lib/palette/keybinds";
  import { emitUiEvent } from "$lib/palette/events";
  import { openAboutDialog } from "$lib/components/aboutDialog";
  import { ai } from "$lib/ai/ai.svelte";
  import { formatDateTime } from "$lib/git/format";
  import iconChevronDown from "@ktibow/iconset-material-symbols/expand-more";

  /** Fallback when no Tauri runtime answers (browser/e2e); = package.json. */
  const FALLBACK_VERSION = "0.1.21";

  /** localStorage prefix the Advanced inspector / reset scope themselves to. */
  const APP_KEY_PREFIX = "mygitui.";

  /** Left-nav entries, in display order. */
  const SECTIONS = [
    { id: "general", label: "General" },
    { id: "appearance", label: "Appearance" },
    { id: "git", label: "Git" },
    { id: "editor", label: "Editor & Diffs" },
    { id: "terminal", label: "Terminal" },
    { id: "openwith", label: "Open With" },
    { id: "ai", label: "AI" },
    { id: "updates", label: "Updates" },
    { id: "advanced", label: "Advanced" },
  ] as const;

  type SectionId = (typeof SECTIONS)[number]["id"];

  let {
    open = $bindable(false),
    settings = appSettings,
    apps = customApps,
  }: {
    open?: boolean;
    /** Global-settings store (injectable for tests; default: the singleton). */
    settings?: AppSettingsStore;
    /** Custom open-with apps store (injectable for tests; default: the singleton). */
    apps?: CustomAppsStore;
  } = $props();

  /** Active left-nav section. */
  let active = $state<SectionId>("general");

  // --- General: theme + about -------------------------------------------------

  /** Mirror of the theme preference (the theme module notifies via event). */
  let themeChoice = $state<ThemePreference>(getThemePreference());
  onMount(() => {
    const sync = () => (themeChoice = getThemePreference());
    window.addEventListener(THEME_PREFERENCE_EVENT, sync);
    return () => window.removeEventListener(THEME_PREFERENCE_EVENT, sync);
  });

  function onThemeChange(event: Event): void {
    const value = (event.currentTarget as HTMLSelectElement).value as ThemePreference;
    themeChoice = value;
    setThemePreference(value);
  }

  function onTerminalThemeChange(event: Event): void {
    const value = (event.currentTarget as HTMLSelectElement).value as TerminalThemePreference;
    terminalThemePref.set(value);
  }

  function onFileIconThemeChange(event: Event): void {
    const value = (event.currentTarget as HTMLSelectElement).value;
    fileIconPreferences.setTheme(
      value === "material" || value === "minimal" ? value : "colorful",
    );
  }

  function onMdViewChange(event: Event): void {
    const value = (event.currentTarget as HTMLSelectElement).value;
    editorPrefs.setMdView(value === "edit" || value === "preview" ? value : "split");
  }

  function onDiffModeChange(event: Event): void {
    const value = (event.currentTarget as HTMLSelectElement).value;
    diffMode = value === "unified" ? "unified" : "split";
    writeDiffModePref(diffMode);
  }

  /** App version (Tauri runtime; fallback constant in plain-browser runs). */
  let version = $state(FALLBACK_VERSION);
  onMount(() => {
    if (!isTauri()) return;
    void getVersion()
      .then((v) => {
        if (typeof v === "string" && v !== "") version = v;
      })
      .catch(() => {
        // Runtime refused/could not answer — keep the fallback constant.
      });
  });

  // --- Git: default auto-fetch cadence + per-repo overrides -------------------

  /** Per-repo cadence overrides, sorted for stable rendering. */
  const autofetchOverrides = $derived(
    Object.entries(autofetch.config).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)),
  );

  function onAutofetchMinutes(event: Event): void {
    const raw = Number((event.currentTarget as HTMLInputElement).value);
    // parseAppSettings clamps + rounds; `0` = off.
    settings.update({ defaultAutofetchMinutes: Number.isFinite(raw) ? raw : 0 });
  }

  function onTerminalFont(event: Event): void {
    const raw = Number((event.currentTarget as HTMLInputElement).value);
    // parseAppSettings clamps to TERMINAL_FONT_MIN/MAX and rounds.
    settings.update({
      terminalFontSize: Number.isFinite(raw) ? raw : APP_SETTINGS_DEFAULTS.terminalFontSize,
    });
  }

  // --- Editor & Diffs: localStorage-backed prefs (mirrored on open) -----------

  /** Diff layout mirror — the pref module is localStorage, not reactive. */
  let diffMode = $state<"split" | "unified">(readDiffModePref());
  /** Diff wrap mirror (default ON, matching readWrapPref). */
  let diffWrap = $state(readWrapPref());

  // Re-read the non-reactive prefs whenever the dialog opens (they may have
  // been flipped by the diff toolbar / palette since the last visit).
  $effect(() => {
    if (!open) return;
    untrack(() => {
      diffMode = readDiffModePref();
      diffWrap = readWrapPref();
      themeChoice = getThemePreference();
    });
  });

  // --- AI ---------------------------------------------------------------------

  const aiBackendLabel = $derived(
    ai.config.backend === "openrouter" ? "OpenRouter (direct API)" : "OpenCode (local server)",
  );

  // --- Updates ----------------------------------------------------------------

  const updateStatusLine = $derived.by(() => {
    switch (updater.status) {
      case "checking":
        return "Checking for updates…";
      case "uptodate":
        return "You're on the latest version";
      case "available":
        return updater.info !== null
          ? `Update to v${updater.info.version} available`
          : "Update available";
      case "downloading": {
        const percent = updater.percent;
        return percent === null
          ? "Downloading update…"
          : `Downloading update — ${percent}%`;
      }
      case "ready":
        return updater.info !== null
          ? `Update to v${updater.info.version} ready — restart to apply`
          : "Update ready — restart to apply";
      case "error":
        return `Update failed: ${updater.error ?? "unknown error"}`;
      default:
        return "Never checked";
    }
  });

  const lastCheckLine = $derived(
    updater.lastCheck === null
      ? "Never"
      : formatDateTime(Math.floor(updater.lastCheck / 1000)),
  );

  /** True while a poll or download is in flight (Check button disabled). */
  const updateBusy = $derived(
    updater.status === "checking" || updater.status === "downloading",
  );

  // --- Advanced: inspector + diagnostics + danger reset -----------------------

  /** One `mygitui.*` localStorage row (raw stored string). */
  interface SettingsEntry {
    key: string;
    value: string;
  }

  /** Inspector rows, refreshed when the section opens or on demand. */
  let entries = $state<SettingsEntry[]>([]);

  /** Re-scans localStorage for `mygitui.*` keys (never throws). */
  function rescan(): void {
    const out: SettingsEntry[] = [];
    try {
      const storage = globalThis.localStorage;
      if (storage) {
        for (let i = 0; i < storage.length; i++) {
          const key = storage.key(i);
          if (key === null || !key.startsWith(APP_KEY_PREFIX)) continue;
          out.push({ key, value: storage.getItem(key) ?? "" });
        }
      }
    } catch {
      // Storage unavailable (privacy modes); the inspector stays empty.
    }
    out.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
    entries = out;
  }

  // The inspector reflects storage on section entry (localStorage is not
  // reactive); the Refresh button re-runs it mid-visit.
  $effect(() => {
    if (!open) return;
    if (active === "advanced") rescan();
  });

  /** Pretty-prints an inspector row's stored JSON when parseable. */
  function prettyValue(value: string): string {
    try {
      return JSON.stringify(JSON.parse(value), null, 2);
    } catch {
      return value;
    }
  }

  /** Copies every `mygitui.*` key/value as JSON to the clipboard. */
  async function copyDiagnostics(): Promise<void> {
    const payload: Record<string, unknown> = {};
    for (const entry of entries) {
      try {
        payload[entry.key] = JSON.parse(entry.value);
      } catch {
        payload[entry.key] = entry.value;
      }
    }
    try {
      await navigator.clipboard.writeText(JSON.stringify(payload, null, 2));
      toast("App settings copied as JSON", { kind: "success" });
    } catch {
      toast("Could not access the clipboard", { kind: "error" });
    }
  }

  /** Resets every keybind override (defaults restored). */
  function resetKeybinds(): void {
    resetBindings();
    toast("Keyboard shortcuts reset to defaults", { kind: "success" });
  }

  /** Confirm gate for the danger reset. */
  let resetOpen = $state(false);

  /**
   * Danger reset: clears every `mygitui.*` localStorage key, then re-defaults
   * the global settings (which re-persist their envelope). The other stores
   * keep their in-memory state for this session and re-read their defaults
   * on the next launch — the confirm copy says exactly that.
   */
  function resetAllSettings(): void {
    try {
      const storage = globalThis.localStorage;
      if (storage) {
        const doomed: string[] = [];
        for (let i = 0; i < storage.length; i++) {
          const key = storage.key(i);
          if (key !== null && key.startsWith(APP_KEY_PREFIX)) doomed.push(key);
        }
        for (const key of doomed) storage.removeItem(key);
      }
    } catch {
      // Storage unavailable: nothing persisted to clear.
    }
    settings.reset();
    rescan();
    toast("App settings reset to defaults", { kind: "info" });
  }

  function close(): void {
    // `open` is bindable and the app shell passes no onclose — clearing it
    // here is what actually dismisses the dialog (Close button / Escape).
    open = false;
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  }
</script>

{#if open}
  <Dialog.Root bind:open>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <Dialog.Overlay class="scrim bg-black/40" role="presentation" />
    <Dialog.Content
      class="dialog flex h-[min(34rem,calc(100dvh_-_2rem))] w-[min(46rem,calc(100vw_-_2rem))] max-w-none flex-col overflow-hidden text-popover-foreground sm:max-w-none"
      role="dialog"
      aria-modal="true"
      aria-labelledby="settings-title"
      aria-describedby="settings-description"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={onKeydown}
    >
      <header class="dialog-header mb-0 flex flex-none items-start justify-between gap-4 border-b border-border px-5 pb-3 pt-4">
        <div>
          <Dialog.Title id="settings-title" class="title m-0 font-semibold text-foreground">Settings</Dialog.Title>
          <p id="settings-description" class="subtitle mb-0 mt-1 text-ui-base text-muted-foreground">
            Every app-wide preference in one place. Changes apply immediately.
          </p>
        </div>
        <Button variant="ghost" size="icon" class="icon-button flex-none rounded-full text-lg leading-none text-muted-foreground" type="button" aria-label="Close settings" onclick={close}>
          ×
        </Button>
      </header>

      <div class="dialog-body flex min-h-0 flex-1">
        <!-- Sticky section nav -->
        <nav
          class="sections-nav flex w-40 flex-none flex-col gap-0.5 overflow-y-auto border-r border-border bg-muted/30 px-2 py-3"
          aria-label="Settings sections"
        >
          {#each SECTIONS as section (section.id)}
            <button
              class="nav-item flex w-full cursor-pointer items-center rounded-md px-2.5 py-1.5 text-left text-sm {active === section.id
                ? 'bg-accent font-medium text-accent-foreground'
                : 'text-muted-foreground hover:bg-muted hover:text-foreground'} focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
              type="button"
              aria-current={active === section.id ? "true" : undefined}
              onclick={() => (active = section.id)}
            >
              {section.label}
            </button>
          {/each}
        </nav>

        <!-- Content pane (the only scroller) -->
        <div class="dialog-content-pane min-h-0 flex-1 overflow-y-auto px-5 py-4">
          {#if active === "general"}
            <section aria-labelledby="settings-general-heading" class="flex flex-col gap-4">
              <h3 id="settings-general-heading" class="m-0 text-sm font-semibold text-foreground">General</h3>
              <div class="field flex flex-col gap-1">
                <label class="flex flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Theme</span>
                  <select
                    class="input w-48 rounded border border-border bg-background px-2 py-1.5 text-ui-base text-foreground focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary"
                    value={themeChoice}
                    onchange={onThemeChange}
                  >
                    <option value="system">Match system</option>
                    <option value="light">Light</option>
                    <option value="dark">Dark</option>
                  </select>
                </label>
                <span class="text-ui-meta text-muted-foreground">
                  “Match system” follows the OS and switches live; a fixed choice overrides it.
                </span>
              </div>
              <div class="about-row flex items-center justify-between gap-4 rounded-xl border border-border bg-muted/30 px-4 py-3">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span class="text-sm font-medium text-foreground">mygitui</span>
                  <span class="text-ui-meta text-muted-foreground">Version {version} · Fast, configurable Git GUI</span>
                </span>
                <Button variant="outline" class="flex-none rounded-full" type="button" onclick={() => openAboutDialog()}>
                  About
                </Button>
              </div>
              <p class="hint m-0 text-ui-meta text-muted-foreground">
                The interface language is English today; more languages are planned.
              </p>
            </section>
          {:else if active === "appearance"}
            <section aria-labelledby="settings-appearance-heading" class="flex flex-col gap-4">
              <h3 id="settings-appearance-heading" class="m-0 text-sm font-semibold text-foreground">Appearance</h3>
              <div class="flex items-center justify-between gap-4 rounded-xl border border-border bg-muted/30 px-4 py-3">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span class="text-sm font-medium text-foreground">Customize appearance</span>
                  <span class="text-ui-meta text-muted-foreground">
                    Style, base color, theme, chart colors, fonts, icon set, radius and menu
                    accents — changes apply live.
                  </span>
                </span>
                <Button
                  variant="outline"
                  class="flex-none rounded-full"
                  type="button"
                  onclick={() => emitUiEvent("open-appearance")}
                >
                  Customize…
                </Button>
              </div>
              <div class="field flex flex-col gap-1">
                <label class="flex flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">File icons</span>
                  <select
                    class="input w-48 rounded border border-border bg-background px-2 py-1.5 text-ui-base text-foreground focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary"
                    value={fileIconPreferences.theme}
                    onchange={onFileIconThemeChange}
                  >
                    <option value="colorful">Colorful</option>
                    <option value="material">Material</option>
                    <option value="minimal">Minimal</option>
                  </select>
                </label>
                <span class="text-ui-meta text-muted-foreground">
                  File-type glyphs in the file trees and the diff header.
                </span>
              </div>
            </section>
          {:else if active === "git"}
            <section aria-labelledby="settings-git-heading" class="flex flex-col gap-4">
              <h3 id="settings-git-heading" class="m-0 text-sm font-semibold text-foreground">Git</h3>
              <div class="row flex items-start justify-between gap-4">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span id="settings-checkpoints-label" class="text-sm font-medium text-foreground">
                    Checkpoints for git changes
                  </span>
                  <span class="text-ui-meta text-muted-foreground">
                    Snapshot a restorable undo point before working-copy discards and cleans —
                    restore them from the Undo panel.
                  </span>
                </span>
                <Switch
                  class="mt-1 flex-none"
                  aria-labelledby="settings-checkpoints-label"
                  checked={settings.checkpointsEnabled}
                  onCheckedChange={(checked) => settings.update({ checkpointsEnabled: checked })}
                />
              </div>
              <div class="row flex items-start justify-between gap-4">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span id="settings-confirm-discard-label" class="text-sm font-medium text-foreground">
                    Confirm before discard
                  </span>
                  <span class="text-ui-meta text-muted-foreground">
                    Ask for confirmation before discarding working-copy changes. Turn off if the
                    prompt feels noisy — discards then apply immediately.
                  </span>
                </span>
                <Switch
                  class="mt-1 flex-none"
                  aria-labelledby="settings-confirm-discard-label"
                  checked={settings.confirmDiscard}
                  onCheckedChange={(checked) => settings.update({ confirmDiscard: checked })}
                />
              </div>
              <div class="field flex max-w-56 flex-col gap-1">
                <label class="flex flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Auto-fetch every (minutes)</span>
                  <Input
                    class="input rounded"
                    type="number"
                    min={0}
                    max={AUTOFETCH_MINUTES_MAX}
                    step={1}
                    aria-label="Default auto-fetch interval in minutes"
                    value={settings.defaultAutofetchMinutes}
                    onchange={onAutofetchMinutes}
                  />
                </label>
                <span class="text-ui-meta text-muted-foreground">
                  Default cadence for repositories without their own override; 0 disables
                  auto-fetch. Per-repo overrides are set from the Branches panel.
                </span>
              </div>
              <div class="overrides flex flex-col gap-1">
                <span class="text-sm font-medium text-foreground">Per-repo auto-fetch overrides</span>
                {#if autofetchOverrides.length === 0}
                  <p class="m-0 text-ui-meta text-muted-foreground">
                    No overrides — every open repository follows the default cadence.
                  </p>
                {:else}
                  <ul class="m-0 flex list-none flex-col gap-1 p-0" role="list" aria-label="Per-repo auto-fetch overrides">
                    {#each autofetchOverrides as [root, minutes] (root)}
                      <li class="flex min-w-0 items-center justify-between gap-3 rounded-md border border-border px-2.5 py-1.5">
                        <span class="min-w-0 truncate font-mono text-ui-meta text-foreground" title={root}>{root}</span>
                        <span class="flex flex-none items-center gap-2">
                          <span class="text-ui-meta text-muted-foreground">{minutes} min</span>
                          <Hint label={`Clear the auto-fetch override for ${root}`}>
                            {#snippet child({ props })}
                              <Button
                                {...props}
                                variant="ghost"
                                size="icon-xs"
                                type="button"
                                aria-label={`Clear auto-fetch override for ${root}`}
                                onclick={() => autofetch.setInterval(root, 0)}
                              >
                                ×
                              </Button>
                            {/snippet}
                          </Hint>
                        </span>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </div>
            </section>
          {:else if active === "editor"}
            <section aria-labelledby="settings-editor-heading" class="flex flex-col gap-4">
              <h3 id="settings-editor-heading" class="m-0 text-sm font-semibold text-foreground">Editor &amp; Diffs</h3>
              <div class="field flex flex-col gap-1">
                <label class="flex flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Diff layout</span>
                  <select
                    class="input w-48 rounded border border-border bg-background px-2 py-1.5 text-ui-base text-foreground focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary"
                    value={diffMode}
                    onchange={onDiffModeChange}
                  >
                    <option value="split">Split (side by side)</option>
                    <option value="unified">Unified (inline)</option>
                  </select>
                </label>
                <span class="text-ui-meta text-muted-foreground">
                  Applies to diffs opened afterwards; the diff toolbar flips the current one.
                </span>
              </div>
              <div class="row flex items-start justify-between gap-4">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span id="settings-diff-wrap-label" class="text-sm font-medium text-foreground">
                    Wrap long diff lines
                  </span>
                  <span class="text-ui-meta text-muted-foreground">
                    Off = horizontal scrolling per diff, like the toolbar's wrap toggle.
                  </span>
                </span>
                <Switch
                  class="mt-1 flex-none"
                  aria-labelledby="settings-diff-wrap-label"
                  checked={diffWrap}
                  onCheckedChange={(checked) => {
                    diffWrap = checked;
                    writeWrapPref(checked);
                  }}
                />
              </div>
              <div class="row flex items-start justify-between gap-4">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span id="settings-editor-wrap-label" class="text-sm font-medium text-foreground">
                    Wrap lines in the editor
                  </span>
                  <span class="text-ui-meta text-muted-foreground">
                    Soft-wrap for files opened in the file workspace.
                  </span>
                </span>
                <Switch
                  class="mt-1 flex-none"
                  aria-labelledby="settings-editor-wrap-label"
                  checked={editorPrefs.wrap}
                  onCheckedChange={(checked) => editorPrefs.setWrap(checked)}
                />
              </div>
              <div class="field flex flex-col gap-1">
                <label class="flex flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Markdown default view</span>
                  <select
                    class="input w-48 rounded border border-border bg-background px-2 py-1.5 text-ui-base text-foreground focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary"
                    value={editorPrefs.mdView}
                    onchange={onMdViewChange}
                  >
                    <option value="edit">Edit</option>
                    <option value="split">Split</option>
                    <option value="preview">Preview</option>
                  </select>
                </label>
                <span class="text-ui-meta text-muted-foreground">
                  How markdown files open in the editor (Ctrl+Shift+V cycles live).
                </span>
              </div>
              <div class="row flex items-start justify-between gap-4">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span id="settings-show-ignored-label" class="text-sm font-medium text-foreground">
                    Show ignored files
                  </span>
                  <span class="text-ui-meta text-muted-foreground">
                    List gitignored paths in the Files panel tree.
                  </span>
                </span>
                <Switch
                  class="mt-1 flex-none"
                  aria-labelledby="settings-show-ignored-label"
                  checked={fileTreePrefs.showIgnored}
                  onCheckedChange={(checked) => fileTreePrefs.setShowIgnored(checked)}
                />
              </div>
            </section>
          {:else if active === "terminal"}
            <section aria-labelledby="settings-terminal-heading" class="flex flex-col gap-4">
              <h3 id="settings-terminal-heading" class="m-0 text-sm font-semibold text-foreground">Terminal</h3>
              <div class="field flex flex-col gap-1">
                <label class="flex flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Terminal theme</span>
                  <select
                    class="input w-48 rounded border border-border bg-background px-2 py-1.5 text-ui-base text-foreground focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary"
                    value={terminalThemePref.preference}
                    onchange={onTerminalThemeChange}
                  >
                    <option value="system">Match app theme</option>
                    <option value="light">Light</option>
                    <option value="dark">Dark</option>
                  </select>
                </label>
                <span class="text-ui-meta text-muted-foreground">
                  “Match app theme” derives colors from the scheme live; a fixed choice pins an
                  independent palette. Applies to running sessions immediately.
                </span>
              </div>
              <div class="field flex max-w-56 flex-col gap-1">
                <label class="flex flex-col gap-1">
                  <span class="text-sm font-medium text-foreground">Font size (px)</span>
                  <Input
                    class="input rounded"
                    type="number"
                    min={TERMINAL_FONT_MIN}
                    max={TERMINAL_FONT_MAX}
                    step={1}
                    aria-label="Terminal font size in pixels"
                    value={settings.terminalFontSize}
                    onchange={onTerminalFont}
                  />
                </label>
                <span class="text-ui-meta text-muted-foreground">
                  Saved with the app settings — applies to new terminal sessions (use the
                  panel's Restart to pick it up); running sessions keep their current size.
                </span>
              </div>
            </section>
          {:else if active === "openwith"}
            <section aria-labelledby="settings-openwith-heading" class="flex flex-col gap-4">
              <h3 id="settings-openwith-heading" class="m-0 text-sm font-semibold text-foreground">Open With</h3>
              <p class="m-0 text-ui-meta text-muted-foreground">
                Detected editors come from this machine; custom apps below join them in the
                header's “Open with” menu. Nothing is synced.
              </p>
              <CustomAppsSection store={apps} />
            </section>
          {:else if active === "ai"}
            <section aria-labelledby="settings-ai-heading" class="flex flex-col gap-4">
              <h3 id="settings-ai-heading" class="m-0 text-sm font-semibold text-foreground">AI</h3>
              <div class="flex items-center justify-between gap-4 rounded-xl border border-border bg-muted/30 px-4 py-3">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span class="text-sm font-medium text-foreground">AI provider</span>
                  <span class="text-ui-meta text-muted-foreground">
                    Preferred backend: {aiBackendLabel}. Backends, models, connection checks and
                    the per-repo feature opt-ins live in the AI settings dialog.
                  </span>
                </span>
                <Button
                  variant="outline"
                  class="flex-none rounded-full"
                  type="button"
                  onclick={() => emitUiEvent("open-ai-settings")}
                >
                  AI settings…
                </Button>
              </div>
            </section>
          {:else if active === "updates"}
            <section aria-labelledby="settings-updates-heading" class="flex flex-col gap-4">
              <h3 id="settings-updates-heading" class="m-0 text-sm font-semibold text-foreground">Updates</h3>
              <div class="row flex items-start justify-between gap-4">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span id="settings-auto-check-label" class="text-sm font-medium text-foreground">
                    Check for updates automatically
                  </span>
                  <span class="text-ui-meta text-muted-foreground">
                    Polls at most every 12 hours after launch.
                  </span>
                </span>
                <Switch
                  class="mt-1 flex-none"
                  aria-labelledby="settings-auto-check-label"
                  checked={updater.autoCheck}
                  onCheckedChange={(checked) => updater.setAutoCheck(checked)}
                />
              </div>
              <div class="row flex items-start justify-between gap-4">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span id="settings-auto-download-label" class="text-sm font-medium text-foreground">
                    Download updates automatically
                  </span>
                  <span class="text-ui-meta text-muted-foreground">
                    Starts the download as soon as an update is found; installing always waits
                    for your restart click.
                  </span>
                </span>
                <Switch
                  class="mt-1 flex-none"
                  aria-labelledby="settings-auto-download-label"
                  checked={updater.autoDownload}
                  onCheckedChange={(checked) => updater.setAutoDownload(checked)}
                />
              </div>
              <div class="status-row flex flex-wrap items-center justify-between gap-3 rounded-xl border border-border bg-muted/30 px-4 py-3">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span class="text-sm font-medium text-foreground" role="status">{updateStatusLine}</span>
                  <span class="text-ui-meta text-muted-foreground">Last check: {lastCheckLine}</span>
                </span>
                <Button
                  variant="outline"
                  class="flex-none rounded-full"
                  type="button"
                  disabled={updateBusy}
                  onclick={() => void updater.checkNow({ manual: true })}
                >
                  {updateBusy ? "Checking…" : "Check for updates"}
                </Button>
              </div>
            </section>
          {:else if active === "advanced"}
            <section aria-labelledby="settings-advanced-heading" class="flex flex-col gap-4">
              <h3 id="settings-advanced-heading" class="m-0 text-sm font-semibold text-foreground">Advanced</h3>

              <div class="keybinds-row flex items-center justify-between gap-4 rounded-xl border border-border bg-muted/30 px-4 py-3">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span class="text-sm font-medium text-foreground">Keyboard shortcuts</span>
                  <span class="text-ui-meta text-muted-foreground">
                    Rebind commands from the command palette (Ctrl/Cmd+Shift+P); overrides are
                    remembered per machine.
                  </span>
                </span>
                <Button variant="outline" class="flex-none rounded-full" type="button" onclick={resetKeybinds}>
                  Reset all keybinds
                </Button>
              </div>

              <p class="hint m-0 text-ui-meta text-muted-foreground">
                Shell actions are configured per repository in the Actions panel and the command
                palette. Recent repositories, pins and session restore are managed on the home
                screen.
              </p>

              <div class="inspector flex flex-col gap-2">
                <div class="inspector-head flex flex-wrap items-center justify-between gap-2">
                  <span class="text-sm font-medium text-foreground">
                    Local storage ({entries.length}
                    {entries.length === 1 ? "key" : "keys"})
                  </span>
                  <span class="flex flex-none gap-2">
                    <Button variant="outline" class="rounded-full" type="button" onclick={rescan}>
                      Refresh
                    </Button>
                    <Button variant="outline" class="rounded-full" type="button" onclick={() => void copyDiagnostics()}>
                      Copy diagnostics
                    </Button>
                  </span>
                </div>
                {#if entries.length === 0}
                  <p class="m-0 rounded-xl border border-dashed border-border px-4 py-3 text-ui-meta text-muted-foreground">
                    No “{APP_KEY_PREFIX}” entries in local storage.
                  </p>
                {:else}
                  <ul class="m-0 flex list-none flex-col gap-1 p-0" role="list" aria-label="Stored app settings">
                    {#each entries as entry (entry.key)}
                      <li>
                        <Collapsible.Root class="rounded-md border border-border bg-muted/30">
                          <Collapsible.Trigger
                            class="flex w-full cursor-pointer items-center justify-between gap-2 rounded-md px-2.5 py-1.5 text-left hover:bg-muted focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-primary"
                          >
                            <span class="min-w-0 truncate font-mono text-ui-meta text-foreground">{entry.key}</span>
                            <Icon icon={iconChevronDown} size={14} />
                          </Collapsible.Trigger>
                          <Collapsible.Content class="border-t border-border px-2.5 py-1.5">
                            <pre class="m-0 max-h-40 overflow-auto whitespace-pre-wrap break-all font-mono text-ui-micro text-muted-foreground">{prettyValue(entry.value)}</pre>
                          </Collapsible.Content>
                        </Collapsible.Root>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </div>

              <div class="danger flex items-center justify-between gap-4 rounded-xl border border-destructive/40 bg-destructive/5 px-4 py-3">
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span class="text-sm font-medium text-destructive">Reset all app settings</span>
                  <span class="text-ui-meta text-muted-foreground">
                    Clears every “{APP_KEY_PREFIX}” local-storage entry (theme, appearance, AI,
                    diff/editor/file-tree prefs, keybinds, update schedule, auto-fetch cadences,
                    Open With apps). Repositories on disk are not touched.
                  </span>
                </span>
                <Button variant="destructive" class="flex-none rounded-full" type="button" onclick={() => (resetOpen = true)}>
                  Reset settings…
                </Button>
              </div>
            </section>
          {/if}
        </div>
      </div>
    </Dialog.Content>
  </Dialog.Root>
{/if}

<ConfirmDialog
  bind:open={resetOpen}
  title="Reset all app settings?"
  message="Every “mygitui.*” entry in local storage is cleared. The global settings re-default immediately; the other stores re-read their defaults on the next launch. Repositories on disk are not touched."
  confirmLabel="Reset settings"
  danger
  onConfirm={resetAllSettings}
/>
