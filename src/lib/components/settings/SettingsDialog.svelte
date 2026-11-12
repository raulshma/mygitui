<script lang="ts">
  /**
   * Settings dialog — the single hub for every app-wide preference.
   *
   * The content is data-driven: a static {@link SECTIONS} registry describes
   * every setting as a typed row (toggle / segmented / number / action /
   * custom), each wiring its OWN owning store — nothing is duplicated or
   * re-persisted here. The view renders rows generically, which buys:
   *
   * - **Search** across every section (title + description + keywords) with
   *   grouped results — the fastest path to "where is X?".
   * - **Keyboard navigation**: `/` or Ctrl/Cmd+F focuses the search field,
   *   ArrowUp/ArrowDown/Home/End walk the section nav, Escape clears the
   *   search first and closes the dialog second.
   * - **Full-row toggles**: clicking anywhere on a switch row flips it.
   *
   * Related dialogs (appearance customizer, AI settings, About) open through
   * the typed UI bus / hosts while this dialog stays open; it renders in the
   * `z-40` band (overlay + content) so those spawned dialogs always stack
   * above it regardless of which portal mounted first.
   *
   * The Advanced section remains the escape hatch: a read-only inspector of
   * the `mygitui.*` localStorage keys with copy-diagnostics, a keybind
   * reset, and a confirmed danger reset that clears those keys (the global
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
  import { Slider } from "$lib/components/ui/slider/index.js";
  import { Hint } from "$lib/components/ui/tooltip/index.js";
  import * as Collapsible from "$lib/components/ui/collapsible/index.js";
  import ConfirmDialog from "$lib/components/safety/ConfirmDialog.svelte";
  import CustomAppsSection from "$lib/components/openWith/CustomAppsSection.svelte";
  import Icon from "$lib/components/icons/Icon.svelte";
  import { appIcon } from "$lib/components/icons/library.svelte";
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
  import { editorPrefs } from "$lib/components/files/editorPrefs.svelte";
  import { fileTreePrefs } from "$lib/components/panels/fileTreePrefs.svelte";
  import { fileIconPreferences } from "$lib/components/icons/fileIconPreferences.svelte";
  import { resetBindings } from "$lib/palette/keybinds";
  import { emitUiEvent } from "$lib/palette/events";
  import { openAboutDialog } from "$lib/components/aboutDialog";
  import { ai } from "$lib/ai/ai.svelte";
  import { formatDateTime } from "$lib/git/format";
  import iconChevronDown from "@ktibow/iconset-material-symbols/expand-more";

  /** Fallback when no Tauri runtime answers (browser/e2e); = package.json. */
  const FALLBACK_VERSION = "0.1.22";

  /** localStorage prefix the Advanced inspector / reset scope themselves to. */
  const APP_KEY_PREFIX = "mygitui.";

  // --- Row registry types -----------------------------------------------------

  /** One pill of a segmented control. */
  interface SegmentedOption {
    value: string;
    label: string;
  }

  /** Text that may be computed at render time (version lines, statuses). */
  type DynamicText = string | (() => string);

  function resolveText(text: DynamicText): string {
    return typeof text === "function" ? text() : text;
  }

  interface BaseRowDef {
    /** Stable id — the `settings-<id>-label` span anchors the control's label. */
    id: string;
    title: string;
    description?: DynamicText;
    /** Extra search terms beyond title + description. */
    keywords?: string;
  }

  /** Boolean setting: a full-row-clickable card with a switch. */
  interface ToggleRowDef extends BaseRowDef {
    kind: "toggle";
    get: () => boolean;
    set: (value: boolean) => void;
  }

  /** 2–3 choice setting rendered as a segmented pill control. */
  interface SegmentedRowDef extends BaseRowDef {
    kind: "segmented";
    options: readonly SegmentedOption[];
    get: () => string;
    set: (value: string) => void;
  }

  /** Numeric setting (number input; optionally a companion slider). */
  interface NumberRowDef extends BaseRowDef {
    kind: "number";
    ariaLabel: string;
    min: number;
    max: number;
    /** Unit suffix rendered after the input ("min", "px"). */
    unit?: string;
    /** Renders a slider alongside the input. */
    slider?: boolean;
    get: () => number;
    set: (value: number) => void;
  }

  /** One button of an {@link ActionRowDef}. */
  interface ActionButtonDef {
    label: DynamicText;
    onclick: () => void;
    disabled?: () => boolean;
    destructive?: boolean;
    /** Rendered only while this returns true (e.g. "Restart now"). */
    hidden?: () => boolean;
  }

  /** A card with text on the left and button(s) on the right. */
  interface ActionRowDef extends BaseRowDef {
    kind: "action";
    /** Live status line above the description (updater state). */
    status?: () => string | null;
    buttons: readonly ActionButtonDef[];
  }

  /** Hand-drawn block (override list, custom apps, storage inspector). */
  interface CustomRowDef extends BaseRowDef {
    kind: "custom";
    render: "overrides" | "apps" | "inspector";
  }

  type SettingRowDef =
    | ToggleRowDef
    | SegmentedRowDef
    | NumberRowDef
    | ActionRowDef
    | CustomRowDef;

  interface SectionDef {
    id: SectionId;
    label: string;
    /** Semantic icon name resolved through the active icon set. */
    icon: string;
    /** One-liner under the section heading. */
    blurb: string;
    intro?: string;
    outro?: string;
    rows: readonly SettingRowDef[];
  }

  type SectionId =
    | "general"
    | "appearance"
    | "git"
    | "editor"
    | "terminal"
    | "openwith"
    | "ai"
    | "updates"
    | "advanced";

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

  // --- Git: per-repo auto-fetch overrides -------------------------------------

  /** Per-repo cadence overrides, sorted for stable rendering. */
  const autofetchOverrides = $derived(
    Object.entries(autofetch.config).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)),
  );

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

  // The inspector reflects storage when it can be on screen (localStorage is
  // not reactive); the Refresh button re-runs it mid-visit.
  $effect(() => {
    if (!open) return;
    if (active === "advanced" || query.trim() !== "") rescan();
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

  // --- Search -----------------------------------------------------------------

  /** Search field text; empty string = not searching. */
  let query = $state("");
  let searchEl = $state<HTMLInputElement | null>(null);

  /** Search glyph resolved reactively (plain divs can't host `{@const}`). */
  const searchIcon = $derived(appIcon("search"));

  const normalizedQuery = $derived(query.trim().toLowerCase());
  const searching = $derived(normalizedQuery !== "");

  /** Static search haystack for one row. */
  function rowHaystack(row: SettingRowDef): string {
    const description = typeof row.description === "string" ? row.description : "";
    return `${row.title} ${description} ${row.keywords ?? ""}`.toLowerCase();
  }

  /** Picking a section is explicit intent — leave search mode. */
  function selectSection(id: SectionId): void {
    active = id;
    query = "";
  }

  function onSearchKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape" && query !== "") {
      event.preventDefault();
      event.stopPropagation();
      query = "";
    }
  }

  function onKeydown(event: KeyboardEvent): void {
    const target = event.target as HTMLElement | null;
    const inField =
      target !== null &&
      (target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.tagName === "SELECT" ||
        target.isContentEditable);
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f" && !inField) {
      event.preventDefault();
      searchEl?.focus();
      searchEl?.select();
      return;
    }
    if (event.key === "/" && !inField) {
      event.preventDefault();
      searchEl?.focus();
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  }

  // --- Nav keyboard walking ---------------------------------------------------

  function onNavKeydown(event: KeyboardEvent): void {
    if (
      event.key !== "ArrowDown" &&
      event.key !== "ArrowUp" &&
      event.key !== "Home" &&
      event.key !== "End"
    ) {
      return;
    }
    event.preventDefault();
    const index = SECTIONS.findIndex((section) => section.id === active);
    const last = SECTIONS.length - 1;
    const next =
      event.key === "ArrowDown"
        ? Math.min(index + 1, last)
        : event.key === "ArrowUp"
          ? Math.max(index - 1, 0)
          : event.key === "Home"
            ? 0
            : last;
    const target = SECTIONS[next];
    if (!target || target.id === active) return;
    selectSection(target.id);
    document.getElementById(`settings-nav-${target.id}`)?.focus();
  }

  // Keep the active section visible in the nav while walking with keys.
  // (Optional call: jsdom and old webviews don't implement scrollIntoView.)
  $effect(() => {
    if (!open) return;
    document.getElementById(`settings-nav-${active}`)?.scrollIntoView?.({
      block: "nearest",
    });
  });

  function close(): void {
    // `open` is bindable and the app shell passes no onclose — clearing it
    // here is what actually dismisses the dialog (Close button / Escape).
    open = false;
  }

  // --- Row setters ------------------------------------------------------------

  function setTheme(value: string): void {
    if (value === "system" || value === "light" || value === "dark") {
      themeChoice = value;
      setThemePreference(value);
    }
  }

  function setFileIcons(value: string): void {
    fileIconPreferences.setTheme(
      value === "material" || value === "minimal" ? value : "colorful",
    );
  }

  function setDiffMode(value: string): void {
    diffMode = value === "unified" ? "unified" : "split";
    writeDiffModePref(diffMode);
  }

  function setMdView(value: string): void {
    editorPrefs.setMdView(value === "edit" || value === "preview" ? value : "split");
  }

  function setTerminalTheme(value: string): void {
    if (value === "system" || value === "light" || value === "dark") {
      terminalThemePref.set(value satisfies TerminalThemePreference);
    }
  }

  // --- Section registry ---------------------------------------------------------

  /**
   * The dialog's entire content. Getters read reactive stores during render,
   * so every row tracks its store; setters write straight through.
   */
  const SECTIONS: readonly SectionDef[] = [
    {
      id: "general",
      label: "General",
      icon: "tune",
      blurb: "Theme and the app itself.",
      rows: [
        {
          kind: "segmented",
          id: "theme",
          title: "Theme",
          description:
            "“Match system” follows the OS and switches live; a fixed choice overrides it.",
          keywords: "dark light appearance scheme system",
          options: [
            { value: "system", label: "Match system" },
            { value: "light", label: "Light" },
            { value: "dark", label: "Dark" },
          ],
          get: () => themeChoice,
          set: setTheme,
        },
        {
          kind: "action",
          id: "about",
          title: "mygitui",
          description: () => `Version ${version} · Fast, configurable Git GUI`,
          keywords: "version about license info",
          buttons: [{ label: "About", onclick: () => openAboutDialog() }],
        },
      ],
      outro: "The interface language is English today; more languages are planned.",
    },
    {
      id: "appearance",
      label: "Appearance",
      icon: "palette",
      blurb: "How the app looks — customizer and file icons.",
      rows: [
        {
          kind: "action",
          id: "appearance-customize",
          title: "Customize appearance",
          description:
            "Style, base color, theme, chart colors, fonts, icon set, radius and menu accents — changes apply live.",
          keywords: "style font radius icons menu colors preset customizer",
          buttons: [
            { label: "Customize…", onclick: () => emitUiEvent("open-appearance") },
          ],
        },
        {
          kind: "segmented",
          id: "file-icons",
          title: "File icons",
          description: "File-type glyphs in the file trees and the diff header.",
          keywords: "icon set glyphs colorful material minimal",
          options: [
            { value: "colorful", label: "Colorful" },
            { value: "material", label: "Material" },
            { value: "minimal", label: "Minimal" },
          ],
          get: () => fileIconPreferences.theme,
          set: setFileIcons,
        },
      ],
    },
    {
      id: "git",
      label: "Git",
      icon: "account-tree",
      blurb: "Undo safety nets, confirmations and background fetching.",
      rows: [
        {
          kind: "toggle",
          id: "checkpoints",
          title: "Checkpoints for git changes",
          description:
            "Snapshot a restorable undo point before working-copy discards and cleans — restore them from the Undo panel.",
          keywords: "checkpoint undo snapshot safety",
          get: () => settings.checkpointsEnabled,
          set: (value) => settings.update({ checkpointsEnabled: value }),
        },
        {
          kind: "toggle",
          id: "confirm-discard",
          title: "Confirm before discard",
          description:
            "Ask for confirmation before discarding working-copy changes. Turn off if the prompt feels noisy — discards then apply immediately.",
          keywords: "confirm discard prompt destructive clean",
          get: () => settings.confirmDiscard,
          set: (value) => settings.update({ confirmDiscard: value }),
        },
        {
          kind: "number",
          id: "autofetch",
          title: "Auto-fetch every",
          description:
            "Default cadence for repositories without their own override; 0 disables auto-fetch. Per-repo overrides are set from the Branches panel.",
          keywords: "fetch interval background poll minutes cadence",
          ariaLabel: "Default auto-fetch interval in minutes",
          min: 0,
          max: AUTOFETCH_MINUTES_MAX,
          unit: "min",
          get: () => settings.defaultAutofetchMinutes,
          set: (value) =>
            settings.update({
              defaultAutofetchMinutes: Number.isFinite(value) ? value : 0,
            }),
        },
        {
          kind: "custom",
          id: "autofetch-overrides",
          title: "Per-repo auto-fetch overrides",
          keywords: "override cadence per repo interval",
          render: "overrides",
        },
      ],
    },
    {
      id: "editor",
      label: "Editor & Diffs",
      icon: "difference",
      blurb: "Diff layout, wrapping and file-editor defaults.",
      rows: [
        {
          kind: "segmented",
          id: "diff-layout",
          title: "Diff layout",
          description:
            "Split = side by side, unified = inline. Applies to diffs opened afterwards; the diff toolbar flips the current one.",
          keywords: "diff split unified side side-by-side inline layout compare",
          options: [
            { value: "split", label: "Split" },
            { value: "unified", label: "Unified" },
          ],
          get: () => diffMode,
          set: setDiffMode,
        },
        {
          kind: "toggle",
          id: "diff-wrap",
          title: "Wrap long diff lines",
          description:
            "Off = horizontal scrolling per diff, like the toolbar's wrap toggle.",
          keywords: "wrap diff lines soft horizontal scroll",
          get: () => diffWrap,
          set: (value) => {
            diffWrap = value;
            writeWrapPref(value);
          },
        },
        {
          kind: "toggle",
          id: "editor-wrap",
          title: "Wrap lines in the editor",
          description: "Soft-wrap for files opened in the file workspace.",
          keywords: "wrap editor lines soft",
          get: () => editorPrefs.wrap,
          set: (value) => editorPrefs.setWrap(value),
        },
        {
          kind: "segmented",
          id: "md-view",
          title: "Markdown default view",
          description:
            "How markdown files open in the editor (Ctrl+Shift+V cycles live).",
          keywords: "markdown preview split edit view",
          options: [
            { value: "edit", label: "Edit" },
            { value: "split", label: "Split" },
            { value: "preview", label: "Preview" },
          ],
          get: () => editorPrefs.mdView,
          set: setMdView,
        },
        {
          kind: "toggle",
          id: "show-ignored",
          title: "Show ignored files",
          description: "List gitignored paths in the Files panel tree.",
          keywords: "ignored gitignored hidden files tree",
          get: () => fileTreePrefs.showIgnored,
          set: (value) => fileTreePrefs.setShowIgnored(value),
        },
      ],
    },
    {
      id: "terminal",
      label: "Terminal",
      icon: "terminal",
      blurb: "Palette and text size for the built-in terminal.",
      rows: [
        {
          kind: "segmented",
          id: "terminal-theme",
          title: "Terminal theme",
          description:
            "“Match app” derives colors from the scheme live; a fixed choice pins an independent palette. Applies to running sessions immediately.",
          keywords: "terminal colors palette dark light scheme",
          options: [
            { value: "system", label: "Match app" },
            { value: "light", label: "Light" },
            { value: "dark", label: "Dark" },
          ],
          get: () => terminalThemePref.preference,
          set: setTerminalTheme,
        },
        {
          kind: "number",
          id: "terminal-font",
          title: "Font size",
          description:
            "Saved with the app settings — applies to new terminal sessions (use the panel's Restart to pick it up); running sessions keep their current size.",
          keywords: "font size text pixels scale terminal",
          ariaLabel: "Terminal font size in pixels",
          min: TERMINAL_FONT_MIN,
          max: TERMINAL_FONT_MAX,
          unit: "px",
          slider: true,
          get: () => settings.terminalFontSize,
          set: (value) =>
            settings.update({
              terminalFontSize: Number.isFinite(value)
                ? value
                : APP_SETTINGS_DEFAULTS.terminalFontSize,
            }),
        },
      ],
    },
    {
      id: "openwith",
      label: "Open With",
      icon: "open-in-new",
      blurb: "Editors and tools that open files from the header menu.",
      intro:
        "Detected editors come from this machine; custom apps below join them in the header's “Open with” menu. Nothing is synced.",
      rows: [
        {
          kind: "custom",
          id: "custom-apps",
          title: "Custom apps",
          keywords: "custom apps open with editors launcher commands",
          render: "apps",
        },
      ],
    },
    {
      id: "ai",
      label: "AI",
      icon: "bolt",
      blurb: "Provider wiring for AI features.",
      rows: [
        {
          kind: "action",
          id: "ai-provider",
          title: "AI provider",
          description: () =>
            `Preferred backend: ${aiBackendLabel}. Backends, models, connection checks and the per-repo feature opt-ins live in the AI settings dialog.`,
          keywords: "openrouter opencode model llm provider backend",
          buttons: [
            { label: "AI settings…", onclick: () => emitUiEvent("open-ai-settings") },
          ],
        },
      ],
    },
    {
      id: "updates",
      label: "Updates",
      icon: "cloud-download",
      blurb: "Automatic checks, downloads and install timing.",
      rows: [
        {
          kind: "toggle",
          id: "auto-check",
          title: "Check for updates automatically",
          description: "Polls at most every 12 hours after launch.",
          keywords: "update check automatic poll",
          get: () => updater.autoCheck,
          set: (value) => updater.setAutoCheck(value),
        },
        {
          kind: "toggle",
          id: "auto-download",
          title: "Download updates automatically",
          description:
            "Starts the download as soon as an update is found; installing always waits for your restart click.",
          keywords: "update download automatic install",
          get: () => updater.autoDownload,
          set: (value) => updater.setAutoDownload(value),
        },
        {
          kind: "action",
          id: "update-status",
          title: "Update status",
          status: () => updateStatusLine,
          description: () => `Last check: ${lastCheckLine}`,
          keywords: "update check version restart install latest",
          buttons: [
            {
              label: () => (updateBusy ? "Checking…" : "Check for updates"),
              onclick: () => void updater.checkNow({ manual: true }),
              disabled: () => updateBusy,
            },
            {
              label: "Restart now",
              onclick: () => updater.relaunch(),
              hidden: () => updater.status !== "ready",
            },
          ],
        },
      ],
    },
    {
      id: "advanced",
      label: "Advanced",
      icon: "inventory",
      blurb: "Keybinds, local storage and the danger zone.",
      rows: [
        {
          kind: "action",
          id: "keybinds",
          title: "Keyboard shortcuts",
          description:
            "Rebind commands from the command palette (Ctrl/Cmd+Shift+P); overrides are remembered per machine.",
          keywords: "keybinds shortcuts hotkeys reset bindings palette",
          buttons: [{ label: "Reset all keybinds", onclick: resetKeybinds }],
        },
        {
          kind: "custom",
          id: "storage-inspector",
          title: "Local storage",
          keywords: "storage diagnostics keys json debug inspector",
          render: "inspector",
        },
        {
          kind: "action",
          id: "reset-all",
          title: "Reset all app settings",
          description:
            "Clears every “mygitui.*” local-storage entry (theme, appearance, AI, diff/editor/file-tree prefs, keybinds, update schedule, auto-fetch cadences, Open With apps). Repositories on disk are not touched.",
          keywords: "reset wipe clear defaults danger destructive",
          buttons: [
            {
              label: "Reset settings…",
              onclick: () => (resetOpen = true),
              destructive: true,
            },
          ],
        },
      ],
      outro:
        "Shell actions are configured per repository in the Actions panel and the command palette. Recent repositories, pins and session restore are managed on the home screen.",
    },
  ];

  /** Shared card chrome for every row. */
  const ROW_CARD =
    "flex items-center justify-between gap-4 rounded-xl border border-border bg-card/40 px-4 py-3 transition-colors";
  const ROW_TEXT = "flex min-w-0 flex-col gap-0.5";
  const ROW_TITLE = "text-sm font-medium text-foreground";
  const ROW_DESC = "text-ui-meta text-muted-foreground";

  const activeSection = $derived(
    SECTIONS.find((section) => section.id === active) ?? SECTIONS[0],
  );

  /** Sections with ≥1 matching row, in nav order. */
  const searchResults = $derived.by(() => {
    if (!searching) return [];
    return SECTIONS.map((section) => ({
      section,
      rows: section.rows.filter((row) => rowHaystack(row).includes(normalizedQuery)),
    })).filter((group) => group.rows.length > 0);
  });

  /** Total matching rows across sections (for the result count line). */
  const searchResultCount = $derived(
    searchResults.reduce((sum, group) => sum + group.rows.length, 0),
  );
</script>

<!-- Row renderer: shared by the section view and search results. -->
{#snippet renderRows(rowList: readonly SettingRowDef[])}
  {#each rowList as row (row.id)}
    {#if row.kind === "toggle"}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="setting-row {ROW_CARD} cursor-pointer hover:border-ring/50 hover:bg-muted/40"
        onclick={() => row.set(!row.get())}
      >
        <span class={ROW_TEXT}>
          <span id={`settings-${row.id}-label`} class={ROW_TITLE}>{row.title}</span>
          <span class={ROW_DESC}>{resolveText(row.description ?? "")}</span>
        </span>
        <Switch
          class="mt-0.5 flex-none"
          aria-labelledby={`settings-${row.id}-label`}
          checked={row.get()}
          onclick={(event) => event.stopPropagation()}
          onCheckedChange={(checked) => row.set(checked)}
        />
      </div>
    {:else if row.kind === "segmented"}
      <div class="setting-row {ROW_CARD}">
        <span class={ROW_TEXT}>
          <span id={`settings-${row.id}-label`} class={ROW_TITLE}>{row.title}</span>
          <span class={ROW_DESC}>{resolveText(row.description ?? "")}</span>
        </span>
        <div
          class="flex flex-none items-center rounded-lg border border-border bg-muted/60 p-0.5"
          role="radiogroup"
          aria-labelledby={`settings-${row.id}-label`}
        >
          {#each row.options as option (option.value)}
            <button
              type="button"
              role="radio"
              aria-checked={row.get() === option.value}
              class="cursor-pointer rounded-md px-2.5 py-1 text-ui-meta transition-colors {row.get() === option.value
                ? 'bg-background font-medium text-foreground shadow-sm'
                : 'text-muted-foreground hover:text-foreground'} focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary"
              onclick={() => row.set(option.value)}
            >
              {option.label}
            </button>
          {/each}
        </div>
      </div>
    {:else if row.kind === "number"}
      <div class="setting-row {ROW_CARD}">
        <span class={ROW_TEXT}>
          <span id={`settings-${row.id}-label`} class={ROW_TITLE}>{row.title}</span>
          <span class={ROW_DESC}>{resolveText(row.description ?? "")}</span>
        </span>
        <span class="flex flex-none items-center gap-2">
          {#if row.slider}
            <Slider
              class="w-24 flex-none"
              type="single"
              min={row.min}
              max={row.max}
              step={1}
              aria-label={`${row.ariaLabel} slider`}
              value={row.get()}
              onValueChange={(next) => row.set(next)}
            />
          {/if}
          <span class="flex flex-none items-center gap-1">
            <Input
              class="w-16 rounded-md text-center"
              type="number"
              min={row.min}
              max={row.max}
              step={1}
              aria-label={row.ariaLabel}
              value={row.get()}
              onchange={(event) => {
                const raw = Number((event.currentTarget as HTMLInputElement).value);
                row.set(raw);
              }}
            />
            {#if row.unit}
              <span class="text-ui-meta text-muted-foreground">{row.unit}</span>
            {/if}
          </span>
        </span>
      </div>
    {:else if row.kind === "action"}
      <div class="setting-row {ROW_CARD} bg-muted/30">
        <span class={ROW_TEXT}>
          <span class={ROW_TITLE}>{row.title}</span>
          {#if row.status}
            {@const statusText = row.status()}
            {#if statusText !== null}
              <span class="text-sm text-foreground" role="status">{statusText}</span>
            {/if}
          {/if}
          <span class={ROW_DESC}>{resolveText(row.description ?? "")}</span>
        </span>
        <span class="flex flex-none items-center gap-2">
          {#each row.buttons.filter((button) => !button.hidden?.()) as button, i (i)}
            <Button
              variant={button.destructive ? "destructive" : "outline"}
              class="rounded-full"
              type="button"
              disabled={button.disabled?.()}
              onclick={button.onclick}
            >
              {resolveText(button.label)}
            </Button>
          {/each}
        </span>
      </div>
    {:else if row.render === "overrides"}
      <div class="overrides flex flex-col gap-1.5">
        <span id="settings-autofetch-overrides-label" class={ROW_TITLE}>{row.title}</span>
        {#if autofetchOverrides.length === 0}
          <p class="m-0 {ROW_DESC}">
            No overrides — every open repository follows the default cadence.
          </p>
        {:else}
          <ul class="m-0 flex list-none flex-col gap-1 p-0" role="list" aria-label="Per-repo auto-fetch overrides">
            {#each autofetchOverrides as [root, minutes] (root)}
              <li class="flex min-w-0 items-center justify-between gap-3 rounded-md border border-border px-2.5 py-1.5">
                <span class="min-w-0 truncate font-mono text-ui-meta text-foreground" title={root}>{root}</span>
                <span class="flex flex-none items-center gap-2">
                  <span class={ROW_DESC}>{minutes} min</span>
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
    {:else if row.render === "apps"}
      <CustomAppsSection store={apps} />
    {:else if row.render === "inspector"}
      <div class="inspector flex flex-col gap-2">
        <div class="inspector-head flex flex-wrap items-center justify-between gap-2">
          <span class={ROW_TITLE}>
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
          <p class="m-0 rounded-xl border border-dashed border-border px-4 py-3 {ROW_DESC}">
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
    {/if}
  {/each}
{/snippet}


{#if open}
  <Dialog.Root bind:open>
    <!--
      z-40 band: the settings hub is the only dialog that SPAWNS other dialogs
      (appearance customizer, AI settings, About). Those keep the default z-50,
      so they always stack above the hub no matter which portal mounted first.
    -->
    <Dialog.Content
      class="flex h-[min(38rem,calc(100dvh_-_2rem))] w-[min(52rem,calc(100vw_-_2rem))] max-w-none flex-col overflow-hidden text-popover-foreground sm:max-w-none z-40"
      overlayClass="z-40 bg-black/40"
      role="dialog"
      aria-modal="true"
      aria-labelledby="settings-title"
      aria-describedby="settings-description"
      showCloseButton={false}
      escapeKeydownBehavior="ignore"
      interactOutsideBehavior="ignore"
      onkeydown={onKeydown}
    >
      <header class="flex flex-none items-start justify-between gap-4 border-b border-border px-5 pb-3 pt-4">
        <div>
          <Dialog.Title id="settings-title" class="m-0 font-semibold text-foreground">Settings</Dialog.Title>
          <p id="settings-description" class="mb-0 mt-1 text-ui-base text-muted-foreground">
            Every app-wide preference in one place. Changes apply immediately.
          </p>
        </div>
        <Button variant="ghost" size="icon" class="flex-none rounded-full text-lg leading-none text-muted-foreground" type="button" aria-label="Close settings" onclick={close}>
          ×
        </Button>
      </header>

      <div class="flex min-h-0 flex-1">
        <!-- Sticky section nav: search + icon list -->
        <nav
          class="flex w-44 flex-none flex-col gap-0.5 overflow-y-auto border-r border-border bg-muted/30 px-2 py-3"
          aria-label="Settings sections"
        >
          <div class="relative mb-2 px-0.5">
            {#if searchIcon}
              <span class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground">
                <Icon icon={searchIcon} size={13} />
              </span>
            {/if}
              <input
                bind:this={searchEl}
                bind:value={query}
                class="w-full rounded-md border border-border bg-background py-1 pl-7 pr-6 text-ui-meta text-foreground placeholder:text-muted-foreground focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary"
                type="text"
                placeholder="Search…  /"
                aria-label="Search settings"
                onkeydown={onSearchKeydown}
              />
              {#if query !== ""}
                <button
                  type="button"
                  class="absolute right-1.5 top-1/2 -translate-y-1/2 cursor-pointer rounded-full px-1 text-ui-meta text-muted-foreground hover:text-foreground focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary"
                  aria-label="Clear search"
                  onclick={() => (query = "")}
                >
                  ×
                </button>
              {/if}
          </div>
          {#each SECTIONS as section (section.id)}
            {@const sectionIcon = appIcon(section.icon)}
            <button
              id={`settings-nav-${section.id}`}
              class="flex w-full cursor-pointer items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-sm transition-colors {active === section.id
                ? 'bg-accent font-medium text-accent-foreground'
                : 'text-muted-foreground hover:bg-muted hover:text-foreground'} focus-visible:-outline-offset-1 focus-visible:outline-2 focus-visible:outline-primary"
              type="button"
              aria-current={active === section.id ? "true" : undefined}
              onclick={() => selectSection(section.id)}
              onkeydown={onNavKeydown}
            >
              {#if sectionIcon}
                <span class="flex-none {active === section.id ? 'text-primary' : 'text-muted-foreground/80'}">
                  <Icon icon={sectionIcon} size={15} />
                </span>
              {/if}
              <span class="truncate">{section.label}</span>
            </button>
          {/each}
        </nav>

        <!-- Content pane (the only scroller) -->
        <div class="min-h-0 flex-1 overflow-y-auto px-5 py-4">
          {#if searching}
            <section aria-label="Search results" class="flex flex-col gap-4">
              <div class="flex flex-col gap-0.5 pb-1">
                <h3 class="m-0 text-sm font-semibold text-foreground">Search results</h3>
                <p class="m-0 {ROW_DESC}">
                  {#if searchResults.length === 0}
                    Nothing matches “{query}”.
                  {:else}
                    {searchResultCount}
                    {searchResultCount === 1 ? "setting" : "settings"} in
                    {searchResults.length}
                    {searchResults.length === 1 ? "section" : "sections"}
                  {/if}
                </p>
              </div>
              {#if searchResults.length === 0}
                <p class="m-0 rounded-xl border border-dashed border-border px-4 py-6 text-center {ROW_DESC}">
                  Try “wrap”, “theme”, “fetch”, “terminal”, “keys” or “updates”.
                </p>
              {:else}
                {#each searchResults as group (group.section.id)}
                  <section class="flex flex-col gap-2" aria-label={group.section.label}>
                    <h4 class="m-0 text-ui-meta font-semibold uppercase tracking-wide text-muted-foreground">
                      {group.section.label}
                    </h4>
                    {@render renderRows(group.rows)}
                  </section>
                {/each}
              {/if}
            </section>
          {:else}
            <section aria-labelledby="settings-section-heading" class="flex flex-col gap-2.5">
              <div id="settings-section-heading" class="flex flex-col gap-0.5 pb-1">
                <h3 class="m-0 text-sm font-semibold text-foreground">{activeSection.label}</h3>
                <p class="m-0 {ROW_DESC}">{activeSection.blurb}</p>
              </div>
              {#if activeSection.intro}
                <p class="m-0 {ROW_DESC}">{activeSection.intro}</p>
              {/if}
              {@render renderRows(activeSection.rows)}
              {#if activeSection.outro}
                <p class="m-0 {ROW_DESC}">{activeSection.outro}</p>
              {/if}
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
